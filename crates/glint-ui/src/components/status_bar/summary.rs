//! What the status bar says about the active tab, worked out from the explorer's state: the counts, the selection,
//! how the listing came and how long it took, the search run, and the folder's breakdown by kind.
//! Ported from the derived values of src/components/explorer/StatusBar.tsx (`items`, `spectrum`, `selectedFiles`,
//! `telemetryStages` and the branches that build `work`).
//!
//! The view calls [`Summary::of`] when the explorer changes, not on every frame, and draws what it holds. Nothing
//! here touches GPUI, so the text and the numbers are tested on their own.

use crate::components::file_grid::thumbnail::ThumbRun;
use crate::explorer::{Engine, Tab, TabId};
use crate::t;
use glint_model::entries::FileEntry;
use glint_model::file_types::{group_filter, type_group, TypeGroup, TYPE_GROUPS};
use glint_model::format::{format_count, format_duration, format_size};
use glint_model::store::ViewMode;
use gpui_kit::SharedString;

/// The folders' color on the breakdown (`FOLDERS_GROUP` in StatusBar.tsx). Folders aren't a kind of file, so they
/// get their own group, with the hue and saturation the TypeScript gave them.
pub const FOLDERS: TypeGroup = TypeGroup {
    name: "Folders",
    hue: 38,
    sat: 82,
};

/// How many entries the breakdown looks at: enough to say what a folder is made of, with no wait on a huge one
/// (the TypeScript's sample of 4,000).
pub const SAMPLE: usize = 4000;

/// One kind's share of the folder: what the breakdown's bar and chips show.
#[derive(Clone, Debug, PartialEq)]
pub struct Category {
    pub group: TypeGroup,
    /// Items of this kind in the sample.
    pub count: usize,
    /// Bytes of the files of this kind (folders count nothing until the folder sizes part is ported).
    pub bytes: u64,
    /// The search box text that shows only this kind; empty when there is none (other files).
    pub query: String,
}

/// The breakdown of a folder's first [`SAMPLE`] entries, most common kind first. Ties keep the order the kinds
/// first appeared in, as the TypeScript's sort (stable) did.
pub fn spectrum(entries: &[FileEntry]) -> Vec<Category> {
    let mut out: Vec<Category> = Vec::new();
    for entry in entries.iter().take(SAMPLE) {
        let group = if entry.dir { FOLDERS } else { *type_group(entry) };
        let bytes = if entry.dir { 0 } else { entry.size };
        match out.iter_mut().find(|c| c.group.name == group.name) {
            Some(category) => {
                category.count += 1;
                category.bytes += bytes;
            }
            None => {
                let query = if entry.dir {
                    "folder:".to_string()
                } else if group != TYPE_GROUPS.other {
                    group_filter(&group)
                } else {
                    String::new()
                };
                out.push(Category {
                    group,
                    count: 1,
                    bytes,
                    query,
                });
            }
        }
    }
    out.sort_by_key(|category| std::cmp::Reverse(category.count));
    out
}

/// A search run, as the bar words it (`FindRun`, the parts the bar shows).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FindInfo {
    /// Who answers: "index", "fd", "index + fd"; empty until known.
    pub engine: String,
    pub fuzzy: bool,
    pub running: bool,
    pub found: usize,
    pub elapsed: f64,
    pub limited: bool,
}

/// An archive's list of entries, as the bar words it (`archive` in the listing).
#[derive(Clone, Debug, PartialEq)]
pub struct ArchiveInfo {
    /// The archive's file name ("backup.zip").
    pub name: String,
    pub format: &'static str,
    pub cached: bool,
    pub entries: usize,
    pub read_ms: f64,
}

/// What the bar shows about the active tab. Built by [`Summary::of`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    /// The tab the bar follows (the active one).
    pub tab: Option<TabId>,
    /// This PC, with no search: the bar says how many drives there are.
    pub this_pc: bool,
    /// How many drives this PC has (`drives` in the explorer).
    pub drives: usize,
    /// The tab has a listing (a folder, an archive, the index's view of a closed folder, a place).
    pub has_listing: bool,
    /// Windows keeps the folder closed and nothing could be listed: the bar says "No access".
    pub no_access: bool,
    /// The rows the list shows (the TypeScript's `rows.length`).
    pub shown: usize,
    /// Items the "Show hidden items" setting left out.
    pub hidden: usize,
    /// Rows selected (the TypeScript's `selected.length`).
    pub selected: usize,
    /// The selected rows' paths, in row order: what Copy paths copies.
    pub selected_paths: Vec<String>,
    /// Selected rows that are files, and their bytes (`selectedFiles`, `selectedBytes`).
    pub selected_files: usize,
    pub selected_bytes: u64,
    /// Entries in the folder's listing (not only the rows shown): the popover's header counts them.
    pub listing_entries: usize,
    /// How long the listing took, and who answered.
    pub read_ms: Option<f64>,
    pub engine: Option<Engine>,
    /// Milliseconds from the click to the list on screen (`shownIn`).
    pub shown_in: f64,
    pub archive: Option<ArchiveInfo>,
    pub find: Option<FindInfo>,
    /// The search box's text (`filter`).
    pub filter: String,
    /// The folder's breakdown by kind (empty without a listing).
    pub spectrum: Vec<Category>,
    /// The tab's lane in the thumbnail service (`tab3`): the bar reads that tab's burst of thumbnails by it.
    pub lane: String,
    /// The scope the grid times its thumbnails under (`dir:C:\a:grid`) while the tab shows the grid; empty in
    /// the other views, where the bar says nothing about thumbnails.
    pub thumbs_scope: String,
}

impl Summary {
    /// The summary of `tab` (`None` when there is no tab), with this PC's `drives`.
    pub fn of(tab: Option<&Tab>, drives: usize) -> Summary {
        let Some(tab) = tab else {
            return Summary {
                drives,
                this_pc: true,
                ..Summary::default()
            };
        };
        let find = tab.find.as_ref().map(|find| FindInfo {
            engine: find.engine.clone(),
            fuzzy: find.fuzzy,
            running: find.running,
            found: find.entries.len(),
            elapsed: find.elapsed,
            limited: find.limited,
        });
        let (selected_paths, selected_files, selected_bytes) = selected_of(tab);
        let mut summary = Summary {
            tab: Some(tab.id),
            this_pc: tab.is_this_pc() && find.is_none(),
            drives,
            has_listing: tab.listing.is_some(),
            no_access: tab.is_closed() && tab.listing.is_none() && find.is_none(),
            shown: tab.rows.len(),
            hidden: tab.rows.hidden,
            selected: selected_paths.len(),
            selected_paths,
            selected_files,
            selected_bytes,
            listing_entries: tab.entries().len(),
            read_ms: None,
            engine: None,
            shown_in: tab.shown_in,
            archive: None,
            find,
            filter: tab.filter.clone(),
            spectrum: Vec::new(),
            lane: tab.id.to_string(),
            thumbs_scope: if matches!(tab.view, ViewMode::Grid) {
                format!("{}:grid", tab.view_key())
            } else {
                String::new()
            },
        };
        if let Some(listing) = &tab.listing {
            summary.read_ms = Some(listing.elapsed);
            summary.engine = Some(listing.engine());
            summary.archive = listing.archive.as_ref().map(|archive| ArchiveInfo {
                name: glint_model::paths::base_name(&archive.path).to_string(),
                format: archive.format,
                cached: archive.cached,
                entries: archive.entries as usize,
                read_ms: archive.read_ms,
            });
            summary.spectrum = spectrum(&listing.entries);
        }
        summary
    }
}

/// The selected rows (in row order), with how many of them are files and their bytes. A single selection is
/// found by its row, the others by scanning the rows (`selected` in Explorer.tsx).
fn selected_of(tab: &Tab) -> (Vec<String>, usize, u64) {
    let selection = &tab.selection;
    let entries: Vec<&FileEntry> = if selection.len() == 1 {
        selection
            .iter()
            .next()
            .and_then(|path| tab.rows.position_of(path))
            .and_then(|at| tab.rows.get(at))
            .into_iter()
            .collect()
    } else if selection.is_empty() {
        Vec::new()
    } else {
        tab.rows.iter().filter(|e| selection.contains(&e.path)).collect()
    };
    let files: Vec<&&FileEntry> = entries.iter().filter(|e| !e.dir).collect();
    let bytes = files.iter().fold(0u64, |sum, e| sum.saturating_add(e.size));
    (entries.iter().map(|e| e.path.clone()).collect(), files.len(), bytes)
}

// ---- Words ------------------------------------------------------------------------------------------------

/// "1 item" or "12 items", counted in the user's grouping (`items` in StatusBar.tsx).
pub fn items(n: usize) -> SharedString {
    if n == 1 {
        t!("1 item")
    } else {
        t!("{count} items", count = format_count(n))
    }
}

/// "1 drive" or "3 drives" (the This PC line).
pub fn drives(n: usize) -> SharedString {
    if n == 1 {
        t!("1 drive")
    } else {
        t!("{count} drives", count = n)
    }
}

/// The counts on the left: items, hidden, selected, and the selection's size.
pub fn counts(s: &Summary) -> SharedString {
    let mut text = if s.no_access && !s.has_listing && s.find.is_none() {
        t!("No access").to_string()
    } else {
        items(s.shown).to_string()
    };
    if s.hidden > 0 {
        text.push_str(&format!(" · {}", t!("{count} hidden", count = format_count(s.hidden))));
    }
    if s.selected > 0 {
        text.push_str(&format!(
            " · {}",
            t!("{count} selected", count = format_count(s.selected))
        ));
    }
    if s.selected_files > 0 {
        text.push_str(&format!(" · {}", format_size(s.selected_bytes)));
    }
    text.into()
}

/// What the bar says about a search that is running or finished.
pub fn search_text(find: &FindInfo) -> SharedString {
    let engine = if find.engine.is_empty() {
        String::new()
    } else {
        format!("{} · ", find.engine)
    };
    let fuzzy = if find.fuzzy {
        format!("{} · ", t!("fuzzy"))
    } else {
        String::new()
    };
    if find.running {
        return t!(
            "{engine}searching… {found} found",
            engine = engine,
            found = format_count(find.found)
        );
    }
    let stopped = if find.limited {
        format!(" · {}", t!("stopped at the limit"))
    } else {
        String::new()
    };
    t!(
        "{engine}{fuzzy}{found} found in {time}{stopped}",
        engine = engine,
        fuzzy = fuzzy,
        found = format_count(find.found),
        time = format_duration(find.elapsed),
        stopped = stopped
    )
}

/// One stretch of the time the folder took to show, for the popover's "How long it took".
#[derive(Clone, Debug, PartialEq)]
pub struct Stage {
    pub label: SharedString,
    pub detail: SharedString,
    pub ms: f64,
}

/// The stages of showing the folder: reading it, and getting it on screen (`telemetryStages`). The folder sizes
/// and Git status stages come with their parts; the thumbnails' stage is [`thumbs_stage`].
pub fn stages(s: &Summary) -> Vec<Stage> {
    let mut out = Vec::new();
    if let (Some(ms), Some(engine)) = (s.read_ms, s.engine) {
        let detail = match engine {
            Engine::Archive(format) => SharedString::from(format),
            Engine::Index => t!("from the index"),
            _ => t!("from the drive"),
        };
        out.push(Stage {
            label: t!("Folder read"),
            detail,
            ms,
        });
    }
    if s.shown_in > 0.0 {
        out.push(Stage {
            label: t!("On screen"),
            detail: items(s.shown),
            ms: s.shown_in,
        });
    }
    out
}

/// The thumbnails of the items on screen, as the status bar reads them from the file grid's burst for the tab
/// (`ThumbRun`). A plain copy: the run keeps its own id, and the words and the stage are worked out from these.
#[derive(Clone, Debug, PartialEq)]
pub struct Thumbs {
    /// Thumbnails that came.
    pub found: usize,
    /// Some are still on their way (`run.pending` in StatusBar.tsx).
    pub pending: bool,
    /// From the burst's first request to its last answer, ms.
    pub elapsed_ms: f64,
    /// Where they came from: Windows' thumbnail cache, made from their files, or core's memory.
    pub cache: usize,
    pub made: usize,
    /// Milliseconds the shell spent making the ones it made.
    pub made_ms: f64,
    pub memory: usize,
}

impl Thumbs {
    /// The status bar's view of a burst.
    pub fn of(run: &ThumbRun) -> Thumbs {
        Thumbs {
            found: run.found,
            pending: run.pending > 0,
            elapsed_ms: run.elapsed_ms,
            cache: run.cache,
            made: run.made,
            made_ms: run.made_ms,
            memory: run.memory,
        }
    }
}

/// "12 thumbnails in 3.2 ms", or "12 thumbnails…" while more are on their way (`ThumbWork` in StatusBar.tsx).
pub fn thumbs_line(th: &Thumbs) -> SharedString {
    let count = format_count(th.found);
    match (th.pending, th.found == 1) {
        (true, true) => t!("{count} thumbnail…", count = count),
        (true, false) => t!("{count} thumbnails…", count = count),
        (false, true) => t!(
            "{count} thumbnail in {time}",
            count = count,
            time = format_duration(th.elapsed_ms)
        ),
        (false, false) => t!(
            "{count} thumbnails in {time}",
            count = count,
            time = format_duration(th.elapsed_ms)
        ),
    }
}

/// The thumbnails' hover text: how long the burst took, how fast, and from where (`ThumbWork`'s title).
pub fn thumbs_title(th: &Thumbs) -> SharedString {
    let elapsed = format_duration(th.elapsed_ms);
    // Math.round of the rate, halves up; no rate when no time has passed.
    let rate = if th.elapsed_ms > 0.0 {
        (th.found as f64 / (th.elapsed_ms / 1000.0) + 0.5).floor()
    } else {
        0.0
    };
    let rate_text = if rate >= 1.0 {
        format!(" {}", t!("({rate} a second)", rate = format_count(rate as usize)))
    } else {
        String::new()
    };
    let mut from: Vec<String> = Vec::new();
    if th.cache > 0 {
        from.push(t!("{count} from Windows' thumbnail cache", count = format_count(th.cache)).to_string());
    }
    if th.made > 0 {
        let each = format_duration(th.made_ms / th.made as f64);
        from.push(
            t!(
                "{count} made from their files ({each} each)",
                count = format_count(th.made),
                each = each
            )
            .to_string(),
        );
    }
    if th.memory > 0 {
        from.push(t!("{count} from memory", count = format_count(th.memory)).to_string());
    }
    t!(
        "Thumbnails of the items on screen, {time} from the first request to the last{rate}: {from}.",
        time = elapsed,
        rate = rate_text,
        from = from.join(", ")
    )
}

/// The thumbnails' stage in "How long it took": its time, and how many came from the cache (`telemetryStages`).
/// None while some are still on their way, as the TypeScript left the stage out then.
pub fn thumbs_stage(th: &Thumbs) -> Option<Stage> {
    if th.pending {
        return None;
    }
    Some(Stage {
        label: t!("Thumbnails ({count})", count = format_count(th.found)),
        detail: t!("{count} cached", count = th.cache),
        ms: th.elapsed_ms,
    })
}

/// The bar's right side for a folder: the text, and what it says when hovered.
#[derive(Clone, Debug, PartialEq)]
pub enum Work {
    /// Nothing to say: no listing yet, or This PC (which says how many drives there are instead).
    Nothing,
    /// A search, running or finished.
    Search { running: bool, text: SharedString },
    /// A folder inside an archive: read from the archive's own list, which is read-only.
    Archive {
        lead: SharedString,
        shown: SharedString,
        title: SharedString,
    },
    /// A folder Windows keeps closed: listed from instant search's index.
    Index {
        lead: SharedString,
        shown: SharedString,
        title: SharedString,
    },
    /// A folder read from its drive (or a remote connection, or a place such as the Recycle Bin).
    Disk {
        lead: SharedString,
        shown: SharedString,
        title: SharedString,
    },
}

impl Work {
    /// The work line for `s`. `lead` is the part a narrow bar leaves out (it says how long reading took).
    pub fn of(s: &Summary) -> Work {
        if let Some(find) = &s.find {
            return Work::Search {
                running: find.running,
                text: search_text(find),
            };
        }
        if s.this_pc {
            return Work::Nothing;
        }
        let (Some(read_ms), Some(engine)) = (s.read_ms, s.engine) else {
            return Work::Nothing;
        };
        let read = format_duration(read_ms);
        let shown_in = format_duration(s.shown_in);
        let shown = t!("shown in {time}", time = shown_in.clone());
        match engine {
            Engine::Archive(_) => {
                let archive = s.archive.clone().unwrap_or(ArchiveInfo {
                    name: String::new(),
                    format: "",
                    cached: false,
                    entries: 0,
                    read_ms: 0.0,
                });
                let lead = if archive.cached {
                    t!(
                        "{format} · read in {time} ·",
                        format = archive.format,
                        time = read.clone()
                    )
                } else {
                    t!(
                        "{format} · {count} items read in {time} ·",
                        format = archive.format,
                        count = format_count(archive.entries),
                        time = read.clone()
                    )
                };
                let title = if archive.cached {
                    t!(
                        "Glint already had the list of what's in {name} ({count} files and folders) in memory, and listed this folder from it in {read}; it was on screen {shown} after the click. The archive is read-only.",
                        name = archive.name,
                        count = format_count(archive.entries),
                        read = read,
                        shown = shown_in
                    )
                } else {
                    t!(
                        "Glint read the list of what's in {name} ({count} files and folders) in {archive_read}, without extracting anything; this folder was on screen {shown} after the click. The archive is read-only.",
                        name = archive.name,
                        count = format_count(archive.entries),
                        archive_read = format_duration(archive.read_ms),
                        shown = shown_in
                    )
                };
                Work::Archive { lead, shown, title }
            }
            Engine::Index => Work::Index {
                lead: t!("index · listed in {time} ·", time = read.clone()),
                shown,
                title: t!(
                    "Windows keeps this folder closed to your account, so Glint listed what instant search's index has in it, in {time}; it was on screen {shown} after the click. Nothing in it can be opened or changed here.",
                    time = read,
                    shown = shown_in
                ),
            },
            _ => Work::Disk {
                lead: t!("read in {time} ·", time = read.clone()),
                shown,
                title: t!(
                    "Glint read this folder in {time}; it was on screen {shown} after the click. Click for what's in it.",
                    time = read,
                    shown = shown_in
                ),
            },
        }
    }
}

/// The breakdown segment's hover text: its name, count and size ("Folders: 3 (click to filter)").
pub fn segment_title(category: &Category) -> SharedString {
    let name = t!(category.group.name);
    let count = format_count(category.count);
    if category.bytes > 0 {
        t!(
            "{name}: {count} ({size})",
            name = name,
            count = count,
            size = format_size(category.bytes)
        )
    } else {
        t!("{name}: {count} (click to filter)", name = name, count = count)
    }
}

/// A chip's hover text: what clicking it does, or just its name when it can't filter.
pub fn chip_title(category: &Category) -> SharedString {
    if category.query.is_empty() {
        t!(category.group.name)
    } else {
        t!("Click to filter to {name}", name = category.group.name.to_lowercase())
    }
}

/// Whether the search box already shows `category` (so clicking it clears the filter).
pub fn chip_active(category: &Category, filter: &str) -> bool {
    !category.query.is_empty() && filter.trim() == category.query
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::{file, folder, hidden};
    use crate::explorer::{build_rows, Listed, RowSpec, Selection};
    use crate::testing;
    use glint_model::store::ViewMode;
    use std::sync::Arc;

    fn tab_with(entries: Vec<FileEntry>) -> Tab {
        let mut tab = Tab::blank(TabId(7), "C:\\a", ViewMode::List, None);
        let entries: Arc<[FileEntry]> = Arc::from(entries);
        tab.rows = build_rows(&entries, &RowSpec::default(), 0);
        tab.listing = Some(Listed {
            path: "C:\\a".into(),
            entries,
            elapsed: 1.5,
            archive: None,
            from_index: false,
            remote: false,
        });
        tab
    }

    fn select(tab: &mut Tab, paths: &[&str]) {
        tab.selection = Selection::of(paths.iter().copied());
    }

    #[test]
    fn the_breakdown_puts_the_most_common_kind_first_and_names_its_filter() {
        let _guard = testing::lock();
        let entries = vec![
            folder("C:\\a", "sub"),
            file("C:\\a", "a.txt", 100, 1),
            file("C:\\a", "b.txt", 200, 1),
            file("C:\\a", "c.txt", 300, 1),
            file("C:\\a", "pic.png", 50, 1),
        ];
        let parts = spectrum(&entries);
        assert_eq!(parts[0].count, 3, "three text files lead");
        assert_eq!(parts[0].bytes, 600);
        assert_eq!(parts[0].query, group_filter(&parts[0].group));
        let folders = parts.iter().find(|c| c.group == FOLDERS).expect("folders are a kind");
        assert_eq!(folders.query, "folder:");
        assert_eq!(folders.bytes, 0, "folder sizes are not counted yet");
        let images = parts.iter().find(|c| c.group == TYPE_GROUPS.images).expect("pictures");
        assert_eq!(images.count, 1);
        assert!(images.query.starts_with("ext:"));
    }

    #[test]
    fn other_files_have_no_filter_and_ties_keep_their_first_order() {
        let entries = vec![file("C:\\a", "mystery.qqq", 1, 1), file("C:\\a", "pic.png", 1, 1)];
        let parts = spectrum(&entries);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].group, TYPE_GROUPS.other);
        assert!(parts[0].query.is_empty(), "other files can't be filtered to");
        assert_eq!(parts[1].group, TYPE_GROUPS.images, "a tie keeps the first kind first");
    }

    #[test]
    fn the_breakdown_looks_at_no_more_than_the_sample() {
        let entries: Vec<FileEntry> = (0..SAMPLE + 10)
            .map(|i| file("C:\\a", &format!("f{i}.png"), 1, 1))
            .collect();
        assert_eq!(spectrum(&entries)[0].count, SAMPLE);
    }

    #[test]
    fn counts_say_items_hidden_selected_and_the_size_of_selected_files() {
        let _guard = testing::lock();
        let mut tab = tab_with(vec![
            folder("C:\\a", "sub"),
            file("C:\\a", "file10.txt", 100, 10),
            file("C:\\a", "notes.md", 200, 20),
            hidden("C:\\a", ".secret"),
        ]);
        let s = Summary::of(Some(&tab), 0);
        assert_eq!(s.shown, 3);
        assert_eq!(s.hidden, 1);
        assert_eq!(counts(&s), "3 items · 1 hidden");

        select(&mut tab, &["C:\\a\\notes.md"]);
        let s = Summary::of(Some(&tab), 0);
        assert_eq!(s.selected, 1);
        assert_eq!(counts(&s), "3 items · 1 hidden · 1 selected · 200 B");
        assert_eq!(s.selected_paths, ["C:\\a\\notes.md"]);

        select(&mut tab, &["C:\\a\\sub", "C:\\a\\file10.txt", "C:\\a\\nope.txt"]);
        let s = Summary::of(Some(&tab), 0);
        assert_eq!(s.selected, 2, "a path that isn't a row isn't counted");
        assert_eq!(s.selected_files, 1, "folders don't add bytes");
        assert_eq!(s.selected_bytes, 100);
        assert_eq!(counts(&s), "3 items · 1 hidden · 2 selected · 100 B");
    }

    #[test]
    fn one_item_is_singular_and_no_access_replaces_the_count() {
        let _guard = testing::lock();
        assert_eq!(items(1), "1 item");
        assert_eq!(items(12), "12 items");
        assert_eq!(drives(1), "1 drive");
        assert_eq!(drives(3), "3 drives");
        let mut tab = Tab::blank(TabId(2), "C:\\closed", ViewMode::List, None);
        tab.load = crate::explorer::LoadState::NoAccess(Default::default());
        let s = Summary::of(Some(&tab), 0);
        assert!(s.no_access);
        assert_eq!(counts(&s), "No access");
    }

    #[test]
    fn this_pc_and_no_tab_say_so_and_have_no_work_line() {
        let _guard = testing::lock();
        let tab = Tab::blank(TabId(1), glint_model::paths::THIS_PC, ViewMode::List, None);
        let s = Summary::of(Some(&tab), 2);
        assert!(s.this_pc);
        assert_eq!(Work::of(&s), Work::Nothing);
        assert!(Summary::of(None, 4).this_pc);
    }

    #[test]
    fn a_disk_folder_says_how_it_was_read_and_shown() {
        let _guard = testing::lock();
        let tab = tab_with(vec![file("C:\\a", "a.txt", 1, 1)]);
        let s = Summary::of(Some(&tab), 0);
        assert_eq!(s.read_ms, Some(1.5));
        assert_eq!(stages(&s)[0].label, "Folder read");
        assert_eq!(stages(&s)[0].detail, "from the drive");
        match Work::of(&s) {
            Work::Disk { lead, shown, title } => {
                assert_eq!(lead, "read in 1.5 ms ·");
                assert_eq!(shown, "shown in 0.0 ms");
                assert!(title.starts_with("Glint read this folder in 1.5 ms;"), "{title}");
            }
            other => panic!("expected a disk line, got {other:?}"),
        }
    }

    #[test]
    fn an_index_listing_and_an_archive_name_their_source() {
        let _guard = testing::lock();
        let mut tab = tab_with(vec![file("C:\\a", "a.txt", 1, 1)]);
        if let Some(listing) = tab.listing.as_mut() {
            listing.from_index = true;
        }
        let s = Summary::of(Some(&tab), 0);
        assert_eq!(stages(&s)[0].detail, "from the index");
        assert!(matches!(Work::of(&s), Work::Index { ref lead, .. } if lead == "index · listed in 1.5 ms ·"));

        if let Some(listing) = tab.listing.as_mut() {
            listing.from_index = false;
            listing.archive = Some(glint_model::entries::InArchive {
                path: "C:\\bag.zip".into(),
                outer: "C:\\bag.zip".into(),
                format: "zip",
                entries: 12,
                cached: false,
                read_ms: 3.0,
            });
        }
        let s = Summary::of(Some(&tab), 0);
        assert_eq!(stages(&s)[0].detail, "zip");
        match Work::of(&s) {
            Work::Archive { lead, title, .. } => {
                assert_eq!(lead, "zip · 12 items read in 1.5 ms ·");
                assert!(title.contains("bag.zip"), "{title}");
                assert!(title.contains("in 3.0 ms, without extracting"), "{title}");
            }
            other => panic!("expected an archive line, got {other:?}"),
        }
    }

    #[test]
    fn search_text_says_what_is_running_and_what_it_found() {
        let _guard = testing::lock();
        let running = FindInfo {
            engine: "index".into(),
            running: true,
            found: 12,
            ..Default::default()
        };
        assert_eq!(search_text(&running), "index · searching… 12 found");
        let done = FindInfo {
            engine: "index + fd".into(),
            fuzzy: true,
            found: 15,
            elapsed: 1234.0,
            limited: true,
            ..Default::default()
        };
        assert_eq!(
            search_text(&done),
            "index + fd · fuzzy · 15 found in 1.23 s · stopped at the limit"
        );
        let bare = FindInfo {
            found: 0,
            elapsed: 4.0,
            ..Default::default()
        };
        assert_eq!(search_text(&bare), "0 found in 4.0 ms");
    }

    #[test]
    fn segment_and_chip_titles_read_as_the_typescript_did() {
        let _guard = testing::lock();
        let folders = Category {
            group: FOLDERS,
            count: 3,
            bytes: 0,
            query: "folder:".into(),
        };
        assert_eq!(segment_title(&folders), "Folders: 3 (click to filter)");
        assert_eq!(chip_title(&folders), "Click to filter to folders");
        let other = Category {
            group: TYPE_GROUPS.other,
            count: 2,
            bytes: 2048,
            query: String::new(),
        };
        assert_eq!(segment_title(&other), "Other files: 2 (2.0 KB)");
        assert_eq!(chip_title(&other), "Other files");
        assert!(chip_active(&folders, " folder: "));
        assert!(!chip_active(&other, ""));
    }

    fn burst(found: usize, pending: bool, elapsed_ms: f64) -> Thumbs {
        Thumbs {
            found,
            pending,
            elapsed_ms,
            cache: 0,
            made: 0,
            made_ms: 0.0,
            memory: 0,
        }
    }

    #[test]
    fn thumbnail_words_count_the_burst_and_say_while_it_is_still_coming() {
        let _guard = testing::lock();
        assert_eq!(thumbs_line(&burst(12, false, 3.2)), "12 thumbnails in 3.2 ms");
        assert_eq!(thumbs_line(&burst(1, false, 3.2)), "1 thumbnail in 3.2 ms");
        assert_eq!(thumbs_line(&burst(5, true, 3.2)), "5 thumbnails…");
        assert_eq!(thumbs_line(&burst(1, true, 3.2)), "1 thumbnail…");
    }

    #[test]
    fn the_thumbnails_title_gives_the_rate_and_where_they_came_from() {
        let _guard = testing::lock();
        // 12 in 3 s is four a second; two made from their files took 40 ms, so 20 ms each.
        let done = Thumbs {
            found: 12,
            pending: false,
            elapsed_ms: 3000.0,
            cache: 10,
            made: 2,
            made_ms: 40.0,
            memory: 0,
        };
        let title = thumbs_title(&done);
        assert!(
            title.starts_with("Thumbnails of the items on screen, 3.00 s from the first request"),
            "{title}"
        );
        assert!(title.contains("(4 a second)"), "{title}");
        assert!(title.contains("10 from Windows' thumbnail cache"), "{title}");
        assert!(title.contains("2 made from their files (20"), "{title}");
        assert!(
            !title.contains("from memory"),
            "no memory origin, nothing said: {title}"
        );
        // Without any time passed there is no rate to say.
        let instant = Thumbs {
            elapsed_ms: 0.0,
            ..done
        };
        assert!(!thumbs_title(&instant).contains("a second"));
    }

    #[test]
    fn the_thumbnails_stage_waits_until_they_are_all_in() {
        let _guard = testing::lock();
        let mut done = burst(12, false, 3.2);
        done.cache = 10;
        let stage = thumbs_stage(&done).expect("done thumbnails have a stage");
        assert_eq!(stage.label, "Thumbnails (12)");
        assert_eq!(stage.detail, "10 cached");
        assert_eq!(stage.ms, 3.2);
        assert!(thumbs_stage(&burst(12, true, 3.2)).is_none());
    }
}
