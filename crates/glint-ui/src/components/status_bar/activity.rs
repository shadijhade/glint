//! The activity popover: what a click on "Changing now" opens. A compact list of the files being written in the
//! folder right now (name, where in the folder, the size as it grows, how long since the last write), newest
//! first, and the switch for Freeze order (`explorer/freeze.rs`). Card 2.B.8 of the 0.16 brief.
//!
//! **Built on what already knows the folder is changing.** Nothing here watches the disk: "Changing now" is
//! Aware's pulse of the folder watcher's notices, and the listing the explorer re-reads on each of them says
//! which files were written lately (their date is within the last seconds). The popover reads that listing while
//! it is open, and asks Windows for the few files' real size and date a few times a second, since a file being
//! written keeps its size in the folder's index until it is closed.
//!
//! Costs nothing while closed: no timer, no scan.

use super::StatusBar;
use crate::explorer::TabId;
use crate::kit::prelude::*;
use crate::{i18n, t, theme};
use glint_model::entries::FileEntry;
use glint_model::format::format_size;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// A file written this recently (ms) is being written now.
pub const WRITTEN_WITHIN: i64 = 12_000;
/// The most files the popover lists; the rest are counted.
pub const MOST: usize = 12;
/// How often the open popover looks again.
pub const EVERY: Duration = Duration::from_millis(400);

/// Asks Windows for a file's size and last write date (ms since the epoch), apart from the folder's listing.
pub type Stat = Arc<dyn Fn(&str) -> Option<(u64, i64)> + Send + Sync>;

/// The real thing: the file's own handle is asked, which is what keeps a size growing.
pub fn real_stat() -> Stat {
    Arc::new(|path| {
        let meta = std::fs::metadata(path).ok()?;
        let modified = meta.modified().ok()?;
        let ms = modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis();
        Some((meta.len(), i64::try_from(ms).unwrap_or(i64::MAX)))
    })
}

/// One file being written.
#[derive(Clone, Debug, PartialEq)]
pub struct Written {
    pub path: String,
    pub name: String,
    /// The folder it is in, from the folder shown ("" for a file right in it).
    pub inside: String,
    pub size: u64,
    /// Ms since the epoch.
    pub modified: i64,
    /// The size is bigger than it was at the last look.
    pub growing: bool,
}

/// Now, ms since the epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// The paths of the files of `entries` written within [`WRITTEN_WITHIN`] of `now`, newest first, and how many
/// there are in all.
pub fn candidates(entries: &[FileEntry], now: i64) -> (Vec<String>, usize) {
    let since = now.saturating_sub(WRITTEN_WITHIN);
    let mut recent: Vec<&FileEntry> = entries
        .iter()
        .filter(|entry| !entry.dir && entry.modified >= since)
        .collect();
    let total = recent.len();
    recent.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.lower.cmp(&b.lower)));
    recent.truncate(MOST);
    (recent.into_iter().map(|entry| entry.path.clone()).collect(), total)
}

/// Where `path` is inside `folder` ("" for a file right in it).
fn inside_of(path: &str, folder: &str) -> String {
    let rest = path
        .get(..folder.len())
        .filter(|head| head.eq_ignore_ascii_case(folder))
        .and_then(|_| path.get(folder.len()..))
        .unwrap_or(path)
        .trim_start_matches(['\\', '/']);
    match rest.rfind(['\\', '/']) {
        Some(at) => rest.get(..at).unwrap_or("").to_string(),
        None => String::new(),
    }
}

/// The popover's list: the `paths` found in `entries`, with the size and date Windows gave (`stats`) where it did,
/// newest first, and which of them grew since `before`.
pub fn written(
    entries: &[FileEntry],
    paths: &[String],
    stats: &HashMap<String, (u64, i64)>,
    before: &[Written],
    folder: &str,
) -> Vec<Written> {
    let by_path: HashMap<&str, &FileEntry> = entries.iter().map(|entry| (entry.path.as_str(), entry)).collect();
    let mut out: Vec<Written> = paths
        .iter()
        .filter_map(|path| {
            let entry = by_path.get(path.as_str())?;
            let (size, modified) = stats.get(path).copied().unwrap_or((entry.size, entry.modified));
            let growing = before
                .iter()
                .find(|old| old.path == *path)
                .is_some_and(|old| size > old.size);
            Some(Written {
                path: path.clone(),
                name: entry.name.clone(),
                inside: inside_of(path, folder),
                size,
                modified,
                growing,
            })
        })
        .collect();
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.name.cmp(&b.name)));
    out
}

/// How long ago `modified` was, in a few words.
pub fn ago(now: i64, modified: i64) -> SharedString {
    let seconds = now.saturating_sub(modified).max(0) / 1000;
    match seconds {
        0..=1 => t!("just now"),
        2..=59 => t!("{seconds} s ago", seconds = seconds),
        _ => t!("{minutes} min ago", minutes = seconds / 60),
    }
}

/// What the status bar keeps of the popover.
pub struct Feed {
    pub open: bool,
    pub items: Vec<Written>,
    /// Files written lately that don't fit in the list.
    pub more: usize,
    /// What Windows said of the listed files, at the last look.
    stats: HashMap<String, (u64, i64)>,
    stat: Stat,
    /// The panel is the focus, so Escape reaches it.
    pub focus: FocusHandle,
    /// The task that looks again while the popover is open.
    pub poll: Option<Task<()>>,
    /// Where the last press outside the popover landed (as the details popover keeps it).
    pub dismissed_at: Option<Point<Pixels>>,
}

impl Feed {
    pub fn new(cx: &mut Context<StatusBar>) -> Self {
        Feed {
            open: false,
            items: Vec::new(),
            more: 0,
            stats: HashMap::new(),
            stat: real_stat(),
            focus: cx.focus_handle(),
            poll: None,
            dismissed_at: None,
        }
    }

    /// Closes the popover and stops looking.
    pub fn close(&mut self) {
        self.open = false;
        self.poll = None;
        self.items.clear();
        self.stats.clear();
    }
}

impl StatusBar {
    /// Instead of Windows' own answer for each file (tests have no real files): `stat` is asked off the UI thread.
    #[cfg(test)]
    pub(super) fn stat_with(&mut self, stat: Stat) {
        self.feed.stat = stat;
    }

    /// The chip was pressed: opens the popover, or closes it. The press that closes it by landing outside it
    /// doesn't open it again.
    pub(super) fn toggle_activity(&mut self, at: Option<Point<Pixels>>, window: &mut Window, cx: &mut Context<Self>) {
        let outside = self.feed.dismissed_at.take();
        let closed_by_this_press = at.is_some() && outside == at;
        if self.feed.open {
            self.feed.close();
        } else if !closed_by_this_press {
            self.feed.open = true;
            self.details_open = false;
            self.look(cx);
            self.keep_looking(cx);
            window.focus(&self.feed.focus, cx);
        }
        cx.notify();
    }

    /// Reads the folder's listing for the files written lately and lists them. Notifies when the list changed.
    pub(super) fn look(&mut self, cx: &mut Context<Self>) {
        let now = now_ms();
        let (items, more) = {
            let explorer = self.explorer.read(cx);
            let Some(tab) = explorer.active_tab() else {
                return;
            };
            let (paths, total) = candidates(tab.entries(), now);
            self.feed.stats.retain(|path, _| paths.contains(path));
            let items = written(tab.entries(), &paths, &self.feed.stats, &self.feed.items, &tab.path);
            (items, total.saturating_sub(paths.len()))
        };
        if items != self.feed.items || more != self.feed.more {
            self.feed.items = items;
            self.feed.more = more;
        }
        cx.notify();
    }

    /// While the popover is open: a few times a second, asks Windows about the listed files and looks again.
    fn keep_looking(&mut self, cx: &mut Context<Self>) {
        self.feed.poll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(EVERY).await;
            let Ok(Some((paths, stat))) = this.update(cx, |this, _| {
                this.feed.open.then(|| {
                    (
                        this.feed.items.iter().map(|item| item.path.clone()).collect::<Vec<_>>(),
                        this.feed.stat.clone(),
                    )
                })
            }) else {
                break;
            };
            let found = cx
                .background_spawn(async move {
                    paths
                        .into_iter()
                        .filter_map(|path| stat(&path).map(|found| (path, found)))
                        .collect::<HashMap<_, _>>()
                })
                .await;
            if this
                .update(cx, |this, cx| {
                    if this.feed.open {
                        this.feed.stats.extend(found);
                        this.look(cx);
                    }
                })
                .is_err()
            {
                break;
            }
        }));
    }

    /// A file in the list was pressed: it is selected and shown in the list.
    fn reveal_written(&mut self, path: &str, cx: &mut Context<Self>) {
        self.explorer.update(cx, |explorer, cx| {
            explorer.select_only(Some(path), cx);
            explorer.flash_paths([path.to_string()], cx);
        });
        self.feed.close();
        cx.notify();
    }

    /// Freeze order, from the popover's switch.
    fn freeze_from_popover(&mut self, on: bool, tab: Option<TabId>, cx: &mut Context<Self>) {
        self.explorer
            .update(cx, |explorer, cx| explorer.set_frozen(tab, on, cx));
        cx.notify();
    }

    /// The popover: it opens above the bar at its end edge, as "What's in this folder" does.
    pub(super) fn activity_popover(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let now = now_ms();
        let tab = self.summary.tab;
        let (frozen, can) = {
            let explorer = self.explorer.read(cx);
            (
                tab.is_some_and(|tab| explorer.is_frozen(tab)),
                tab.is_some_and(|tab| explorer.can_freeze(Some(tab))),
            )
        };
        let count = self.feed.items.len() + self.feed.more;
        let counted: SharedString = if count == 1 {
            t!("1 file being written")
        } else {
            t!("{count} files being written", count = count)
        };
        let header = row_start()
            .justify_between()
            .items_center()
            .gap(space(2.))
            .child(
                row_start()
                    .items_center()
                    .gap(space(1.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.popover_foreground)
                    .child(icon(IconName::Radar).size_3p5().text_color(t.primary))
                    .child(t!("Changing now")),
            )
            .child(
                div()
                    .id("activity-count")
                    .role(Role::Status)
                    .aria_label(counted.clone())
                    .test_support()
                    .text_size(theme::size::XXS)
                    .text_color(t.muted_foreground)
                    .child(counted),
            );

        let rows: Vec<AnyElement> = self
            .feed
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| self.written_row(i, item, now, t, cx))
            .collect();
        let body = if rows.is_empty() {
            div()
                .id("activity-empty")
                .role(Role::Status)
                .aria_label(t!("Nothing is being written right now"))
                .test_support()
                .py(space(2.))
                .text_size(theme::size::XXS)
                .text_color(t.muted_foreground)
                .child(t!("Nothing is being written right now"))
                .into_any_element()
        } else {
            col()
                .id("activity-list")
                .role(Role::List)
                .aria_label(t!("Files being written"))
                .test_support()
                .gap(space(0.5))
                .children(rows)
                .into_any_element()
        };
        let more = (self.feed.more > 0).then(|| {
            div()
                .px(space(2.))
                .text_size(theme::size::XXS)
                .text_color(t.muted_foreground)
                .child(t!("and {count} more", count = self.feed.more))
        });

        let freeze = row_start()
            .items_center()
            .justify_between()
            .gap(space(3.))
            .pt(space(2.5))
            .border_t_1()
            .border_color(t.border)
            .child(
                col()
                    .min_w_0()
                    .gap(space(0.5))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.popover_foreground)
                            .child(t!("Freeze order")),
                    )
                    .child(
                        div()
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground)
                            .child(t!("Keep every file where it is while the folder changes")),
                    ),
            )
            .child(
                Switch::new("activity-freeze")
                    .checked(frozen)
                    .disabled(!can && !frozen)
                    .accessibility_label(t!("Freeze order"))
                    .on_click(cx.listener(move |this, on: &bool, _, cx| this.freeze_from_popover(*on, tab, cx))),
            );
        let recent = Button::new("activity-recent")
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(super::tint(t.primary, 0.12))
                    .foreground(t.primary)
                    .hover(super::tint(t.primary, 0.2))
                    .active(super::tint(t.primary, 0.2)),
            )
            .compact()
            .w_full()
            .accessibility_label(t!("Recent changes"))
            .on_click(cx.listener(|this, _, _, cx| {
                let pulse = crate::aware::try_shared(cx)
                    .and_then(|aware| aware.read(cx).pulse(&this.folder))
                    .unwrap_or([0; 30]);
                cx.emit(super::StatusBarEvent::Insight(super::Insight::Changing(pulse)));
                this.feed.close();
                cx.notify();
            }))
            .child(
                row_start()
                    .items_center()
                    .justify_center()
                    .gap(space(1.5))
                    .text_size(theme::size::XXS)
                    .font_weight(FontWeight::MEDIUM)
                    .child(icon(IconName::ScrollText).size_3())
                    .child(t!("Recent changes")),
            );

        col()
            .id("status-activity")
            .role(Role::Group)
            .aria_label(t!("Changing now"))
            .test_support()
            .track_focus(&self.feed.focus)
            .absolute()
            .bottom_full()
            .mb(space(2.))
            .w(space(80.))
            .p(space(3.))
            .gap(space(2.))
            .text_size(theme::size::XS)
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.border)
            .bg(t.popover)
            .text_color(t.popover_foreground)
            .shadow_lg()
            .occlude()
            .when(i18n::is_rtl(), |this| this.left_0())
            .when(!i18n::is_rtl(), |this| this.right_0())
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                this.feed.dismissed_at = Some(event.position);
                this.feed.close();
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.feed.close();
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(header)
            .child(body)
            .children(more)
            .child(freeze)
            .child(recent)
            .into_any_element()
    }

    /// One file in the list: its name and where it is, how big it is now, and how long ago it was written.
    fn written_row(&self, i: usize, item: &Written, now: i64, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let size = format_size(item.size);
        let when = ago(now, item.modified);
        let label = format!("{}: {size}, {when}", item.name);
        let path = item.path.clone();
        Button::new(("activity-file", i))
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(t.accent.opacity(0.))
                    .foreground(t.popover_foreground)
                    .hover(t.accent)
                    .active(t.accent),
            )
            .compact()
            .w_full()
            .tooltip(item.path.clone())
            .accessibility_label(label)
            .on_click(cx.listener(move |this, _, _, cx| this.reveal_written(&path, cx)))
            .child(
                row_start()
                    .w_full()
                    .items_center()
                    .gap(space(2.))
                    .text_size(theme::size::XXS)
                    .child(
                        row_start()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(space(1.5))
                            .child(div().min_w_0().truncate().child(isolate(item.name.as_str())))
                            .children((!item.inside.is_empty()).then(|| {
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(t.muted_foreground)
                                    .child(isolate(item.inside.as_str()))
                            })),
                    )
                    .children(
                        item.growing
                            .then(|| icon(IconName::TrendingUp).size_3().text_color(t.primary).flex_none()),
                    )
                    .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(size))
                    .child(
                        div()
                            .flex_none()
                            .w(space(16.))
                            .text_end()
                            .text_color(t.muted_foreground)
                            .child(when),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::{file, folder};

    #[test]
    fn only_files_written_lately_are_candidates_newest_first_and_counted_past_the_most() {
        let now = 1_000_000;
        let mut entries = vec![
            folder("C:\\x", "sub"),
            file("C:\\x", "old.txt", 1, now - WRITTEN_WITHIN - 1),
            file("C:\\x", "b.txt", 1, now - 2_000),
            file("C:\\x", "a.txt", 1, now - 2_000),
            file("C:\\x", "new.txt", 1, now - 10),
        ];
        let (paths, total) = candidates(&entries, now);
        assert_eq!(
            paths,
            ["C:\\x\\new.txt", "C:\\x\\a.txt", "C:\\x\\b.txt"],
            "the same moment goes by name"
        );
        assert_eq!(total, 3);
        for n in 0..20 {
            entries.push(file("C:\\x", &format!("z{n:02}.log"), 1, now - 500));
        }
        let (paths, total) = candidates(&entries, now);
        assert_eq!((paths.len(), total), (MOST, 23));
    }

    #[test]
    fn where_a_file_is_inside_the_folder_is_the_part_of_its_path_between_them() {
        assert_eq!(inside_of("C:\\x\\a.txt", "C:\\x"), "");
        assert_eq!(inside_of("c:\\X\\out\\deep\\a.txt", "C:\\x"), "out\\deep");
        assert_eq!(inside_of("D:\\elsewhere\\a.txt", "C:\\x"), "D:\\elsewhere");
    }

    #[test]
    fn windows_answer_wins_over_the_listing_and_a_bigger_size_is_growth() {
        let entries = vec![file("C:\\x", "a.log", 100, 5), file("C:\\x", "تقرير.txt", 50, 9)];
        let paths: Vec<String> = entries.iter().map(|entry| entry.path.clone()).collect();
        let first = written(&entries, &paths, &HashMap::new(), &[], "C:\\x");
        assert_eq!(
            first.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(),
            ["تقرير.txt", "a.log"]
        );
        assert!(first.iter().all(|w| !w.growing));
        let stats = HashMap::from([("C:\\x\\a.log".to_string(), (400, 20))]);
        let next = written(&entries, &paths, &stats, &first, "C:\\x");
        assert_eq!(next[0].name, "a.log", "its date is newer now");
        assert_eq!((next[0].size, next[0].modified, next[0].growing), (400, 20, true));
        assert!(!next[1].growing);
    }

    #[test]
    fn how_long_ago_is_said_in_seconds_then_minutes() {
        let _guard = crate::testing::lock();
        assert_eq!(ago(10_000, 9_500), "just now");
        assert_eq!(ago(10_000, 5_000), "5 s ago");
        assert_eq!(ago(200_000, 20_000), "3 min ago");
        assert_eq!(
            ago(1_000, 5_000),
            "just now",
            "a clock a little behind is not the future"
        );
    }
}
