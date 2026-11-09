//! Content Credentials (C2PA): what a picture or video says of where it came from, signed by whoever
//! made or changed it. A camera that signs its photos, Photoshop, an AI image generator. Read by
//! c2patool (the Content Authenticity Initiative's own, in Rust; installed with one click), offline: it
//! checks the signature against what the file holds, and says when the signer isn't one it trusts.

use crate::fsx::blocking;
use serde::Serialize;

#[derive(Serialize, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    /// The app (and version) that made the last change: "Adobe Photoshop 26.1", "DALL·E".
    pub made_with: Option<String>,
    /// Who signed it, as their certificate says.
    pub signed_by: Option<String>,
    /// When it was signed (RFC 3339).
    pub signed_at: Option<String>,
    /// What was done to it, in words ("Created", "Edited", "Filtered"…), in order, each once.
    pub actions: Vec<String>,
    /// It says AI made it (all of it, or part of it).
    pub ai: Option<&'static str>,
    /// How many files it was made from (a composite, an edit of a photo).
    pub ingredients: usize,
    /// The signature holds over what the file holds now.
    pub valid: bool,
    /// The signer is one c2patool trusts (its list), not just some certificate.
    pub trusted: bool,
}

/// An action's words.
fn action_words(action: &str) -> String {
    let name = action.rsplit('.').next().unwrap_or(action);
    match name {
        "created" => "Created".into(),
        "edited" => "Edited".into(),
        "opened" => "Opened".into(),
        "placed" => "Placed in it".into(),
        "cropped" => "Cropped".into(),
        "resized" => "Resized".into(),
        "filtered" => "Filtered".into(),
        "color_adjustments" => "Colors adjusted".into(),
        "converted" => "Converted".into(),
        "transcoded" => "Transcoded".into(),
        "published" => "Published".into(),
        "drawing" => "Drawn on".into(),
        "removed" => "Something removed".into(),
        "repackaged" => "Repackaged".into(),
        other => other.replace('_', " "),
    }
}

/// What c2patool's report says, for the file's active manifest.
fn read_report(json: &str) -> Option<Credentials> {
    let report: serde_json::Value = serde_json::from_str(json).ok()?;
    let active = report["active_manifest"].as_str()?;
    let manifest = &report["manifests"][active];
    let info = &manifest["claim_generator_info"][0];
    let made_with = match (info["name"].as_str(), info["version"].as_str()) {
        (Some(name), Some(version)) => Some(format!("{name} {version}")),
        (Some(name), None) => Some(name.to_string()),
        _ => manifest["claim_generator"].as_str().map(|g| g.split(['/', ' ']).next().unwrap_or(g).to_string()),
    };
    let signature = &manifest["signature_info"];
    let mut actions = Vec::new();
    let mut ai = None;
    for assertion in manifest["assertions"].as_array().into_iter().flatten() {
        let label = assertion["label"].as_str().unwrap_or_default();
        if !label.starts_with("c2pa.actions") {
            continue;
        }
        for action in assertion["data"]["actions"].as_array().into_iter().flatten() {
            if let Some(name) = action["action"].as_str() {
                let words = action_words(name);
                if !actions.contains(&words) {
                    actions.push(words);
                }
            }
            match action["digitalSourceType"].as_str().map(|t| t.rsplit('/').next().unwrap_or(t)) {
                Some("trainedAlgorithmicMedia") => ai = Some("made by AI"),
                Some("compositeWithTrainedAlgorithmicMedia" | "algorithmicMedia") if ai.is_none() => ai = Some("partly made by AI"),
                _ => {}
            }
        }
    }
    let statuses: Vec<&str> = report["validation_status"].as_array().into_iter().flatten().filter_map(|s| s["code"].as_str()).collect();
    let failed = statuses.iter().any(|c| c.contains("mismatch") || c.ends_with(".invalid") || c.contains("missing") || c.contains("revoked") || c.contains("expired"));
    Some(Credentials {
        made_with,
        signed_by: signature["issuer"].as_str().or(signature["common_name"].as_str()).map(str::to_string),
        signed_at: signature["time"].as_str().map(str::to_string),
        actions,
        ai,
        ingredients: manifest["ingredients"].as_array().map_or(0, Vec::len),
        valid: !failed,
        trusted: !failed && !statuses.contains(&"signingCredential.untrusted"),
    })
}

/// What a picture or video's Content Credentials say: Ok(None) when it has none, Err("tool") when
/// c2patool isn't installed.
pub async fn content_credentials(path: String) -> Result<Option<Credentials>, String> {
    blocking(move || {
        let Ok(mut tool) = crate::tools::command("c2patool") else { return Err("tool".into()) };
        tool.arg(&path);
        crate::proc::no_window(&mut tool);
        let out = tool.output().map_err(|e| format!("Couldn't run c2patool: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() {
            let said = String::from_utf8_lossy(&out.stderr);
            // No manifest in it: the usual case.
            if said.contains("No claim found") || said.contains("not found") || said.contains("unsupported") {
                return Ok(None);
            }
            return Err(said.trim().trim_start_matches("Error: ").to_string());
        }
        Ok(read_report(&text))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_a_manifest_says() {
        let report = r#"{
          "active_manifest": "urn:uuid:1",
          "manifests": { "urn:uuid:1": {
            "claim_generator_info": [{ "name": "Adobe Firefly", "version": "4.0" }],
            "assertions": [{ "label": "c2pa.actions.v2", "data": { "actions": [
              { "action": "c2pa.created", "digitalSourceType": "http://cv.iptc.org/newscodes/digitalsourcetype/trainedAlgorithmicMedia" },
              { "action": "c2pa.edited" }, { "action": "c2pa.edited" }
            ] } }],
            "ingredients": [{}, {}],
            "signature_info": { "issuer": "Adobe Inc.", "time": "2026-11-01T10:00:00+00:00" }
          } },
          "validation_status": []
        }"#;
        let c = read_report(report).unwrap();
        assert_eq!(c.made_with.as_deref(), Some("Adobe Firefly 4.0"));
        assert_eq!(c.actions, ["Created", "Edited"]);
        assert_eq!((c.ai, c.ingredients, c.valid, c.trusted), (Some("made by AI"), 2, true, true));
        assert_eq!(c.signed_by.as_deref(), Some("Adobe Inc."));
        let untrusted = report.replace(r#""validation_status": []"#, r#""validation_status": [{ "code": "signingCredential.untrusted" }]"#);
        let c = read_report(&untrusted).unwrap();
        assert!(c.valid && !c.trusted);
        let tampered = report.replace(r#""validation_status": []"#, r#""validation_status": [{ "code": "assertion.dataHash.mismatch" }]"#);
        assert!(!read_report(&tampered).unwrap().valid);
        assert!(read_report("{}").is_none());
    }
}
