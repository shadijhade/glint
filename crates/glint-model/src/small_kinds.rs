//! Ported from src/lib/smallKinds.ts.
//!
//! The small file kinds with their own previews (contacts, calendars, mail...). The TypeScript kept this
//! table apart from the preview component so that FilePreview could tell which kind a file is without
//! loading the preview module. The same split keeps the table here, away from the UI crate.

use serde::{Deserialize, Serialize};

/// The kinds of file the small previews show. The TS string union becomes this enum; the serialized names
/// are the TS strings ("contact", "calendar", ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmallKind {
    /// vCard contacts (.vcf, .vcard).
    Contact,
    /// Calendar entries (.ics, .ical).
    Calendar,
    /// Internet e-mail messages (.eml).
    Email,
    /// Outlook messages (.msg).
    Outlook,
    /// Paint and design files that are archives of layers (.kra, .ora, .procreate, .sketch).
    Design,
    /// Graphviz graphs (.dot, .gv).
    Graphviz,
}

/// The small preview kind of a file extension, or `None` when no small preview handles it.
///
/// The extension is lowercased and has no dot (`FileEntry.ext`), and the match is exact, as the TS lookup
/// was. The TS function is named `smallKind`; this is the name the port's callers use.
///
/// NOTE(port): the TS looked the extension up in a plain object, so names that object inherits from
/// `Object.prototype` (`constructor`, `toString`, `__proto__`...) came back as a function, not `undefined`,
/// and FilePreview would have tried to show that as a kind. Rust returns `None` for them, which is what the
/// code meant. The coordinator may want to know that such a file name used to take the small-preview path.
pub fn small_kind_of(ext: &str) -> Option<SmallKind> {
    match ext {
        "vcf" | "vcard" => Some(SmallKind::Contact),
        "ics" | "ical" => Some(SmallKind::Calendar),
        "eml" => Some(SmallKind::Email),
        "msg" => Some(SmallKind::Outlook),
        "kra" | "ora" | "procreate" | "sketch" => Some(SmallKind::Design),
        "dot" | "gv" => Some(SmallKind::Graphviz),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The check files do not assert smallKind, so these tests cover the table itself: one case per group.
    #[test]
    fn small_kind_of_maps_each_extension_group() {
        assert_eq!(small_kind_of("vcf"), Some(SmallKind::Contact));
        assert_eq!(small_kind_of("vcard"), Some(SmallKind::Contact));
        assert_eq!(small_kind_of("ics"), Some(SmallKind::Calendar));
        assert_eq!(small_kind_of("ical"), Some(SmallKind::Calendar));
        assert_eq!(small_kind_of("eml"), Some(SmallKind::Email));
        assert_eq!(small_kind_of("msg"), Some(SmallKind::Outlook));
        assert_eq!(small_kind_of("kra"), Some(SmallKind::Design));
        assert_eq!(small_kind_of("ora"), Some(SmallKind::Design));
        assert_eq!(small_kind_of("procreate"), Some(SmallKind::Design));
        assert_eq!(small_kind_of("sketch"), Some(SmallKind::Design));
        assert_eq!(small_kind_of("dot"), Some(SmallKind::Graphviz));
        assert_eq!(small_kind_of("gv"), Some(SmallKind::Graphviz));
        // Mermaid files are text since 0.12.
        assert_eq!(small_kind_of("mmd"), None);
    }

    #[test]
    fn small_kind_of_is_exact_and_ignores_unknown_extensions() {
        assert_eq!(small_kind_of("VCF"), None);
        assert_eq!(small_kind_of(""), None);
        assert_eq!(small_kind_of("txt"), None);
        assert_eq!(small_kind_of("constructor"), None);
        assert_eq!(small_kind_of("__proto__"), None);
    }

    #[test]
    fn small_kind_serializes_to_the_ts_names() {
        assert_eq!(
            serde_json::to_string(&SmallKind::Graphviz).ok().as_deref(),
            Some("\"graphviz\"")
        );
        assert_eq!(
            serde_json::from_str::<SmallKind>("\"design\"").ok(),
            Some(SmallKind::Design)
        );
    }
}
