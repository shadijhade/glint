//! A Git repository's card, when its folder is previewed: what onefetch says of it (its languages, who
//! works on it, when it last changed, its version and license) and tokei's lines of code by language,
//! both optional tools installed on demand. A card is kept for the commit the repository is at, so it
//! comes back at once; each tool gets 20 seconds.

use crate::fsx::blocking;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Author {
    pub name: String,
    pub commits: u64,
    /// Its share of the commits, in percent.
    pub share: u64,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Lines {
    pub language: String,
    pub code: u64,
    pub comments: u64,
    pub blanks: u64,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RepoCard {
    /// onefetch and tokei: whether each was there to ask.
    pub onefetch: bool,
    pub tokei: bool,
    pub name: Option<String>,
    pub description: Option<String>,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub version: Option<String>,
    /// When its first and last commits were made (RFC 3339).
    pub created: Option<String>,
    pub last_change: Option<String>,
    /// Each language's share of the code, in percent, biggest first.
    pub languages: Vec<(String, f64)>,
    pub authors: Vec<Author>,
    pub contributors: Option<u64>,
    pub commits: Option<u64>,
    pub url: Option<String>,
    pub license: Option<String>,
    /// tokei's lines by language, the most code first, and their total.
    pub lines: Vec<Lines>,
    pub total: Option<Lines>,
}

/// Cards by repository root and the commit it's at.
static CARDS: LazyLock<Mutex<HashMap<String, RepoCard>>> = LazyLock::new(Default::default);

/// What `command` prints, given TIMEOUT; None when it fails or takes too long.
fn output(mut command: Command) -> Option<String> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let text = reader.join().ok()?;
    status.success().then_some(text)
}

/// onefetch's `--output json`: a list of one-field objects, each a part of what it shows.
fn read_onefetch(json: &str, card: &mut RepoCard) {
    let Ok(value) = serde_json::from_str::<Value>(json) else { return };
    let text = |v: &Value| v.as_str().map(str::to_string).filter(|s| !s.is_empty());
    for field in value["infoFields"].as_array().into_iter().flatten() {
        let Some((kind, info)) = field.as_object().and_then(|o| o.iter().next()) else { continue };
        match kind.as_str() {
            "ProjectInfo" => card.name = text(&info["repoName"]),
            "DescriptionInfo" => card.description = text(&info["description"]),
            "HeadInfo" => {
                card.head = text(&info["headRefs"]["shortCommitId"]);
                card.branch = info["headRefs"]["refs"].as_array().and_then(|refs| refs.first()).and_then(text);
            }
            "VersionInfo" => card.version = text(&info["version"]),
            "CreatedInfo" => card.created = text(&info["creationDate"]),
            "LastChangeInfo" => card.last_change = text(&info["lastChange"]),
            "LanguagesInfo" => {
                card.languages = info["languagesWithPercentage"].as_array().into_iter().flatten().filter_map(|l| Some((text(&l["language"])?, l["percentage"].as_f64()?))).collect();
            }
            "AuthorsInfo" => {
                card.authors = info["authors"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|a| Some(Author { name: text(&a["name"])?, commits: a["nbrOfCommits"].as_u64().unwrap_or(0), share: a["contribution"].as_u64().unwrap_or(0) }))
                    .collect();
            }
            "ContributorsInfo" => card.contributors = info["totalNumberOfAuthors"].as_u64(),
            "CommitsInfo" => card.commits = info["numberOfCommits"].as_u64(),
            "UrlInfo" => card.url = text(&info["repoUrl"]),
            "LicenseInfo" => card.license = text(&info["license"]),
            _ => {}
        }
    }
}

/// tokei's `--output json`: lines by language (and "Total"), each with its files' reports, which
/// aren't needed.
fn read_tokei(json: &str, card: &mut RepoCard) {
    let Ok(Value::Object(languages)) = serde_json::from_str::<Value>(json) else { return };
    let lines = |language: &str, v: &Value| Lines { language: language.to_string(), code: v["code"].as_u64().unwrap_or(0), comments: v["comments"].as_u64().unwrap_or(0), blanks: v["blanks"].as_u64().unwrap_or(0) };
    for (language, counts) in &languages {
        if language == "Total" {
            card.total = Some(lines(language, counts));
        } else if counts["code"].as_u64().unwrap_or(0) > 0 {
            card.lines.push(lines(language, counts));
        }
    }
    card.lines.sort_by(|a, b| b.code.cmp(&a.code).then_with(|| a.language.cmp(&b.language)));
}

/// The card of the repository whose folder `path` is (its working folder's root), or None when it
/// isn't one.
#[tauri::command]
pub async fn repo_card(path: String) -> Result<Option<RepoCard>, String> {
    blocking(move || {
        let Ok(repo) = gix::discover(&path) else { return Ok(None) };
        let Some(root) = repo.workdir().map(Path::to_path_buf) else { return Ok(None) };
        let same = |a: &Path, b: &Path| a.to_string_lossy().trim_end_matches(['\\', '/']).replace('/', "\\").eq_ignore_ascii_case(b.to_string_lossy().trim_end_matches(['\\', '/']));
        if !same(&root, Path::new(&path)) {
            return Ok(None);
        }
        let (onefetch, tokei) = (crate::tools::command("onefetch").ok(), crate::tools::command("tokei").ok());
        // Kept for the commit, and for the tools there: one installed since reads it again.
        let head = repo.head_id().map(|id| id.to_string()).unwrap_or_default();
        let key = format!("{}|{head}|{}{}", path.to_lowercase(), onefetch.is_some(), tokei.is_some());
        if let Some(card) = CARDS.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return Ok(Some(card.clone()));
        }
        let mut card = RepoCard { onefetch: onefetch.is_some(), tokei: tokei.is_some(), ..Default::default() };
        if let Some(mut onefetch) = onefetch {
            onefetch.arg(&path).args(["--output", "json"]);
            if let Some(json) = output(onefetch) {
                read_onefetch(&json, &mut card);
            }
        }
        if let Some(mut tokei) = tokei {
            tokei.arg(&path).args(["--output", "json"]);
            if let Some(json) = output(tokei) {
                read_tokei(&json, &mut card);
            }
        }
        CARDS.lock().unwrap_or_else(|e| e.into_inner()).insert(key, card.clone());
        Ok(Some(card))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_onefetch_says() {
        let json = r#"{"title":{"gitUsername":"me"},"infoFields":[
            {"ProjectInfo":{"repoName":"glint","numberOfBranches":2}},
            {"DescriptionInfo":{"description":null}},
            {"HeadInfo":{"headRefs":{"shortCommitId":"9ebade7","refs":["main"]}}},
            {"VersionInfo":{"version":"v0.6.0"}},
            {"CreatedInfo":{"creationDate":"2025-11-21T17:04:37Z"}},
            {"LanguagesInfo":{"languagesWithPercentage":[{"language":"Rust","percentage":60.7},{"language":"Tsx","percentage":27.6}]}},
            {"AuthorsInfo":{"authors":[{"name":"shadow","email":null,"nbrOfCommits":98,"contribution":67}]}},
            {"LastChangeInfo":{"lastChange":"2026-10-23T23:07:37Z"}},
            {"ContributorsInfo":{"totalNumberOfAuthors":4}},
            {"UrlInfo":{"repoUrl":"https://github.com/shadijhade/glint.git"}},
            {"CommitsInfo":{"numberOfCommits":147,"isShallow":false}},
            {"SomethingNew":{"x":1}},
            {"LicenseInfo":{"license":"MIT"}}]}"#;
        let mut card = RepoCard::default();
        read_onefetch(json, &mut card);
        assert_eq!((card.name.as_deref(), card.description.as_deref(), card.head.as_deref(), card.branch.as_deref()), (Some("glint"), None, Some("9ebade7"), Some("main")));
        assert_eq!((card.version.as_deref(), card.license.as_deref(), card.commits, card.contributors), (Some("v0.6.0"), Some("MIT"), Some(147), Some(4)));
        assert_eq!(card.languages, vec![("Rust".to_string(), 60.7), ("Tsx".to_string(), 27.6)]);
        assert_eq!(card.authors, vec![Author { name: "shadow".into(), commits: 98, share: 67 }]);
        assert_eq!(card.last_change.as_deref(), Some("2026-10-23T23:07:37Z"));
        read_onefetch("not json", &mut card);
        assert_eq!(card.name.as_deref(), Some("glint"), "nothing changes when it isn't JSON");
    }

    #[test]
    fn reads_lines_from_tokei() {
        let json = r#"{"Rust":{"blanks":10,"code":500,"comments":40,"reports":[{"name":"a.rs"}]},"Markdown":{"blanks":3,"code":0,"comments":90},
            "TSX":{"blanks":5,"code":800,"comments":2},"Total":{"blanks":18,"code":1300,"comments":132}}"#;
        let mut card = RepoCard::default();
        read_tokei(json, &mut card);
        assert_eq!(card.lines.iter().map(|l| (l.language.as_str(), l.code)).collect::<Vec<_>>(), [("TSX", 800), ("Rust", 500)]);
        assert_eq!(card.total.as_ref().map(|t| (t.code, t.comments, t.blanks)), Some((1300, 132, 18)));
    }
}
