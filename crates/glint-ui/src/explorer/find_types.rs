//! The types of searching by name: what a search is asked, what it answers, and where instant search stands.
//! Ported from `FindOptions` and `FindEvent` in src/lib/fs.ts, `Wider` in src/lib/explorer.ts and the parts of
//! `IndexInfo` (fs.ts) that `indexed` and `serviceOutdated` (explorer.ts) read.
//!
//! The run a tab keeps ([`FindRun`](super::FindRun)) is in `types.rs`; the methods that drive it are in
//! `find.rs`. Nothing here touches GPUI or the disk.

use glint_model::entries::FileEntry;

/// What a search is told besides its words: the settings it follows (`FindOptions` in fs.ts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindOptions {
    /// Include hidden and system files (Settings → Show hidden items).
    pub hidden: bool,
    /// Respect .gitignore and .ignore files (fd's alone: the index doesn't read them).
    pub respect_ignore: bool,
    /// Names or globs to skip (Settings → Find → Folders to skip).
    pub exclude: Vec<String>,
    /// The most results to keep; `None` for no limit.
    pub max_results: Option<usize>,
    /// When nothing matches as typed, the index looks again with typos allowed (fd doesn't).
    pub fuzzy: bool,
    /// The index sends the best `max_results` of every drive's matches, best first.
    pub rank: bool,
}

/// One thing a running search says (`FindEvent` in fs.ts).
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    /// Who answers: "index", "fd", or "index + fd" when the index covers only some of the folders.
    Started { engine: String },
    /// A batch of results, each with its size, date and attributes. Batches add to each other.
    Entries(Vec<FileEntry>),
    /// The search failed (a bad filter, fd could not start…). What was found so far is dropped.
    Error(String),
    /// The search ended. `count` is all it found; `limited` says it stopped at the result limit; `fuzzy` says
    /// nothing matched as typed and these are the index's close matches.
    Finished {
        count: usize,
        elapsed: f64,
        limited: bool,
        fuzzy: bool,
    },
}

/// Searching again more widely than Settings say, once: elsewhere, without the limit, or skipping nothing
/// (`Wider` in explorer.ts).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wider {
    /// Search here instead of where the tab is.
    pub scope: Option<String>,
    /// Keep every result, not only the first `findMaxResults`.
    pub unlimited: bool,
    /// Look in the folders Settings skip, and in what .gitignore files list.
    pub skip_nothing: bool,
}

impl Wider {
    /// The same search again where it was run (F5, "Search again").
    pub fn in_scope(scope: impl Into<String>) -> Wider {
        Wider {
            scope: Some(scope.into()),
            ..Default::default()
        }
    }

    /// Without the result limit.
    pub fn without_limit(mut self) -> Wider {
        self.unlimited = true;
        self
    }

    /// Skipping nothing.
    pub fn skipping_nothing(mut self) -> Wider {
        self.skip_nothing = true;
        self
    }
}

/// Where instant search stands, as far as searching cares (`IndexInfo` in fs.ts, with `indexed` and
/// `serviceOutdated` of explorer.ts folded in).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchIndex {
    /// The service runs and answers.
    pub on: bool,
    /// It isn't running, but turning it on would start it (not installed, or stopped).
    pub can_turn_on: bool,
    /// It runs an older build than the app's: until it is updated, fd answers queries with filters or paths.
    pub outdated: bool,
    /// The drives it has read and keeps up, as "C:" (upper case).
    pub ready: Vec<String>,
}

impl SearchIndex {
    /// Whether the index has the drive `path` is on, ready to answer (`indexed`).
    pub fn indexed(&self, path: &str) -> bool {
        if !self.on {
            return false;
        }
        // The TypeScript took `path.slice(0, 2)`: the drive letter and its colon.
        let drive: String = path.chars().take(2).collect::<String>().to_uppercase();
        self.ready.contains(&drive)
    }

    /// Whether the app's version is newer than the service's, comparing their numbers as numbers (`serviceOutdated`).
    /// A newer service (from another account's Glint) serves this app as it is.
    pub fn outdated_by(on: bool, service: Option<&str>, app: &str) -> bool {
        let numbers = !app.is_empty()
            && app
                .split('.')
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
        let Some(service) = service.filter(|s| !s.is_empty()) else {
            return false;
        };
        on && numbers && numeric_cmp(app, service) == std::cmp::Ordering::Greater
    }
}

/// `0.10.0` against `0.9.5`: numbers compared as numbers (`localeCompare(.., { numeric: true })`).
fn numeric_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |s: &str| -> Vec<u64> { s.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x.cmp(&y);
        }
    }
    std::cmp::Ordering::Equal
}

/// What searching has to tell the person that is not about one file (the TypeScript's toasts in `runFind`).
/// The workspace words them with `t!`, as the toasts were.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FindNotice {
    /// `Nothing to search below the Recycle Bin`, with `What you type already narrows what's shown in it.` as
    /// its description (toast id "find-recycled"): what is in the Recycle Bin isn't in folders to search.
    NothingBelowRecycleBin,
    /// `Instant search finds names in milliseconds` (description: `It reads each drive's file table once and
    /// keeps up with every change. Windows asks for administrator approval.`, a "Turn on" button, 15 s): after
    /// a search without the index took over a second, and instant search could be turned on. The TypeScript
    /// showed it once ever (`firstNotice("offer-instant-search", "1")`); the model raises it at most once per
    /// run of the app, and whoever words it keeps the "once ever" in the saved state.
    OfferInstantSearch,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drive_is_indexed_when_the_service_runs_and_has_read_it() {
        let index = SearchIndex {
            on: true,
            ready: vec!["C:".into(), "D:".into()],
            ..Default::default()
        };
        assert!(index.indexed("C:\\Users"));
        assert!(index.indexed("d:\\"), "the drive letter is compared in upper case");
        assert!(!index.indexed("E:\\"));
        assert!(
            !SearchIndex { on: false, ..index }.indexed("C:\\"),
            "a stopped service answers nothing"
        );
    }

    #[test]
    fn a_service_is_outdated_only_when_it_runs_and_is_older_than_the_app() {
        assert!(SearchIndex::outdated_by(true, Some("0.9.5"), "0.10.0"));
        assert!(
            !SearchIndex::outdated_by(true, Some("0.10.0"), "0.9.5"),
            "a newer service serves this app"
        );
        assert!(!SearchIndex::outdated_by(false, Some("0.9.5"), "0.10.0"));
        assert!(!SearchIndex::outdated_by(true, None, "0.10.0"));
        assert!(
            !SearchIndex::outdated_by(true, Some("0.9.5"), "0.10.0-beta"),
            "only plain numbers compare"
        );
    }
}
