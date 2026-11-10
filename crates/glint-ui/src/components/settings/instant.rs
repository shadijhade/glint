//! Settings → Instant search: turning the NTFS index service on and off, and how it is doing (`InstantSearch`
//! of SettingsView.tsx, and `enableIndex`, `disableIndex` and `refreshIndex` of explorer.ts).
//!
//! The service runs elevated, so turning it on or off makes Windows ask for administrator approval; that
//! prompt belongs to the main window (core takes it as the owner). A declined prompt answers "cancelled" and
//! says nothing, as before.

use super::backend::{Backend, IndexInfo, IndexState, Volume};
use super::widgets::{self, bad, good};
use crate::kit::prelude::*;
use crate::{t, theme};
use glint_model::format::{format_count, format_duration, format_size};
use std::sync::Arc;
use std::time::Duration;

/// How long to wait before asking again while a drive is being read, or after an error on one.
const SCANNING_POLL: Duration = Duration::from_secs(1);
const ERROR_POLL: Duration = Duration::from_secs(5);

/// The Instant search card.
pub struct InstantSearch {
    backend: Arc<dyn Backend>,
    info: Option<IndexInfo>,
    /// A turn on or off is under way (a spinner shows and the buttons wait).
    working: bool,
    /// The Microsoft Store's copy: uninstalling it runs nothing of Glint's.
    store: bool,
    poll: Option<Task<()>>,
}

impl InstantSearch {
    /// Reads the service's state at once (`refreshIndex` on mount).
    pub fn new(backend: Arc<dyn Backend>, store: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            backend,
            info: None,
            working: false,
            store,
            poll: None,
        };
        this.refresh(window, cx);
        this
    }

    /// The last answer.
    pub fn info(&self) -> Option<&IndexInfo> {
        self.info.as_ref()
    }

    /// Whether a turn on or off is under way.
    pub fn working(&self) -> bool {
        self.working
    }

    /// Asks the service how it does. While a drive is read, asks again every second; after an error on one,
    /// every five. A failed question keeps the last answer.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let asked = self.backend.index_status();
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |this, window, cx| {
                let next = match answer {
                    Ok(info) => {
                        let next = next_poll(&info);
                        this.info = Some(info);
                        next
                    }
                    Err(_) => None,
                };
                this.poll = next.map(|delay| {
                    cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor().timer(delay).await;
                        this.update_in(cx, |this, window, cx| this.refresh(window, cx)).ok();
                    })
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Turns instant search on, starts it, or updates its service (`enableIndex`).
    pub fn turn_on(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.working {
            return;
        }
        let updating = self.info.as_ref().is_some_and(IndexInfo::outdated);
        self.working = true;
        cx.notify();
        let asked = self.backend.index_enable();
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |this, window, cx| {
                this.working = false;
                match answer {
                    Ok(info) => {
                        let version = info.app_version.clone();
                        this.info = Some(info);
                        if updating {
                            good(
                                window,
                                cx,
                                t!("Instant search is updated"),
                                Some(t!("Its service now runs version {version}.", version = version)),
                            );
                        } else {
                            good(
                                window,
                                cx,
                                t!("Instant search is on"),
                                Some(t!("Each drive is searched with the index as soon as it has been read.")),
                            );
                        }
                        this.refresh(window, cx);
                    }
                    Err(reason) if reason == "cancelled" => {}
                    Err(reason) => bad(
                        window,
                        cx,
                        if updating {
                            t!("Couldn't update instant search")
                        } else {
                            t!("Couldn't turn on instant search")
                        },
                        reason,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Stops and removes the service and its index files (`disableIndex`).
    pub fn turn_off(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.working {
            return;
        }
        self.working = true;
        cx.notify();
        let asked = self.backend.index_disable();
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |this, window, cx| {
                this.working = false;
                match answer {
                    Ok(info) => {
                        this.info = Some(info);
                        good(
                            window,
                            cx,
                            t!("Instant search is off"),
                            Some(t!("The service and its index files were removed.")),
                        );
                    }
                    Err(reason) if reason == "cancelled" => {}
                    Err(reason) => bad(window, cx, t!("Couldn't turn off instant search"), reason),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// When to ask again (`refreshIndex`): every second while a drive is read or none is listed yet, every five
/// seconds after an error on one, never otherwise; and only a running service is asked again.
fn next_poll(info: &IndexInfo) -> Option<Duration> {
    if info.state != IndexState::On {
        None
    } else if info.volumes.is_empty() || info.volumes.iter().any(|v| v.state == "scanning") {
        Some(SCANNING_POLL)
    } else if info.volumes.iter().any(|v| v.state == "error") {
        Some(ERROR_POLL)
    } else {
        None
    }
}

/// One drive in a few words (`describeVolume`).
pub fn describe_volume(v: &Volume) -> SharedString {
    match v.state.as_str() {
        "scanning" if v.progress_total != 0 => {
            // Done in 128 bits: the product of two 64-bit counts doesn't fit in 64.
            let percent = u128::from(v.progress) * 100 / u128::from(v.progress_total);
            t!("{volume} reading {percent}%…", volume = v.volume, percent = percent)
        }
        "scanning" => t!("{volume} reading…", volume = v.volume),
        "error" => match &v.error {
            Some(error) => t!("{volume} unavailable ({error})", volume = v.volume, error = error),
            None => t!("{volume} unavailable", volume = v.volume),
        },
        _ => t!(
            "{volume} {count} items",
            volume = v.volume,
            count = format_count(usize::try_from(v.files.saturating_add(v.folders)).unwrap_or(usize::MAX))
        ),
    }
}

/// What the tooltip lists: each drive's memory and how its index was built.
fn details(volumes: &[Volume]) -> String {
    volumes
        .iter()
        .map(|v| {
            if v.state == "ready" {
                let source = if v.source == "scan" {
                    t!("the MFT")
                } else {
                    t!("its snapshot")
                };
                t!(
                    "{volume} {memory} in memory, built from {source} in {time}",
                    volume = v.volume,
                    memory = format_size(v.memory),
                    source = source,
                    time = format_duration(v.build_ms)
                )
                .to_string()
            } else {
                format!("{} {}", v.volume, v.error.clone().unwrap_or_else(|| v.state.clone()))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Render for InstantSearch {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let info = self.info.clone();
        let state = info.as_ref().map_or(IndexState::Off, |i| i.state);
        let volumes = info.as_ref().map(|i| i.volumes.clone()).unwrap_or_default();
        let outdated = info.as_ref().is_some_and(IndexInfo::outdated);
        let hint: SharedString = match state {
            IndexState::On if volumes.is_empty() => t!("Starting…"),
            IndexState::On => volumes
                .iter()
                .map(describe_volume)
                .collect::<Vec<_>>()
                .join(" · ")
                .into(),
            IndexState::Stopped => t!("Installed, but the service isn't running. Turning it on starts it again."),
            IndexState::NoAccess => t!("Another account on this PC turned it on. Allow yours too to use it."),
            IndexState::Untrusted => match info.as_ref().and_then(|i| i.error.clone()) {
                Some(error) => error.into(),
                None => t!("Something else is using the service's name, so it isn't trusted."),
            },
            IndexState::Unsupported => t!("Needs Windows and NTFS drives."),
            IndexState::Off => t!(
                "Searches whole drives by name in milliseconds, and shows every folder's size as it opens, like Everything and WizTree."
            ),
        };
        let title: SharedString = if state == IndexState::On {
            if outdated {
                t!("On · an update for the service is ready")
            } else {
                t!("On")
            }
        } else {
            t!("Index file names and folder sizes")
        };
        let enable_label = match state {
            IndexState::On => t!("Update"),
            IndexState::Stopped => t!("Start"),
            IndexState::NoAccess => t!("Allow me"),
            IndexState::Untrusted | IndexState::Unsupported | IndexState::Off => t!("Turn on"),
        };
        let details_text = details(&volumes);
        let working = self.working;
        let note = {
            let mut text = t!(
                "Turning it on asks for administrator approval once: a small Windows service reads your NTFS drives' file tables, follows their change journals, and answers only your account and administrators. Like Everything, it knows the name, size and date of every file on those drives, including files your account can't open. Turning it off removes the service and its index files."
            )
            .to_string();
            if self.store {
                text.push(' ');
                text.push_str(&t!(
                    "Uninstalling Glint from the Microsoft Store removes it too, within the hour, unless another copy of Glint here uses it."
                ));
            }
            text
        };
        col()
            .id("settings-instant-search-body")
            .child(
                row_start()
                    .id("settings-instant-search-state")
                    .role(Role::Group)
                    .aria_label(title.clone())
                    .aria_description(hint.clone())
                    .test_support()
                    .justify_between()
                    .gap(space(6.))
                    .px(space(4.))
                    .py(space(3.))
                    .child(
                        row_start()
                            .flex_1()
                            .min_w_0()
                            .items_start()
                            .gap(space(3.))
                            .child(
                                icon(IconName::Zap)
                                    .size_4()
                                    .mt(space(0.5))
                                    .flex_none()
                                    .text_color(if state == IndexState::On {
                                        t.primary
                                    } else {
                                        t.muted_foreground
                                    }),
                            )
                            .child(
                                col()
                                    .id("settings-instant-search-text")
                                    .min_w_0()
                                    .align_start()
                                    .when(!details_text.is_empty(), |this| {
                                        let tip: SharedString = details_text.clone().into();
                                        this.tooltip(move |window, cx| tooltip(tip.clone(), window, cx))
                                    })
                                    .child(div().text_size(theme::size::SM).text_start().child(title))
                                    .child(
                                        div()
                                            .mt(space(0.5))
                                            .text_size(theme::size::XS)
                                            .text_start()
                                            .text_color(t.muted_foreground)
                                            .child(hint),
                                    ),
                            ),
                    )
                    .child(
                        row_start()
                            .flex_none()
                            .gap(space(1.5))
                            .when(working, |this| {
                                this.child(div().size_4().text_color(t.muted_foreground).child(Spinner::new()))
                            })
                            .when((state != IndexState::On || outdated) && state != IndexState::Unsupported, |this| {
                                this.child(
                                    widgets::primary_button("instant-on", enable_label, Some(IconName::ShieldCheck))
                                        .disabled(working)
                                        .tooltip(if outdated {
                                            t!("The service updates itself when it can; otherwise Windows asks for administrator approval")
                                        } else {
                                            t!("Windows asks for administrator approval")
                                        })
                                        .on_click(cx.listener(|this, _, window, cx| this.turn_on(window, cx))),
                                )
                            })
                            .when(matches!(state, IndexState::On | IndexState::Stopped), |this| {
                                this.child(
                                    widgets::button("instant-off", t!("Turn off"), None)
                                        .disabled(working)
                                        .tooltip(t!("Windows asks for administrator approval"))
                                        .on_click(cx.listener(|this, _, window, cx| this.turn_off(window, cx))),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(t.border)
                    .child(widgets::note(note, &t)),
            )
    }
}
