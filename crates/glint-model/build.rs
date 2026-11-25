//! Compiles the interface's catalogs, the gettext files in the repository's `locales/` folder, into tables
//! sorted by their English: `zh-CN.po` becomes `ZH_CN` in `$OUT_DIR/catalogs.rs`, which src/i18n.rs includes.
//! Nothing is read or parsed when Glint starts, and a lookup is a binary search over static text.
//!
//! What goes in is src/po.rs's `usable`: an entry no one has translated yet, or marked `fuzzy`, is left out and
//! its English shows. So is an entry that can't stand in for its English (placeholders that differ, a context,
//! plural forms, the same English twice): those go in `REJECTED` too, which a test requires to be empty, and the
//! build prints them as warnings. A translator's slip never breaks the build, and never reaches the window
//! either. A catalog that can't be read at all (a quote left open) stops the build, with the file and line.

#[path = "src/po.rs"]
#[allow(dead_code)]
mod po;

use std::fmt::Write as _;
use std::path::Path;

/// The catalogs src/i18n.rs's `Language` names. One that's missing becomes an empty table (all English), so the
/// crate still builds; a file not listed here is compiled too, for the day its language joins.
const KNOWN: &[&str] = &["zh-CN"];

fn main() {
    let locales = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../locales");
    println!("cargo:rerun-if-changed=src/po.rs");

    let mut files: Vec<_> = std::fs::read_dir(&locales)
        .map(|dir| {
            dir.filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "po"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    // The catalogs themselves, not the folder: `node scripts/i18n.mjs extract` rewriting glint.pot beside them
    // mustn't rebuild glint-model and all that uses it. A new language is added to KNOWN, which reruns this.
    for path in &files {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    if KNOWN.iter().any(|code| {
        !files
            .iter()
            .any(|path| path.file_stem().is_some_and(|stem| stem == *code))
    }) {
        // A catalog that's missing: watch the folder so the build notices when it comes.
        println!("cargo:rerun-if-changed={}", locales.display());
    }

    let mut out = String::from("// Made by build.rs from locales/*.po: don't edit.\n\n");
    let mut rejected: Vec<(String, po::Rejected)> = Vec::new();
    for path in &files {
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let code = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("locales/{name}: {e}"));
        let catalog = po::parse(&text).unwrap_or_else(|e| panic!("locales/{name}, {e}"));
        let (kept, left_out) = po::usable(catalog);
        rejected.extend(left_out.into_iter().map(|r| (name.clone(), r)));
        let _ = writeln!(
            out,
            "/// locales/{name}: {} translated texts, sorted by their English.",
            kept.len()
        );
        let _ = writeln!(
            out,
            "#[allow(dead_code)]\npub(crate) static {}: &[(&str, &str)] = &[",
            ident(&code)
        );
        for (english, translation) in &kept {
            let _ = writeln!(out, "    ({english:?}, {translation:?}),");
        }
        out.push_str("];\n\n");
    }
    for code in KNOWN {
        if !files
            .iter()
            .any(|path| path.file_stem().is_some_and(|stem| stem == *code))
        {
            println!("cargo:warning=locales/{code}.po is missing: that language shows in English");
            let _ = writeln!(
                out,
                "/// locales/{code}.po is missing.\npub(crate) static {}: &[(&str, &str)] = &[];\n",
                ident(code)
            );
        }
    }
    out.push_str("/// The entries the build left out, and why: (file, line, English, why). The tests require none.\n");
    out.push_str("#[allow(dead_code)]\npub(crate) static REJECTED: &[(&str, usize, &str, &str)] = &[\n");
    for (file, r) in &rejected {
        println!(
            "cargo:warning=locales/{file}:{}: {:?} is left out: {}",
            r.line, r.english, r.why
        );
        let _ = writeln!(out, "    ({file:?}, {}, {:?}, {:?}),", r.line, r.english, r.why);
    }
    out.push_str("];\n");

    let dest = Path::new(&std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("catalogs.rs");
    std::fs::write(&dest, out).unwrap_or_else(|e| panic!("{}: {e}", dest.display()));
}

/// The table's name for a catalog's code: "zh-CN" is `ZH_CN`.
fn ident(code: &str) -> String {
    code.to_uppercase().replace(['-', '.'], "_")
}
