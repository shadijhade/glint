//! Settings → About → Settings file: Export and Import, Glint's set-up in a file to bring to another PC, never a
//! secret (`SettingsFileRows` of SettingsView.tsx). The file's format, what each part holds and how it is taken
//! are `persistence`'s (`export_settings`, `read_settings_file`, `import_settings`); this is the choosing.

use super::backend::Backend;
use super::widgets::{self, bad, good};
use crate::kit::prelude::*;
use crate::persistence::{self, Part, SettingsFile, ALL_PARTS};
use crate::{core_bridge, t, theme};
use glint_model::format::{format_count, format_date};
use glint_model::paths::base_name;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

/// A part's name, as the choice lists it (`PARTS`' labels).
pub fn part_label(part: Part) -> SharedString {
    match part {
        Part::Settings => t!("Settings"),
        Part::Pinned => t!("Pinned folders"),
        Part::SavedSearches => t!("Saved searches"),
        Part::Remotes => t!("Remote connections"),
        Part::Rules => t!("Folder rules"),
        Part::History => t!("Search history"),
        Part::Visits => t!("Folders Ctrl+K ranks"),
        Part::FolderViews => t!("Folders' own views"),
    }
}

/// What is to know about a part (`PARTS`' notes).
pub fn part_note(part: Part) -> Option<SharedString> {
    match part {
        Part::Settings => Some(t!("every option here")),
        Part::Remotes => Some(t!(
            "without their passwords and keys; a cloud only by its name in rclone"
        )),
        Part::Rules => Some(t!("they come turned off")),
        Part::FolderViews => Some(t!("their columns, sort and list or grid")),
        Part::Pinned | Part::SavedSearches | Part::History | Part::Visits => None,
    }
}

/// A part's slot name for element ids.
fn slot(part: Part) -> &'static str {
    match part {
        Part::Settings => "settings",
        Part::Pinned => "pinned",
        Part::SavedSearches => "saved-searches",
        Part::Remotes => "remotes",
        Part::Rules => "rules",
        Part::History => "history",
        Part::Visits => "visits",
        Part::FolderViews => "folder-views",
    }
}

/// Milliseconds since 1970 of an ISO date like `2026-11-10T12:00:00.000Z` (`Date.parse`), or `None`.
pub fn parse_iso_ms(text: &str) -> Option<i64> {
    let text = text.trim();
    let (date, time) = text.split_once('T').unwrap_or((text, "00:00:00"));
    let mut d = date.split('-');
    let (year, month, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let time = time.trim_end_matches('Z');
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let mut c = clock.split(':');
    let (h, m, s): (i64, i64, i64) = (
        c.next()?.parse().ok()?,
        c.next().unwrap_or("0").parse().ok()?,
        c.next().unwrap_or("0").parse().ok()?,
    );
    let millis: i64 = format!("{fraction:0<3}")[..3].parse().ok()?;
    // Howard Hinnant's days-from-civil.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + m) * 60_000 + s * 1000 + millis)
}

/// A file chosen for Import, and the parts ticked.
struct Incoming {
    path: String,
    file: SettingsFile,
    parts: HashSet<Part>,
}

/// The Settings file row, and the choice of what to take from a file just read.
pub struct Transfer {
    backend: Arc<dyn Backend>,
    incoming: Option<Incoming>,
}

impl Transfer {
    /// A row with nothing chosen yet.
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        Self {
            backend,
            incoming: None,
        }
    }

    /// The parts ticked in the file offered, in order.
    pub fn ticked(&self) -> Vec<Part> {
        match &self.incoming {
            Some(incoming) => ALL_PARTS.into_iter().filter(|p| incoming.parts.contains(p)).collect(),
            None => Vec::new(),
        }
    }

    /// Whether a file is on offer.
    pub fn offering(&self) -> bool {
        self.incoming.is_some()
    }

    /// Asks where to write, then writes everything that is set up (`exportFile`).
    pub fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = core_bridge::hwnd_of(window).map_or(0, |h| h.0 as isize);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_millis()).unwrap_or(0))
            .unwrap_or(0);
        let day = persistence::iso_date(now).chars().take(10).collect::<String>();
        let picked = self
            .backend
            .save_settings_file(format!("Glint settings {day}.json"), owner);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Some(path)) = picked.await else { return };
            let Ok(state) = this.update(cx, |_, cx| persistence::app(cx).cloned()) else {
                return;
            };
            let Some(state) = state else { return };
            let target = path.clone();
            let written = cx
                .background_spawn(async move {
                    persistence::write_settings_file(&PathBuf::from(&target), &state, &ALL_PARTS)
                })
                .await;
            this.update_in(cx, |_, window, cx| match written {
                Ok(()) => good(window, cx, t!("Exported Glint's settings"), Some(path.into())),
                Err(reason) => bad(window, cx, t!("Couldn't export the settings"), reason),
            })
            .ok();
        })
        .detach();
    }

    /// Asks which file to read, reads it, and offers what is in it (`importFile`).
    pub fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = core_bridge::hwnd_of(window).map_or(0, |h| h.0 as isize);
        let picked = self.backend.pick_settings_file(owner);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Some(path)) = picked.await else { return };
            let target = path.clone();
            let read = cx
                .background_spawn(async move { persistence::read_settings_file(&PathBuf::from(&target)) })
                .await;
            this.update_in(cx, |this, window, cx| {
                match read {
                    Ok(file) => this.offer(path, file),
                    Err(reason) => bad(window, cx, t!("Couldn't import it"), reason),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Offers a file read: everything in it is ticked except the history and the folders Ctrl+K ranks.
    pub fn offer(&mut self, path: String, file: SettingsFile) {
        let parts = ALL_PARTS
            .into_iter()
            .filter(|p| file.count_of(*p) > 0 && !matches!(p, Part::History | Part::Visits))
            .collect();
        self.incoming = Some(Incoming { path, file, parts });
    }

    /// Takes the ticked parts: added to what is here, or replacing it (`take`).
    pub fn take(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(incoming) = self.incoming.take() else { return };
        let parts: Vec<Part> = ALL_PARTS.into_iter().filter(|p| incoming.parts.contains(p)).collect();
        let Some(mut working) = persistence::app(cx).cloned() else {
            return;
        };
        let did = persistence::import_settings(&mut working, &incoming.file, &parts, replace);
        // The live settings are the master copy (`state::update` keeps the saved state's in step), so the
        // imported ones go in through it; the rest of the saved state is replaced as imported.
        let settings = working.settings.clone();
        persistence::update_app(cx, move |app| *app = working);
        crate::state::update(cx, move |current| *current = settings);
        good(
            window,
            cx,
            if replace {
                t!("Replaced with the file's")
            } else {
                t!("Added from the file")
            },
            Some(did.join(", ").into()),
        );
        cx.notify();
    }

    fn toggle(&mut self, part: Part, on: bool, cx: &mut Context<Self>) {
        if let Some(incoming) = &mut self.incoming {
            if on {
                incoming.parts.insert(part);
            } else {
                incoming.parts.remove(&part);
            }
            cx.notify();
        }
    }
}

impl Render for Transfer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let language = crate::i18n::language();
        let mut block = col().child(widgets::setting_row(
            t!("Settings file"),
            Some(t!("Settings, pinned folders, saved searches, connections and folder rules, to bring to another PC. Never a password: those stay with Windows and rclone.")),
            vec![
                widgets::button("transfer-export", t!("Export…"), Some(IconName::FileDown))
                    .on_click(cx.listener(|this, _, window, cx| this.export(window, cx)))
                    .into_any_element(),
                widgets::button("transfer-import", t!("Import…"), Some(IconName::FileUp))
                    .on_click(cx.listener(|this, _, window, cx| this.import(window, cx)))
                    .into_any_element(),
            ],
            &t,
        ));
        let Some(incoming) = &self.incoming else {
            return block;
        };
        let offered: Vec<Part> = ALL_PARTS
            .into_iter()
            .filter(|p| incoming.file.count_of(*p) > 0)
            .collect();
        let none_ticked = incoming.parts.is_empty();
        let when = format_date(
            incoming.file.exported().and_then(parse_iso_ms).unwrap_or(0),
            None,
            language,
        );
        let from = match incoming.file.app() {
            Some(app) => t!("Glint {app}, {date}", app = app, date = when),
            None => when.into(),
        };
        block = block.child(
            col()
                .id("settings-incoming")
                .role(Role::Group)
                .aria_label(base_name(&incoming.path).to_string())
                .aria_description(from.clone())
                .test_support()
                .gap(space(2.))
                .px(space(4.))
                .py(space(3.))
                .border_t_1()
                .border_color(t.border)
                .text_size(theme::size::SM)
                .child(
                    div()
                        .text_start()
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .child(isolate(base_name(&incoming.path).to_string())),
                        )
                        .child(div().text_color(t.muted_foreground).child(from)),
                )
                .when(offered.is_empty(), |this| {
                    this.child(
                        div()
                            .text_color(t.muted_foreground)
                            .child(t!("There's nothing in it to take.")),
                    )
                })
                .children(offered.iter().map(|part| {
                    let part = *part;
                    let on = incoming.parts.contains(&part);
                    let label = part_label(part);
                    let count = format_count(incoming.file.count_of(part));
                    row_start()
                        .items_start()
                        .gap(space(2.))
                        .child(
                            Checkbox::new(widgets::id(&["settings-part", slot(part)]))
                                .checked(on)
                                .accessibility_label(t!("{part} ({count})", part = label, count = count))
                                .on_click(
                                    cx.listener(move |this, checked: &bool, _, cx| this.toggle(part, *checked, cx)),
                                ),
                        )
                        .child(
                            col()
                                .align_start()
                                .child(
                                    div()
                                        .text_start()
                                        .child(t!("{part} ({count})", part = label, count = count)),
                                )
                                .children(part_note(part).map(|note| {
                                    div()
                                        .text_size(theme::size::XS)
                                        .text_color(t.muted_foreground)
                                        .text_start()
                                        .child(note)
                                })),
                        )
                }))
                .child(
                    row_start()
                        .flex_wrap()
                        .gap(space(2.))
                        .pt(space(1.))
                        .child(
                            widgets::primary_button("transfer-add", t!("Add to what's here"), None)
                                .disabled(none_ticked)
                                .tooltip(t!("What's here stays; what the file has besides is added"))
                                .on_click(cx.listener(|this, _, window, cx| this.take(false, window, cx))),
                        )
                        .child(
                            widgets::button("transfer-replace", t!("Replace what's here"), None)
                                .disabled(none_ticked)
                                .tooltip(t!("Each part ticked becomes the file's"))
                                .on_click(cx.listener(|this, _, window, cx| this.take(true, window, cx))),
                        )
                        .child(
                            widgets::quiet_button("transfer-cancel", t!("Cancel")).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.incoming = None;
                                    cx.notify();
                                },
                            )),
                        ),
                ),
        );
        block
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_dates_parse_back_to_the_same_instant() {
        for ms in [0_i64, 1_000, 1_700_000_000_123, 1_794_312_000_000] {
            let iso = persistence::iso_date(ms);
            assert_eq!(parse_iso_ms(&iso), Some(ms), "{iso}");
        }
        assert_eq!(parse_iso_ms("2026-11-10"), Some(1_794_268_800_000));
        assert_eq!(parse_iso_ms("not a date"), None);
        assert_eq!(parse_iso_ms("2026-13-01"), None);
    }
}
