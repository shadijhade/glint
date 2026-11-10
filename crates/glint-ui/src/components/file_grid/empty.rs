//! What the grid says when a folder shows no tiles (the `empty` node Explorer.tsx builds for both views): a plain
//! folder, the filter, the Recycle Bin, a search (running, failed, or finding nothing), the Gallery and Recent.
//!
//! The list words the same states in `file_list/states.rs`; its views are private to it, so these are the grid's own
//! copies, worded and ordered the same.

use super::FileGrid;
use crate::explorer::{TabId, Wider};
use crate::kit::prelude::*;
use crate::platform::pickers::{self, Options};
use crate::{core_bridge, state, t, theme};
use glint_model::paths::{same_path, THIS_PC};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long what Windows said about keeping recent items is trusted before Recent asks again.
const RECENT_FRESH: Duration = Duration::from_secs(5);

/// Opens the folder picker: the dialog's title and the owner window in, the chosen folder out.
type PickFolder = Arc<dyn Fn(String, isize) -> pickers::Fut<Result<Option<String>, String>> + Send + Sync>;

/// What the empty states ask of the PC. Tests put their own in with `set_services`.
#[derive(Clone)]
struct Services {
    /// A folder the person picks (`None`: cancelled).
    pick_folder: PickFolder,
    /// Whether Windows keeps recently opened items (`recentKept`). Blocks (the registry), so it runs off the UI thread.
    recent_kept: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl Default for Services {
    fn default() -> Self {
        Services {
            pick_folder: Arc::new(|title, owner| pickers::open_folder(Options::new().title(title).owner(owner))),
            recent_kept: Arc::new(glint_core::recent::recent_kept),
        }
    }
}

impl Global for Services {}

/// Instead of the PC's own: the folder picker and the answer about Recent (tests).
#[cfg(test)]
pub(super) fn set_services(
    cx: &mut App,
    pick_folder: impl Fn(String, isize) -> pickers::Fut<Result<Option<String>, String>> + Send + Sync + 'static,
    recent_kept: impl Fn() -> bool + Send + Sync + 'static,
) {
    cx.set_global(Services {
        pick_folder: Arc::new(pick_folder),
        recent_kept: Arc::new(recent_kept),
    });
    cx.set_global(RecentKnown::default());
}

fn services(cx: &App) -> Services {
    cx.try_global::<Services>().cloned().unwrap_or_default()
}

/// What Windows last said about keeping recent items, and whether the question is out.
#[derive(Default)]
struct RecentKnown {
    kept: Option<(bool, Instant)>,
    asking: bool,
}

impl Global for RecentKnown {}

/// A special place the tab shows, which has a message of its own when it has nothing in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Place {
    Plain,
    Gallery,
    Recent,
}

/// A search's run, as far as the grid shows it (`find` in Explorer.tsx).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Search {
    pub query: String,
    pub scope: String,
    pub running: bool,
    pub error: String,
    pub skipped: Vec<String>,
}

/// Why there is nothing to show.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Empty {
    /// The search failed: the engine's own words, and a button to try again.
    SearchFailed {
        query: String,
        scope: String,
        error: String,
    },
    /// A search is under way and has found nothing yet.
    Searching,
    /// The search found nothing: what it left out, and a button for each way to look wider.
    NothingFound {
        query: String,
        scope: String,
        skipped: Vec<String>,
        show_hidden: bool,
    },
    /// The Gallery has no photos or videos.
    Gallery,
    /// Recent has nothing; `off` when Windows is set not to keep recently opened items.
    Recent { off: bool },
    /// The search box's text matches nothing here.
    NoMatch { filter: String, bin: bool },
    /// The Recycle Bin has nothing in it.
    BinHasNothing,
    /// Compare panes shows only the differences, and there are none: all `same` items match the other pane.
    NoDifferences { same: usize },
    /// Everything here is hidden, and hidden items aren't shown.
    OnlyHidden,
    /// Nothing in the folder (or archive).
    Folder { archive: bool },
}

impl Empty {
    /// What a search says when it has nothing to draw, or `None` when its tiles speak for themselves. A failure
    /// speaks over the tiles; the others only when there are none (`find?.error`, `find?.running && !rows.length`).
    pub fn of_search(search: &Search, rows_empty: bool, show_hidden: bool) -> Option<Empty> {
        if !search.error.is_empty() {
            Some(Empty::SearchFailed {
                query: search.query.clone(),
                scope: search.scope.clone(),
                error: search.error.clone(),
            })
        } else if !rows_empty {
            None
        } else if search.running {
            Some(Empty::Searching)
        } else {
            Some(Empty::NothingFound {
                query: search.query.clone(),
                scope: search.scope.clone(),
                skipped: search.skipped.clone(),
                show_hidden,
            })
        }
    }

    /// The state for a listing that made no rows, from what the tab shows. `listed` is how many entries the
    /// listing has before hiding and filtering.
    pub fn of(filter: &str, bin: bool, listed: usize, archive: bool, diff_same: usize, place: Place) -> Empty {
        if !filter.is_empty() {
            Empty::NoMatch {
                filter: filter.to_string(),
                bin,
            }
        } else if bin && listed == 0 {
            Empty::BinHasNothing
        } else if place == Place::Gallery && listed == 0 {
            Empty::Gallery
        } else if place == Place::Recent && listed == 0 {
            Empty::Recent { off: false }
        } else if diff_same > 0 {
            Empty::NoDifferences { same: diff_same }
        } else if listed > 0 {
            Empty::OnlyHidden
        } else {
            Empty::Folder { archive }
        }
    }
}

/// The centred message (`Centered` in Explorer.tsx). `tab` is the grid's own tab: Search subfolders too searches it,
/// not whichever pane is active.
pub(super) fn view(empty: &Empty, tab: TabId, cx: &App) -> impl IntoElement {
    let t = theme::tokens(cx);
    let strong = |text: SharedString| {
        div()
            .font_weight(FontWeight::MEDIUM)
            .text_color(t.foreground)
            .child(text)
    };
    let note = |text: SharedString| div().text_size(theme::size::XS).child(text);
    let centered = col()
        .id("file-grid-empty")
        .role(Role::Status)
        .size_full()
        .items_center()
        .justify_center()
        .gap(space(2.))
        .p(space(6.))
        .text_center()
        .text_size(theme::size::SM)
        .text_color(t.muted_foreground)
        // A failed search speaks over tiles it found before it failed.
        .bg(t.background);
    match empty {
        Empty::SearchFailed { query, scope, error } => {
            let heading = t!("The search failed");
            let (query, scope) = (query.clone(), scope.clone());
            centered
                .role(Role::Alert)
                .aria_label(heading.clone())
                .aria_description(error.clone())
                .test_support()
                .child(icon(IconName::TriangleAlert).size_6().text_color(t.destructive))
                .child(strong(heading))
                // The engine's own words, as they are.
                .child(div().max_w_full().text_size(theme::size::XS).child(error.clone()))
                .child(
                    Button::new("file-grid-search-try-again")
                        .outline()
                        .small()
                        .icon(IconName::RotateCw)
                        .label(t!("Try again"))
                        .on_click(move |_, _, cx| search_again(tab, &query, Wider::in_scope(scope.clone()), cx)),
                )
        }
        Empty::Searching => centered.aria_label(t!("Searching…")).test_support().child(
            row_start()
                .gap(space(2.))
                .child(Spinner::new().color(t.primary))
                .child(t!("Searching…")),
        ),
        Empty::NothingFound {
            query,
            scope,
            skipped,
            show_hidden,
        } => {
            let heading = t!("Nothing named like “{query}”", query = query.clone());
            let mut hint = String::new();
            if !skipped.is_empty() {
                hint.push_str(&t!("Skipped: {names}. ", names = skipped.join(", ")));
            }
            if !*show_hidden {
                hint.push_str(&t!("Hidden files weren't searched. "));
            }
            hint.push_str(&t!("Filters work too: ext:pdf, size:>10mb, dm:7d."));
            let hidden = (!*show_hidden).then(|| {
                let (scope, query) = (scope.clone(), query.clone());
                Button::new("file-grid-search-hidden")
                    .outline()
                    .small()
                    .icon(IconName::Eye)
                    .label(t!("Include hidden files"))
                    .on_click(move |_, _, cx| {
                        state::update(cx, |settings| settings.show_hidden = true);
                        search_again(tab, &query, Wider::in_scope(scope.clone()), cx)
                    })
            });
            let skipping = (!skipped.is_empty()).then(|| {
                let (scope, query) = (scope.clone(), query.clone());
                Button::new("file-grid-search-skipped")
                    .outline()
                    .small()
                    .icon(IconName::FolderSearch)
                    .label(t!("Search skipped folders too"))
                    .on_click(move |_, _, cx| {
                        search_again(tab, &query, Wider::in_scope(scope.clone()).skipping_nothing(), cx)
                    })
            });
            let every_drive = (scope != THIS_PC).then(|| {
                let query = query.clone();
                Button::new("file-grid-search-every-drive")
                    .outline()
                    .small()
                    .icon(IconName::HardDrive)
                    .label(t!("Search every drive"))
                    .on_click(move |_, _, cx| search_again(tab, &query, Wider::in_scope(THIS_PC), cx))
            });
            centered
                .aria_label(heading.clone())
                .test_support()
                .child(icon(IconName::SearchX).size_6())
                .child(strong(heading))
                .child(div().max_w(rems(24.)).child(hint))
                .child(
                    row_start()
                        .flex_wrap()
                        .justify_center()
                        .gap(space(2.))
                        .children(hidden)
                        .children(skipping)
                        .children(every_drive),
                )
        }
        Empty::Gallery => {
            let heading = t!("No photos or videos here yet");
            centered
                .aria_label(heading.clone())
                .test_support()
                .child(
                    icon(IconName::Images)
                        .size_8()
                        .text_color(t.muted_foreground.opacity(0.6)),
                )
                .child(strong(heading))
                .child(div().max_w(rems(24.)).text_size(theme::size::XS).child(t!(
                    "The gallery shows what's in Pictures and Videos, and in the folders you add to it, with everything below them."
                )))
                .child(
                    Button::new("file-grid-gallery-add-folder")
                        .outline()
                        .small()
                        .icon(IconName::FolderPlus)
                        .label(t!("Add a folder"))
                        .on_click(move |_, window, cx| add_gallery_folder(tab, window, cx)),
                )
        }
        Empty::Recent { off } => {
            let heading = if *off {
                t!("Windows doesn't keep recently opened items")
            } else {
                t!("Nothing opened lately")
            };
            let note = if *off {
                t!("It's set not to, in Settings → Personalization → Start: Show recently opened items in Start, Jump Lists and File Explorer. Turned on, the files you open in any app come here.")
            } else {
                t!("The files and folders you open, here or in any app, come here, newest first.")
            };
            centered
                .aria_label(heading.clone())
                .test_support()
                .child(
                    icon(IconName::Clock)
                        .size_8()
                        .text_color(t.muted_foreground.opacity(0.6)),
                )
                .child(strong(heading))
                .child(div().max_w(rems(24.)).text_size(theme::size::XS).child(note))
        }
        Empty::NoMatch { filter, bin } => {
            let place = if *bin { t!("the Recycle Bin") } else { t!("this folder") };
            centered
                .aria_label(t!(
                    "Nothing in {place} matches “{filter}”.",
                    place = place,
                    filter = filter
                ))
                .test_support()
                .child(icon(IconName::SearchX).size_6())
                .child(div().child(t!(
                    "Nothing in {place} matches “{filter}”.",
                    place = place,
                    filter = filter
                )))
                .when(!*bin, |centered| {
                    let query = filter.clone();
                    centered.child(
                        Button::new("file-grid-search-subfolders")
                            .outline()
                            .small()
                            .icon(IconName::Search)
                            .label(t!("Search subfolders too"))
                            .on_click(move |_, _, cx| {
                                if let Some(explorer) = crate::explorer::try_explorer(cx) {
                                    explorer.update(cx, |explorer, cx| {
                                        explorer
                                            .run_find_in(tab, &query, crate::explorer::Wider::default(), cx)
                                            .detach()
                                    });
                                }
                            }),
                    )
                })
        }
        Empty::BinHasNothing => centered
            .aria_label(t!("The Recycle Bin is empty"))
            .test_support()
            .child(
                icon(IconName::Trash)
                    .size_8()
                    .text_color(t.muted_foreground.opacity(0.6)),
            )
            .child(strong(t!("The Recycle Bin is empty")))
            .child(note(t!(
                "What you delete waits here, to be restored or deleted for good."
            ))),
        Empty::NoDifferences { same } => {
            let heading = t!("No differences in this pane");
            let line = t!(
                "All {n} items match the other pane.",
                n = glint_model::format::format_count(*same)
            );
            centered
                .aria_label(heading.clone())
                .aria_description(line.clone())
                .test_support()
                .child(icon(IconName::GitCompareArrows).size_6().text_color(t.success))
                .child(strong(heading))
                .child(div().child(line))
                .child(
                    Button::new("file-grid-show-identical")
                        .outline()
                        .small()
                        .label(t!("Show identical items"))
                        .on_click(|_, _, cx| {
                            if let Some(explorer) = crate::explorer::try_explorer(cx) {
                                explorer.update(cx, |explorer, cx| explorer.toggle_diff_only(cx));
                            }
                        }),
                )
        }
        Empty::OnlyHidden => centered
            .aria_label(t!("This folder only has hidden items."))
            .test_support()
            .child(div().child(t!("This folder only has hidden items.")))
            .child(
                Button::new("file-grid-show-hidden")
                    .outline()
                    .small()
                    .label(t!("Show hidden items"))
                    .on_click(|_, _, cx| state::update(cx, |settings| settings.show_hidden = true)),
            ),
        Empty::Folder { archive } => {
            let title = if *archive {
                t!("This archive is empty")
            } else {
                t!("This folder is empty")
            };
            centered
                .aria_label(title.clone())
                .test_support()
                .child(
                    icon(IconName::FolderOpen)
                        .size_8()
                        .text_color(t.muted_foreground.opacity(0.6)),
                )
                .child(strong(title))
                .when(!archive, |centered| {
                    centered.child(note(t!("Drop files here, or paste them with Ctrl+V.")))
                })
        }
    }
}

/// A search again in `tab`, wider than Settings say if `wider` says so (`runFind(query, { scope, .. })`).
fn search_again(tab: TabId, query: &str, wider: Wider, cx: &mut App) {
    if let Some(explorer) = crate::explorer::try_explorer(cx) {
        explorer.update(cx, |explorer, cx| explorer.run_find_in(tab, query, wider, cx).detach());
    }
}

/// "Add a folder" in the empty Gallery: a folder picked is added to Settings' gallery folders, and the Gallery is
/// read again (`addGalleryFolder`).
fn add_gallery_folder(tab: TabId, window: &mut Window, cx: &mut App) {
    let owner = core_bridge::hwnd_of(window).map_or(0, |h| h.0 as isize);
    let picked = (services(cx).pick_folder)(t!("A folder for the gallery").to_string(), owner);
    cx.spawn(async move |cx| {
        let Ok(Some(folder)) = picked.await else {
            return;
        };
        cx.update(|cx| {
            state::update(cx, |settings| {
                if !settings.gallery_folders.iter().any(|f| same_path(f, &folder)) {
                    settings.gallery_folders.push(folder);
                }
            });
            if let Some(explorer) = crate::explorer::try_explorer(cx) {
                explorer.update(cx, |explorer, cx| {
                    let here = explorer
                        .tab(tab)
                        .is_some_and(|t| glint_model::paths::is_gallery(&t.path) && t.find.is_none());
                    if here {
                        explorer.reload(Some(tab), cx).detach();
                    }
                });
            }
        })
    })
    .detach();
}

impl FileGrid {
    /// Whether Windows is set not to keep recently opened items. Asked off the UI thread; until it answers (and
    /// while the answer is older than a few seconds) the last answer stands, "kept" at first.
    pub(super) fn recent_is_off(&self, cx: &mut Context<Self>) -> bool {
        if !cx.has_global::<RecentKnown>() {
            cx.set_global(RecentKnown::default());
        }
        let known = cx.global::<RecentKnown>();
        let off = known.kept.is_some_and(|(kept, _)| !kept);
        let stale = known.kept.is_none_or(|(_, at)| at.elapsed() > RECENT_FRESH);
        if stale && !known.asking {
            cx.global_mut::<RecentKnown>().asking = true;
            let ask = services(cx).recent_kept;
            cx.spawn(async move |this, cx| {
                let kept = cx.background_spawn(async move { ask() }).await;
                this.update(cx, |_, cx| {
                    let known = cx.global_mut::<RecentKnown>();
                    let changed = known.kept.is_none_or(|(was, _)| was != kept);
                    known.kept = Some((kept, Instant::now()));
                    known.asking = false;
                    if changed {
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        off
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_follows_what_the_tab_shows() {
        assert_eq!(
            Empty::of("zz", false, 5, false, 0, Place::Plain),
            Empty::NoMatch {
                filter: "zz".into(),
                bin: false
            }
        );
        assert_eq!(Empty::of("", true, 0, false, 0, Place::Plain), Empty::BinHasNothing);
        assert_eq!(Empty::of("", false, 3, false, 0, Place::Plain), Empty::OnlyHidden);
        assert_eq!(
            Empty::of("", false, 0, true, 0, Place::Plain),
            Empty::Folder { archive: true }
        );
        // A bin with things in it whose rows are all hidden or filtered isn't "empty".
        assert_eq!(Empty::of("", true, 2, false, 0, Place::Plain), Empty::OnlyHidden);
        // Only the differences shown, and none: the identical ones are what the folder has (hidden ones too).
        assert_eq!(
            Empty::of("", false, 4, false, 4, Place::Plain),
            Empty::NoDifferences { same: 4 }
        );
        assert_eq!(
            Empty::of("zz", false, 4, false, 4, Place::Plain),
            Empty::NoMatch {
                filter: "zz".into(),
                bin: false
            }
        );
        // The Gallery and Recent have messages of their own when nothing is listed; a listing with only hidden
        // items is something else.
        assert_eq!(Empty::of("", false, 0, false, 0, Place::Gallery), Empty::Gallery);
        assert_eq!(
            Empty::of("", false, 0, false, 0, Place::Recent),
            Empty::Recent { off: false }
        );
        assert_eq!(Empty::of("", false, 2, false, 0, Place::Gallery), Empty::OnlyHidden);
    }

    fn search(running: bool, error: &str) -> Search {
        Search {
            query: "invoice".into(),
            scope: "C:\\Work".into(),
            running,
            error: error.into(),
            skipped: vec!["node_modules".into()],
        }
    }

    #[test]
    fn a_search_speaks_when_it_failed_is_running_or_found_nothing() {
        // A failure speaks even over tiles; the rest only when there are none.
        assert!(matches!(
            Empty::of_search(&search(false, "fd is missing"), false, false),
            Some(Empty::SearchFailed { .. })
        ));
        assert_eq!(Empty::of_search(&search(true, ""), true, false), Some(Empty::Searching));
        assert_eq!(Empty::of_search(&search(true, ""), false, false), None);
        assert_eq!(Empty::of_search(&search(false, ""), false, false), None);
        assert_eq!(
            Empty::of_search(&search(false, ""), true, true),
            Some(Empty::NothingFound {
                query: "invoice".into(),
                scope: "C:\\Work".into(),
                skipped: vec!["node_modules".into()],
                show_hidden: true
            })
        );
    }
}

#[cfg(test)]
#[path = "empty_tests.rs"]
mod empty_tests;
