//! What the list says when it has no rows to draw: loading, errors, folders Windows keeps closed, empty
//! folders (`empty` and `NoAccessView` in Explorer.tsx, worded as they are there).

use super::{FileList, FileListEvent, TabView};
use crate::explorer::{LoadState, TabId, Wider};
use crate::kit::prelude::*;
use crate::platform::pickers::{self, Options};
use crate::theme::Tokens;
use crate::{core_bridge, state, t, theme};
use glint_core::remote::smb::Trouble;
use glint_model::paths::{base_name, is_gallery, same_path, THIS_PC};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long what Windows said about keeping recent items is trusted before Recent asks again.
const RECENT_FRESH: Duration = Duration::from_secs(5);

/// Opens the folder picker: the dialog's title and the owner window in, the chosen folder out.
type PickFolder = Arc<dyn Fn(String, isize) -> pickers::Fut<Result<Option<String>, String>> + Send + Sync>;

/// What the empty states ask of the PC. Tests put their own in with `set_services`.
#[derive(Clone)]
struct Services {
    /// A folder the person picks (`None`: cancelled), given the dialog's title and the owner window.
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

use super::calm::{after_a_beat, centered, faded, title};

impl FileList {
    /// The message over the list when there is nothing to list, or None when the rows speak for themselves.
    pub(super) fn state_view(&self, view: &TabView, t: &Tokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(find) = &view.find {
            let (tab, query, scope, flat) = (view.id, find.query.clone(), find.scope.clone(), find.flat);
            if !find.error.is_empty() {
                return Some(
                    centered(t, "file-list-search-failed")
                        .role(Role::Alert)
                        .aria_label(t!("The search failed"))
                        .child(icon(IconName::TriangleAlert).size(rems(1.5)).text_color(t.destructive))
                        .child(title(t, t!("The search failed")))
                        // The engine's own words, as they are.
                        .child(
                            div()
                                .max_w_full()
                                .font_family(theme::mono_font(cx))
                                .text_size(theme::size::XS)
                                .child(find.error.clone()),
                        )
                        .child(
                            Button::new("file-list-search-try-again")
                                .outline()
                                .small()
                                .icon(IconName::RotateCw)
                                .label(t!("Try again"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.search_again(tab, &query, Wider { flat, ..Wider::in_scope(scope.clone()) }, cx)
                                })),
                        )
                        .test_support()
                        .into_any_element(),
                );
            }
            if find.running && view.rows.is_empty() {
                return Some(self.busy(t, t!("Searching…"), "file-list-searching", cx));
            }
            if view.rows.is_empty() {
                let mut hint = String::new();
                if !find.skipped.is_empty() {
                    hint.push_str(&t!("Skipped: {names}. ", names = find.skipped.join(", ")));
                }
                if !view.show_hidden {
                    hint.push_str(&t!("Hidden files weren't searched. "));
                }
                if !flat {
                    hint.push_str(&t!("Filters work too: ext:pdf, size:>10mb, dm:7d."));
                }
                // The files' contents were searched too: neither a name nor a line matched.
                let nothing = if flat {
                    t!("No files in this folder or its subfolders")
                } else if find.inside.words.is_empty() {
                    t!("Nothing named like “{query}”", query = find.query)
                } else {
                    t!("No name and no line matches “{query}”", query = find.query)
                };
                return Some(
                    centered(t, "file-list-nothing-found")
                        .role(Role::Status)
                        .aria_label(nothing.clone())
                        .child(faded(t, IconName::SearchX))
                        .child(title(t, nothing))
                        .child(div().max_w(rems(24.)).child(hint))
                        .child(
                            row_start()
                                .flex_wrap()
                                .justify_center()
                                .gap(space(2.))
                                .children((!view.show_hidden).then(|| {
                                    let scope = scope.clone();
                                    let query = query.clone();
                                    Button::new("file-list-search-hidden")
                                        .outline()
                                        .small()
                                        .icon(IconName::Eye)
                                        .label(t!("Include hidden files"))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            state::update(cx, |settings| settings.show_hidden = true);
                                            this.search_again(tab, &query, Wider { flat, ..Wider::in_scope(scope.clone()) }, cx)
                                        }))
                                }))
                                .children((!find.skipped.is_empty()).then(|| {
                                    let scope = scope.clone();
                                    let query = query.clone();
                                    Button::new("file-list-search-skipped")
                                        .outline()
                                        .small()
                                        .icon(IconName::FolderSearch)
                                        .label(t!("Search skipped folders too"))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let wider = Wider { flat, ..Wider::in_scope(scope.clone()) }.skipping_nothing();
                                            this.search_again(tab, &query, wider, cx)
                                        }))
                                }))
                                .children((scope != THIS_PC && !flat).then(|| {
                                    let query = query.clone();
                                    Button::new("file-list-search-every-drive")
                                        .outline()
                                        .small()
                                        .icon(IconName::HardDrive)
                                        .label(t!("Search every drive"))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.search_again(tab, &query, Wider::in_scope(THIS_PC), cx)
                                        }))
                                })),
                        )
                        .test_support()
                        .into_any_element(),
                );
            }
            return None;
        }
        if let Some(no_access) = view.no_access.filter(|_| !view.has_listing) {
            return Some(self.no_access_view(view, no_access, t, cx));
        }
        if view.has_listing {
            if !view.rows.is_empty() {
                return None;
            }
            return Some(self.empty_view(view, t, cx));
        }
        match &view.load {
            LoadState::Failed(message) => {
                // The tab keeps the folder it showed before, not the one that failed, so the heading can't name it.
                let heading = t!("Can't open this folder");
                Some(
                    centered(t, "file-list-failed")
                        .role(Role::Alert)
                        .aria_label(heading.clone())
                        .aria_description(message.clone())
                        .child(icon(IconName::TriangleAlert).size(rems(1.5)).text_color(t.destructive))
                        .child(title(t, heading))
                        .child(div().max_w(rems(24.)).child(message.clone()))
                        .test_support()
                        .into_any_element(),
                )
            }
            // Nothing listed yet: the thin bar at the top says a folder is being read.
            LoadState::Idle | LoadState::Loading | LoadState::NoAccess(_) | LoadState::Ready(_) => {
                if view.loading || matches!(view.load, LoadState::Loading) {
                    Some(self.busy(t, t!("Loading…"), "file-list-loading", cx))
                } else {
                    None
                }
            }
        }
    }

    /// A search again in `tab`, wider than Settings say if `wider` says so (`runFind(query, { scope, .. })`).
    fn search_again(&mut self, tab: TabId, query: &str, wider: Wider, cx: &mut Context<Self>) {
        self.explorer
            .update(cx, |explorer, cx| explorer.run_find_in(tab, query, wider, cx).detach());
    }

    /// "Add a folder" in the empty Gallery: a folder picked is added to Settings' gallery folders, and the Gallery
    /// is read again (`addGalleryFolder`).
    fn add_gallery_folder(&mut self, tab: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let owner = core_bridge::hwnd_of(window).map_or(0, |h| h.0 as isize);
        let picked = (services(cx).pick_folder)(t!("A folder for the gallery").to_string(), owner);
        let explorer = self.explorer.clone();
        cx.spawn(async move |_, cx| {
            let Ok(Some(folder)) = picked.await else {
                return;
            };
            cx.update(|cx| {
                state::update(cx, |settings| {
                    if !settings.gallery_folders.iter().any(|f| same_path(f, &folder)) {
                        settings.gallery_folders.push(folder);
                    }
                });
                explorer.update(cx, |explorer, cx| {
                    let here = explorer
                        .tab(tab)
                        .is_some_and(|t| is_gallery(&t.path) && t.find.is_none());
                    if here {
                        explorer.reload(Some(tab), cx).detach();
                    }
                });
            })
        })
        .detach();
    }

    /// Whether Windows is set not to keep recently opened items. Asked off the UI thread; until it answers (and
    /// while the answer is older than a few seconds) the last answer stands, "kept" at first.
    fn recent_is_off(&self, cx: &mut Context<Self>) -> bool {
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

    /// A spinner and a word (`Searching…`).
    fn busy(&self, t: &Tokens, text: SharedString, id: &'static str, cx: &App) -> AnyElement {
        // A folder that opens in a blink never shows it: it comes in after a beat, so there is no spinner flash
        // between two folders.
        let message = centered(t, id)
            .role(Role::Status)
            .aria_label(text.clone())
            .child(
                row_start()
                    .gap(space(2.))
                    .child(Spinner::new().color(t.primary))
                    .child(text),
            )
            .test_support();
        after_a_beat(id, message, cx.reduce_motion())
    }

    /// An empty folder, an empty Recycle Bin, a filter that matches nothing, a folder of only hidden items.
    fn empty_view(&self, view: &TabView, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        if !view.filter.is_empty() {
            let text = if view.recycled {
                t!("Nothing in the Recycle Bin matches “{filter}”.", filter = view.filter)
            } else {
                t!("Nothing in this folder matches “{filter}”.", filter = view.filter)
            };
            let (query, tab) = (view.filter.clone(), view.id);
            return centered(t, "file-list-no-match")
                .role(Role::Status)
                .aria_label(text.clone())
                .child(faded(t, IconName::SearchX))
                .child(title(t, text))
                .when(!view.recycled, |this| {
                    this.child(
                        Button::new("file-list-search-subfolders")
                            .outline()
                            .small()
                            .icon(IconName::Search)
                            .label(t!("Search subfolders too"))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.search_again(tab, &query, Wider::default(), cx)),
                            ),
                    )
                })
                .test_support()
                .into_any_element();
        }
        if view.recycled && view.entries == 0 {
            return centered(t, "file-list-bin-empty")
                .role(Role::Status)
                .aria_label(t!("The Recycle Bin is empty"))
                .child(faded(t, IconName::Trash))
                .child(title(t, t!("The Recycle Bin is empty")))
                .child(
                    div()
                        .text_size(theme::size::XS)
                        .child(t!("What you delete waits here, to be restored or deleted for good.")),
                )
                .test_support()
                .into_any_element();
        }
        let tab = view.id;
        if view.gallery && view.entries == 0 {
            return centered(t, "file-list-gallery-empty")
                .role(Role::Status)
                .aria_label(t!("No photos or videos here yet"))
                .child(faded(t, IconName::Images))
                .child(title(t, t!("No photos or videos here yet")))
                .child(div().max_w(rems(24.)).text_size(theme::size::XS).child(t!(
                    "The gallery shows what's in Pictures and Videos, and in the folders you add to it, with everything below them."
                )))
                .child(
                    Button::new("file-list-gallery-add-folder")
                        .outline()
                        .small()
                        .icon(IconName::FolderPlus)
                        .label(t!("Add a folder"))
                        .on_click(cx.listener(move |this, _, window, cx| this.add_gallery_folder(tab, window, cx))),
                )
                .test_support()
                .into_any_element();
        }
        if view.recent && view.entries == 0 {
            let off = self.recent_is_off(cx);
            let heading = if off {
                t!("Windows doesn't keep recently opened items")
            } else {
                t!("Nothing opened lately")
            };
            let note = if off {
                t!("It's set not to, in Settings → Personalization → Start: Show recently opened items in Start, Jump Lists and File Explorer. Turned on, the files you open in any app come here.")
            } else {
                t!("The files and folders you open, here or in any app, come here, newest first.")
            };
            return centered(t, "file-list-recent-empty")
                .role(Role::Status)
                .aria_label(heading.clone())
                .child(faded(t, IconName::Clock))
                .child(title(t, heading))
                .child(div().max_w(rems(24.)).text_size(theme::size::XS).child(note))
                .test_support()
                .into_any_element();
        }
        if let Some(same) = view
            .diffs
            .as_ref()
            .filter(|_| view.diff_only)
            .map(|diffs| diffs.same)
            .filter(|&n| n > 0)
        {
            // Compare panes with only the differences shown, and there are none.
            let heading = t!("No differences in this pane");
            let note = t!(
                "All {n} items match the other pane.",
                n = glint_model::format::format_count(same)
            );
            return centered(t, "file-list-no-differences")
                .role(Role::Status)
                .aria_label(heading.clone())
                .aria_description(note.clone())
                .child(icon(IconName::GitCompareArrows).size(rems(1.5)).text_color(t.success))
                .child(title(t, heading))
                .child(div().child(note))
                .child(
                    Button::new("file-list-show-identical")
                        .outline()
                        .small()
                        .label(t!("Show identical items"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.explorer.update(cx, |explorer, cx| explorer.toggle_diff_only(cx));
                        })),
                )
                .test_support()
                .into_any_element();
        }
        if view.entries > 0 {
            return centered(t, "file-list-only-hidden")
                .role(Role::Status)
                .aria_label(t!("This folder only has hidden items."))
                .child(faded(t, IconName::EyeOff))
                .child(title(t, t!("This folder only has hidden items.")))
                .child(
                    Button::new("file-list-show-hidden")
                        .outline()
                        .small()
                        .label(t!("Show hidden items"))
                        .on_click(cx.listener(|_, _, _, cx| {
                            state::update(cx, |settings| settings.show_hidden = true);
                        })),
                )
                .test_support()
                .into_any_element();
        }
        let text = if view.archive_root {
            t!("This archive is empty")
        } else {
            t!("This folder is empty")
        };
        centered(t, "file-list-empty")
            .role(Role::Status)
            .aria_label(text.clone())
            .child(
                div()
                    .opacity(0.9)
                    .child(crate::art::folder(px(72.), theme::folder_tone(cx), None, cx)),
            )
            .child(title(t, text))
            .when(!view.in_archive, |this| {
                this.child(
                    div()
                        .text_size(theme::size::XS)
                        .child(t!("Drop files here, or paste them with Ctrl+V.")),
                )
            })
            .test_support()
            .into_any_element()
    }

    /// A folder Windows keeps this account out of: why, and what can be done (`NoAccessView`).
    fn no_access_view(
        &self,
        view: &TabView,
        info: crate::explorer::NoAccess,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = view.path.clone();
        let name = {
            let base = base_name(&path);
            if base.is_empty() {
                path.clone()
            } else {
                base.to_string()
            }
        };
        let open_in_explorer = {
            let path = path.clone();
            Button::new("file-list-open-in-file-explorer")
                .outline()
                .small()
                .icon(IconName::FolderSearch)
                .label(t!("Open in File Explorer"))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(FileListEvent::OpenInFileExplorer { path: path.clone() });
                }))
        };
        let properties = {
            let path = path.clone();
            Button::new("file-list-properties")
                .outline()
                .small()
                .icon(IconName::Info)
                .label(t!("Properties"))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(FileListEvent::ShowProperties { path: path.clone() });
                }))
        };
        if let Some(trouble) = info.share {
            return self.share_view(view, trouble, t, cx);
        }
        let mut heading = t!("You don't have permission to open “{name}”", name = name);
        let text: SharedString;
        let mut actions: Vec<AnyElement> = Vec::new();
        if info.remote {
            text = t!(
                "It's on another computer, which decides who may open it. File Explorer can ask for a different user name and password."
            );
            actions.push(open_in_explorer.into_any_element());
        } else if info.refused {
            heading = t!("Windows keeps “{name}” to itself", name = name);
            text = t!(
                "Even administrators may not open it without taking it over, which can stop Windows, or the apps it holds, from working."
            );
            actions.push(properties.into_any_element());
        } else if info.still_denied {
            heading = t!("“{name}” still won't open", name = name);
            text = t!(
                "Your account may open it now, but a rule in its permissions keeps you out. Its Properties show who may open it, on the Security tab."
            );
            actions.push(properties.into_any_element());
        } else {
            text = t!(
                "Glint can give your account permission to open it and everything in it, as File Explorer's Continue button does. Windows asks for administrator approval."
            );
            actions.push(
                Button::new("file-list-get-access")
                    .small()
                    .icon(IconName::ShieldCheck)
                    .label(t!("Get access"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.explorer
                            .update(cx, |explorer, cx| explorer.get_access(None, cx).detach());
                    }))
                    .into_any_element(),
            );
            actions.push(open_in_explorer.into_any_element());
        }
        centered(t, "file-list-no-access")
            .role(Role::Group)
            .aria_label(heading.clone())
            .child(faded(t, IconName::FolderLock))
            .child(title(t, heading))
            .child(div().max_w(rems(24.)).child(text))
            .child(
                row_start()
                    .mt(space(1.))
                    .flex_wrap()
                    .justify_center()
                    .gap(space(2.))
                    .children(actions),
            )
            .test_support()
            .into_any_element()
    }

    /// A Windows share Windows won't open, for a reason Glint can help with (glint-core's remote/smb.rs): why, in
    /// words for this Windows, and what fits: signing in right here, opening it read-only through rclone (which
    /// leaves Windows' settings as they are), waking its server.
    fn share_view(&self, view: &TabView, trouble: Trouble, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let path = view.path.clone();
        let (server, share) = share_names(&path, cx);
        let tab = view.id;
        let sign_in = |label: SharedString, primary: bool, cx: &mut Context<Self>| {
            let path = path.clone();
            Button::new("file-list-share-sign-in")
                .small()
                .when(!primary, |b| b.outline())
                .icon(IconName::KeyRound)
                .label(label)
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(FileListEvent::SignInToShare { path: path.clone() });
                }))
                .into_any_element()
        };
        let guest = trouble == Trouble::GuestOnly;
        let read_only = |cx: &mut Context<Self>| {
            let path = path.clone();
            Button::new("file-list-share-read-only")
                .small()
                .icon(IconName::Eye)
                .label(t!("Open read-only"))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(FileListEvent::OpenShareReadOnly {
                        path: path.clone(),
                        guest,
                    });
                }))
                .into_any_element()
        };
        let open_in_explorer = |cx: &mut Context<Self>| {
            let path = path.clone();
            Button::new("file-list-open-in-file-explorer")
                .outline()
                .small()
                .icon(IconName::FolderSearch)
                .label(t!("Open in File Explorer"))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(FileListEvent::OpenInFileExplorer { path: path.clone() });
                }))
                .into_any_element()
        };
        let try_again = |cx: &mut Context<Self>| {
            Button::new("file-list-share-try-again")
                .outline()
                .small()
                .icon(IconName::RotateCw)
                .label(t!("Try again"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.explorer.update(cx, |e, cx| e.reload(Some(tab), cx).detach());
                }))
                .into_any_element()
        };
        let strict = glint_core::remote::smb::strict_windows();
        let (icon_name, heading, text, actions): (IconName, SharedString, SharedString, Vec<AnyElement>) = match trouble {
            Trouble::SignIn => (
                IconName::KeyRound,
                t!("Sign in to {server}", server = server),
                t!(
                    "{server} wants a user name and password for “{share}”. Glint keeps them in Windows Credential Manager and signs in by itself from then on.",
                    server = server,
                    share = share
                ),
                vec![sign_in(t!("Sign in…"), true, cx), open_in_explorer(cx)],
            ),
            Trouble::WrongPassword => (
                IconName::KeyRound,
                t!("{server} didn't take the password", server = server),
                t!(
                    "The user name and password Glint keeps for “{share}” didn't work. If they changed on {server}, type the new ones.",
                    server = server,
                    share = share
                ),
                vec![sign_in(t!("Sign in again…"), true, cx)],
            ),
            Trouble::NotAllowed => (
                IconName::FolderLock,
                t!("You don't have permission to open “{name}”", name = share),
                t!(
                    "{server} knows this account but doesn't let it in here. Another account may be.",
                    server = server
                ),
                vec![sign_in(t!("Sign in as someone else…"), true, cx), open_in_explorer(cx)],
            ),
            Trouble::Locked => (
                IconName::Lock,
                t!("{server} locked the account", server = server),
                t!(
                    "Too many wrong passwords. It unlocks after a while, or when someone unlocks it on {server}.",
                    server = server
                ),
                vec![sign_in(t!("Sign in…"), false, cx), try_again(cx)],
            ),
            Trouble::Disabled => (
                IconName::Lock,
                t!("The account is turned off on {server}", server = server),
                t!(
                    "Someone who looks after {server} can turn it back on, or you can sign in with another account.",
                    server = server
                ),
                vec![sign_in(t!("Sign in…"), true, cx)],
            ),
            Trouble::Expired => (
                IconName::Lock,
                t!("The account's password has expired"),
                t!("Set a new password on {server}, then sign in here with it.", server = server),
                vec![sign_in(t!("Sign in…"), true, cx)],
            ),
            Trouble::OtherAccount => (
                IconName::User,
                t!("Windows is signed in to {server} as someone else", server = server),
                t!(
                    "Windows lets this PC use one account per server, and File Explorer or a mapped drive already uses another one there. Glint can open “{share}” read-only on its own, or sign out there and try again.",
                    share = share
                ),
                vec![read_only(cx), try_again(cx)],
            ),
            Trouble::GuestOnly => (
                IconName::User,
                t!("Windows won't sign in to {server} as a guest", server = server),
                if strict {
                    t!(
                        "{server} lets in guests only, which Windows 11 24H2 and later refuse to keep this PC safe. Glint can open “{share}” read-only on its own, without changing Windows, or you can give {server} an account and sign in with it.",
                        server = server,
                        share = share
                    )
                } else {
                    t!(
                        "{server} lets in guests only, which this edition of Windows refuses to keep this PC safe. Glint can open “{share}” read-only on its own, without changing Windows, or you can give {server} an account and sign in with it.",
                        server = server,
                        share = share
                    )
                },
                vec![read_only(cx), sign_in(t!("Sign in with an account…"), false, cx)],
            ),
            Trouble::Unsigned => (
                IconName::ShieldCheck,
                t!("Windows won't sign in to {server}", server = server),
                t!(
                    "{server} doesn't sign what it sends, which Windows 11 24H2 and later require. Turn on SMB signing in {server}'s settings, or Glint can open “{share}” read-only on its own, without changing Windows.",
                    server = server,
                    share = share
                ),
                vec![read_only(cx), try_again(cx)],
            ),
            Trouble::Asleep { wakeable } => {
                let waking = Waking::since(&server, cx).is_some_and(|at| at.elapsed() < WAKING_FOR);
                let mut actions = Vec::new();
                if wakeable && waking {
                    actions.push(
                        Button::new("file-list-share-wake")
                            .small()
                            .loading(true)
                            .label(t!("Waking {server}…", server = server))
                            .into_any_element(),
                    );
                } else if wakeable {
                    let (path, woken) = (path.clone(), server.clone());
                    actions.push(
                        Button::new("file-list-share-wake")
                            .small()
                            .icon(IconName::Power)
                            .label(t!("Wake {server}", server = server))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                Waking::start(&woken, cx);
                                cx.emit(FileListEvent::WakeServer { path: path.clone() });
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
                actions.push(try_again(cx));
                (
                    IconName::Moon,
                    t!("{server} isn't answering", server = server),
                    t!(
                        "It may be asleep, switched off, or on another network. “{share}” opens as soon as {server} answers.",
                        server = server,
                        share = share
                    ),
                    actions,
                )
            }
        };
        centered(t, "file-list-share")
            .role(Role::Group)
            .aria_label(heading.clone())
            .aria_description(text.clone())
            .child(faded(t, icon_name))
            .child(title(t, heading))
            .child(div().max_w(rems(26.)).child(text))
            .child(
                row_start()
                    .mt(space(1.))
                    .flex_wrap()
                    .justify_center()
                    .gap(space(2.))
                    .children(actions),
            )
            .test_support()
            .into_any_element()
    }
}

/// The server and share a share's page names: from its path, or (opened through rclone, at `remote:<id>`) from its
/// connection's address.
fn share_names(path: &str, cx: &App) -> (String, String) {
    let address = match glint_model::paths::is_remote_path(path) {
        true => {
            let id = glint_model::paths::remote_id_of(path).to_lowercase();
            crate::persistence::app(cx)
                .and_then(|app| {
                    app.remotes
                        .iter()
                        .find(|r| r.id.to_lowercase() == id)
                        .map(|r| r.endpoint.clone())
                })
                .unwrap_or_else(|| path.to_string())
        }
        false => path.to_string(),
    };
    let server = glint_core::remote::smb::server_of(&address).to_string();
    let share = glint_core::remote::smb::share_of(&address)
        .map(|s| base_name(&s).to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| server.clone());
    (server, share)
}

/// How long "Waking…" shows after Wake was pressed: a NAS takes a minute or two to come up.
const WAKING_FOR: Duration = Duration::from_secs(120);

/// The servers Wake was pressed for, and when (lower case).
#[derive(Default)]
struct Waking(std::collections::HashMap<String, Instant>);

impl Global for Waking {}

impl Waking {
    fn since(server: &str, cx: &App) -> Option<Instant> {
        cx.try_global::<Waking>()
            .and_then(|w| w.0.get(&server.to_lowercase()).copied())
    }

    fn start(server: &str, cx: &mut App) {
        if !cx.has_global::<Waking>() {
            cx.set_global(Waking::default());
        }
        cx.global_mut::<Waking>()
            .0
            .insert(server.to_lowercase(), Instant::now());
    }
}
