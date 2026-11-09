//! Drive health: what Windows and the drives themselves say about each physical disk.
//! Ported from src/components/explorer/DriveHealth.tsx. **This is the exemplar: every dialog copies it.**
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                         | Here                                                          |
//! |------------------------------------|---------------------------------------------------------------|
//! | zustand store + `refreshHealth`    | [`Health`], a model entity (global), throttled to once a minute |
//! | `useDriveHealth(drivePath)`        | [`Health::for_drive`]                                         |
//! | `useFailingDisks()`                | [`Health::failing`]                                           |
//! | `DriveHealthBadge`                 | [`DriveHealthBadge`]                                          |
//! | `DriveHealthBanner`                | [`DriveHealthBanner`]                                         |
//! | `DriveHealthDialog`                | [`DriveHealthDialog`], opened with [`open`]                   |
//!
//! **The five parts of a dialog** (see [`super`]): the model loads from glint-core on its runtime (never
//! the UI thread) and has `Loading`, `Failed` and `Ready` states; the view uses only kit controls, theme
//! tokens and `t!`, and mirrors for right to left with the kit's logical helpers; every row is a labelled
//! group for screen readers; Escape closes it and focus moves into it (the dialog layer does both); and a
//! headless test at the bottom opens it with fake data and checks all of that.
//!
//! The TypeScript showed one disk (the one whose badge was clicked). So does [`open`] with `Some(disk)`;
//! with `None` (the toolbar button, the `ShowDriveHealth` action) it shows every disk, one section each.

use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use glint_model::format::{format_count, format_size, plural};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use glint_core::drivehealth::DriveHealth;

/// How long a reading is reused (the TypeScript's 60 s): opening This PC again soon doesn't query the drives.
const FRESH_FOR: Duration = Duration::from_secs(60);

/// Where the readings come from: glint-core in the app, a fake in tests.
pub type Source = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<Vec<DriveHealth>, String>> + Send>> + Send + Sync>;

/// glint-core's `drive_health`, as a [`Source`].
pub fn core_source() -> Source {
    Arc::new(|| Box::pin(glint_core::drivehealth::drive_health()))
}

// ---- The model --------------------------------------------------------------------------------------

/// Where a reading stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Not asked for yet.
    Idle,
    /// Asked; waiting for Windows and the drives.
    Loading,
    /// The last attempt failed, with the reason (shown with a retry).
    Failed(SharedString),
    /// `disks` holds the last successful reading.
    Ready,
}

/// The disks' health, shared by the dialog, the badges and the banner. One lives in a global ([`init`]).
pub struct Health {
    disks: Vec<DriveHealth>,
    state: State,
    asked_at: Option<Instant>,
    source: Source,
}

impl Health {
    /// A model reading from `source`. It reads nothing until [`Health::refresh`].
    pub fn new(source: Source) -> Self {
        Self {
            disks: Vec::new(),
            state: State::Idle,
            asked_at: None,
            source,
        }
    }

    /// The disks of the last reading (empty before the first).
    pub fn disks(&self) -> &[DriveHealth] {
        &self.disks
    }

    /// Where the reading stands.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Reads the drives' health, at most once a minute unless `force`. The work runs on core's runtime; the
    /// answer comes back here, and the views redraw. A reading already under way isn't started twice.
    pub fn refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.state == State::Loading {
            return;
        }
        if !force && self.asked_at.is_some_and(|at| at.elapsed() < FRESH_FOR) && self.state == State::Ready {
            return;
        }
        self.asked_at = Some(Instant::now());
        self.state = State::Loading;
        cx.notify();
        let read = (self.source)();
        cx.spawn(async move |this, cx| {
            // Two layers: the core task may panic (outer Err), the drives may not answer (inner Err).
            let result = core_bridge::run(read).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(disks) => {
                        this.disks = disks;
                        this.state = State::Ready;
                    }
                    // Keep the last good reading: a failed retry doesn't blank a dialog that has data.
                    Err(reason) => this.state = State::Failed(reason.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The health of the disk `drive_path` ("C:\") is on, if Windows could read it (`useDriveHealth`).
    pub fn for_drive(&self, drive_path: &str) -> Option<&DriveHealth> {
        self.disks
            .iter()
            .find(|d| d.readable && d.letters.iter().any(|l| l.eq_ignore_ascii_case(drive_path)))
    }

    /// Disks whose health isn't good, for the banner at the top of This PC (`useFailingDisks`).
    pub fn failing(&self) -> Vec<&DriveHealth> {
        self.disks
            .iter()
            .filter(|d| d.readable && Verdict::of(d) != Verdict::Good)
            .collect()
    }
}

/// The shared model, in a GPUI `Global`.
struct SharedHealth(Entity<Health>);

impl Global for SharedHealth {}

/// Makes the shared model (reading nothing yet). Called once at start.
pub fn init(cx: &mut App) {
    let health = cx.new(|_| Health::new(core_source()));
    cx.set_global(SharedHealth(health));
}

/// The shared model.
pub fn shared(cx: &App) -> Entity<Health> {
    cx.global::<SharedHealth>().0.clone()
}

// ---- What the readings mean ---------------------------------------------------------------------------

/// core's verdict string as a type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Good,
    Warning,
    Bad,
}

impl Verdict {
    /// An unknown verdict counts as good, as the TypeScript's fallthrough did.
    pub fn of(health: &DriveHealth) -> Self {
        match health.verdict {
            "bad" => Self::Bad,
            "warning" => Self::Warning,
            _ => Self::Good,
        }
    }

    fn tone(self, t: &theme::Tokens) -> Hsla {
        match self {
            Self::Bad => t.destructive,
            Self::Warning => t.warning,
            Self::Good => t.success,
        }
    }

    fn icon(self) -> IconName {
        if self == Self::Good {
            IconName::HeartPulse
        } else {
            IconName::TriangleAlert
        }
    }

    /// The badge's word.
    fn badge_label(self) -> SharedString {
        match self {
            Self::Bad => t!("Failing"),
            Self::Warning => t!("Check"),
            Self::Good => t!("Healthy"),
        }
    }

    /// The dialog's verdict line.
    fn line(self) -> SharedString {
        match self {
            Self::Bad => t!("May be failing"),
            Self::Warning => t!("Needs a look"),
            Self::Good => t!("Healthy"),
        }
    }
}

/// How the bus is named ("nvme" is "NVMe"); empty for one we have no name for.
fn bus_name(bus: &str) -> SharedString {
    match bus {
        "nvme" => "NVMe".into(),
        "sata" => "SATA".into(),
        "usb" => "USB".into(),
        "sd" => t!("SD card"),
        "raid" => "RAID".into(),
        "virtual" => t!("Virtual"),
        _ => "".into(),
    }
}

/// "C:, D:" for the drive letters a disk holds ("C:\" without its backslash).
fn letters_text(health: &DriveHealth) -> String {
    health
        .letters
        .iter()
        .map(|l| l.replace('\\', ""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A count as the dialog shows it (grouped in the user's locale). Counts beyond `usize` can't occur on a drive.
fn count(n: u64) -> String {
    format_count(usize::try_from(n).unwrap_or(usize::MAX))
}

/// The dialog's name/value rows for a disk, in the TypeScript's order, leaving out what the drive didn't say.
/// Text is translated; numbers are in the user's locale.
pub fn rows(health: &DriveHealth) -> Vec<(SharedString, SharedString)> {
    let mut rows: Vec<(SharedString, SharedString)> = Vec::new();
    if let Some(temperature) = health.temperature {
        let value = match health.temperature_warning.filter(|&w| w != 0) {
            Some(warn) => t!("{t} °C (warns at {w} °C)", t = temperature, w = warn),
            None => t!("{t} °C", t = temperature),
        };
        rows.push((t!("Temperature"), value));
    }
    if let Some(wear) = health.wear {
        rows.push((t!("Life used"), t!("{n}% of what it's rated for", n = wear)));
    }
    if let Some(spare) = health.spare {
        let value = match health.spare_threshold.filter(|&th| th != 0) {
            Some(th) => t!("{n}% left (warns below {th}%)", n = spare, th = th),
            None => t!("{n}% left", n = spare),
        };
        rows.push((t!("Spare blocks"), value));
    }
    if let Some(written) = health.written {
        rows.push((t!("Written to it"), format_size(written).into()));
    }
    if let Some(hours) = health.power_on_hours {
        let days = hours / 24;
        let value = if days != 0 {
            let about = plural(usize::try_from(days).unwrap_or(usize::MAX), "day");
            t!("{hours} hours (about {days})", hours = count(hours), days = about)
        } else {
            t!("{hours} hours", hours = count(hours))
        };
        rows.push((t!("Hours on"), value));
    }
    for (label, value) in [
        (t!("Times turned on"), health.power_cycles),
        (t!("Power lost while on"), health.unsafe_shutdowns),
        (t!("Errors it couldn't fix"), health.media_errors),
        (t!("Sectors moved aside"), health.reallocated),
        (t!("Sectors waiting to be checked"), health.pending),
    ] {
        if let Some(value) = value {
            rows.push((label, count(value).into()));
        }
    }
    rows
}

// ---- The dialog -----------------------------------------------------------------------------------------

/// A disk's health in full: what the drive and Windows report. Loading, failed and ready are all drawn.
pub struct DriveHealthDialog {
    health: Entity<Health>,
    /// Only this disk (a badge was clicked), or every disk.
    only: Option<u32>,
    _redraw_with_model: Subscription,
}

impl DriveHealthDialog {
    /// Shows `only` (or every disk) from `health`, and asks it to read if its reading is stale.
    pub fn new(health: Entity<Health>, only: Option<u32>, cx: &mut Context<Self>) -> Self {
        let redraw = cx.observe(&health, |_, _, cx| cx.notify());
        health.update(cx, |health, cx| health.refresh(false, cx));
        Self {
            health,
            only,
            _redraw_with_model: redraw,
        }
    }

    /// One disk's section: header, verdict, reasons, rows and the footnote.
    fn disk_section(&self, ix: usize, disk: &DriveHealth, t: &theme::Tokens) -> impl IntoElement {
        let verdict = Verdict::of(disk);
        let tone = verdict.tone(t);
        let title = if disk.model.is_empty() {
            t!("Disk {n}", n = disk.disk)
        } else {
            SharedString::from(disk.model.clone())
        };
        let letters = letters_text(disk);
        let subtitle: Vec<SharedString> = [
            Some(bus_name(&disk.bus)).filter(|b| !b.is_empty()),
            (!letters.is_empty()).then(|| t!("Holds {drives}", drives = letters)),
        ]
        .into_iter()
        .flatten()
        .collect();
        let subtitle = subtitle.join(" · ");
        let section_id = disk.disk as usize;

        let header = align_row(3.)
            .items_start()
            .child(
                icon(if verdict == Verdict::Good {
                    IconName::HeartPulse
                } else {
                    IconName::ShieldAlert
                })
                .size_5()
                .text_color(tone),
            )
            .child(
                col()
                    .align_start()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(isolate(title.clone())))
                    .when(!subtitle.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(theme::size::XS)
                                .text_color(t.muted_foreground)
                                .child(subtitle.clone()),
                        )
                    }),
            );

        let verdict_line = align_row(2.)
            .id(("drive-health-verdict", section_id))
            .role(Role::Group)
            .aria_label(verdict.line())
            .test_support()
            .font_weight(FontWeight::MEDIUM)
            .text_color(tone)
            .child(icon(verdict.icon()).size_4())
            .child(verdict.line());

        let reasons = (!disk.reasons.is_empty()).then(|| {
            col()
                .id(("drive-health-reasons", section_id))
                .role(Role::List)
                .aria_label(t!("Reasons"))
                .gap(space(0.5))
                .text_size(theme::size::XS)
                .ps(5.)
                .children(disk.reasons.iter().enumerate().map(|(i, reason)| {
                    align_row(2.)
                        .id(("drive-health-reason", section_id * 64 + i))
                        .role(Role::ListItem)
                        .aria_label(SharedString::from(reason.clone()))
                        .test_support()
                        .items_start()
                        .child(div().child("•"))
                        .child(div().flex_1().min_w_0().child(SharedString::from(reason.clone())))
                }))
        });

        let details = if disk.readable {
            let rows = rows(disk);
            col()
                .id(("drive-health-details", section_id))
                .role(Role::DescriptionList)
                .aria_label(t!("Details"))
                .gap(space(1.5))
                .text_size(theme::size::XS)
                .children(rows.into_iter().enumerate().map(|(i, (label, value))| {
                    // One labelled group per row: "Temperature: 41 °C" is what a screen reader says.
                    align_row(4.)
                        .id(("drive-health-row", section_id * 64 + i))
                        .role(Role::Group)
                        .aria_label(format!("{label}: {value}"))
                        .test_support()
                        .items_start()
                        .child(
                            div()
                                .w(space(48.))
                                .flex_none()
                                .text_color(t.muted_foreground)
                                .child(label),
                        )
                        .child(div().flex_1().min_w_0().child(value))
                }))
                .into_any_element()
        } else {
            div()
                .text_size(theme::size::XS)
                .text_color(t.muted_foreground)
                .child(t!("Windows couldn't read anything from this drive."))
                .into_any_element()
        };

        col()
            .id(("drive-health-disk", section_id))
            .role(Role::Group)
            .aria_label(title)
            .test_support()
            .gap(space(3.))
            .when(ix > 0, |this| this.pt(space(4.)).border_t_1().border_color(t.border))
            .child(header)
            .child(verdict_line)
            .children(reasons)
            .child(details)
    }

    /// The note under the sections: where the numbers come from, and the honest limit of them.
    fn footnote(t: &theme::Tokens) -> impl IntoElement {
        align_row(1.5)
            .items_start()
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(icon(IconName::Thermometer).size_3p5().mt(space(0.5)).flex_none())
            .child(div().flex_1().min_w_0().child(t!(
                "Read from the drive itself, as Windows passes it on. Drives report differently: a drive that says it's healthy can still fail, so keep backups."
            )))
    }
}

impl Render for DriveHealthDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let health = self.health.read(cx);
        let disks: Vec<DriveHealth> = match self.only {
            Some(disk) => health.disks().iter().filter(|d| d.disk == disk).cloned().collect(),
            None => health.disks().to_vec(),
        };
        let state = health.state().clone();

        let body = if !disks.is_empty() {
            col()
                .gap(space(4.))
                .children(
                    disks
                        .iter()
                        .enumerate()
                        .map(|(ix, disk)| self.disk_section(ix, disk, &t)),
                )
                .child(Self::footnote(&t))
                .into_any_element()
        } else {
            match state {
                State::Idle | State::Loading => align_row(2.)
                    .id("drive-health-loading")
                    .role(Role::Status)
                    .aria_label(t!("Reading the drives…"))
                    .test_support()
                    .py(space(6.))
                    .justify_center()
                    .text_color(t.muted_foreground)
                    .child(Spinner::new())
                    .child(t!("Reading the drives…"))
                    .into_any_element(),
                State::Failed(reason) => col()
                    .id("drive-health-error")
                    .role(Role::Alert)
                    .aria_label(t!("Couldn't read the drives: {reason}", reason = reason))
                    .test_support()
                    .gap(space(3.))
                    .py(space(4.))
                    .child(
                        align_row(2.)
                            .items_start()
                            .text_color(t.destructive)
                            .child(icon(IconName::TriangleAlert).size_4().mt(space(0.5)).flex_none())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(t!("Couldn't read the drives: {reason}", reason = reason)),
                            ),
                    )
                    .child(
                        Button::new("drive-health-retry")
                            .label(t!("Try again"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.health.update(cx, |health, cx| health.refresh(true, cx));
                            })),
                    )
                    .into_any_element(),
                State::Ready => div()
                    .id("drive-health-empty")
                    .role(Role::Status)
                    .aria_label(t!("No drive reported its health."))
                    .test_support()
                    .py(space(6.))
                    .text_center()
                    .text_color(t.muted_foreground)
                    .child(t!("No drive reported its health."))
                    .into_any_element(),
            }
        };

        // The dialog layer names its own surface poorly (see notes_for_component_agents), so the content has
        // the name too: a screen reader landing in the dialog hears "Drive health".
        col()
            .id("drive-health")
            .role(Role::Group)
            .aria_label(t!("Drive health"))
            .test_support()
            .text_size(theme::size::SM)
            // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .pt(space(1.))
            .child(body)
    }
}

/// Opens the dialog in `window`: `Some(disk)` for one disk (a badge was clicked), `None` for every disk.
/// Reads the drives first if the last reading is stale.
pub fn open(window: &mut Window, cx: &mut App, only: Option<u32>) {
    open_with(window, cx, shared(cx), only);
}

/// [`open`] for a given model (tests pass one with fake data).
pub fn open_with(window: &mut Window, cx: &mut App, health: Entity<Health>, only: Option<u32>) {
    let view = cx.new(|cx| DriveHealthDialog::new(health, only, cx));
    super::show(window, cx, t!("Drive health"), px(480.), view);
}

// ---- The badge and the banner -------------------------------------------------------------------------

type OnOpen = Rc<dyn Fn(u32, &mut Window, &mut App)>;

/// A drive tile's health: a heart and its temperature, or a warning word; a click shows the details.
#[derive(IntoElement)]
pub struct DriveHealthBadge {
    health: DriveHealth,
    on_open: OnOpen,
}

impl DriveHealthBadge {
    /// `on_open(disk, …)` runs when it's clicked; the usual one is `|disk, w, cx| open(w, cx, Some(disk))`.
    pub fn new(health: DriveHealth, on_open: impl Fn(u32, &mut Window, &mut App) + 'static) -> Self {
        Self {
            health,
            on_open: Rc::new(on_open),
        }
    }
}

impl RenderOnce for DriveHealthBadge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme::tokens(cx);
        let health = self.health;
        let verdict = Verdict::of(&health);
        let tone = verdict.tone(t);
        let label = verdict.badge_label();
        let shown: SharedString = match (verdict, health.temperature) {
            (Verdict::Good, Some(temperature)) => t!("{t} °C", t = temperature),
            _ => label.clone(),
        };
        // The tooltip lists what the badge stands for: the model and verdict, why, and the two headline numbers.
        let mut lines = vec![format!("{}: {}", health.model, label.to_lowercase())];
        lines.extend(health.reasons.iter().cloned());
        if let Some(temperature) = health.temperature {
            lines.push(t!("{t} °C", t = temperature).to_string());
        }
        if let Some(wear) = health.wear {
            lines.push(t!("{n}% of its life used", n = wear).to_string());
        }
        let tooltip: SharedString = lines.join("\n").into();
        let disk = health.disk;
        let on_open = self.on_open;
        Button::new(("drive-health-badge", disk as usize))
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(t.accent.opacity(0.))
                    .foreground(tone)
                    .hover(t.accent)
                    .active(t.accent),
            )
            .compact()
            .tooltip(tooltip.clone())
            .accessibility_label(tooltip.replace('\n', ". "))
            .on_click(move |_, window, cx| {
                // The badge sits on a tile that opens the drive; this click is only for the health.
                cx.stop_propagation();
                on_open(disk, window, cx);
            })
            .child(
                row()
                    .gap(space(1.))
                    .text_size(theme::size::XXS)
                    .font_weight(FontWeight::MEDIUM)
                    .child(icon(verdict.icon()).size_3())
                    .child(shown),
            )
    }
}

/// The banner on This PC when a disk's health isn't good: one clickable card per failing disk.
#[derive(IntoElement)]
pub struct DriveHealthBanner {
    failing: Vec<DriveHealth>,
    on_open: OnOpen,
}

impl DriveHealthBanner {
    /// `failing` is [`Health::failing`], cloned; `on_open` as for the badge.
    pub fn new(failing: Vec<DriveHealth>, on_open: impl Fn(u32, &mut Window, &mut App) + 'static) -> Self {
        Self {
            failing,
            on_open: Rc::new(on_open),
        }
    }
}

impl RenderOnce for DriveHealthBanner {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let on_open = self.on_open;
        col()
            .id("drive-health-banner")
            .gap(space(2.))
            .children(self.failing.into_iter().map(|disk| {
                let verdict = Verdict::of(&disk);
                let tone = verdict.tone(&t);
                let letters = letters_text(&disk);
                let model = if disk.model.is_empty() {
                    t!("Disk {n}", n = disk.disk).to_string()
                } else {
                    disk.model.clone()
                };
                let named = if letters.is_empty() {
                    model
                } else {
                    format!("{model} ({letters})")
                };
                let headline: SharedString = if verdict == Verdict::Bad {
                    t!("{name} may be failing", name = named)
                } else {
                    t!("{name} needs a look", name = named)
                };
                let mut why = disk.reasons.join(" · ");
                if verdict == Verdict::Bad {
                    why.push_str(&t!(". Copy what matters off it soon."));
                }
                let on_open = on_open.clone();
                let id = disk.disk;
                Button::new(("drive-health-banner-item", id as usize))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(tone.opacity(0.1))
                            .foreground(t.foreground)
                            .hover(tone.opacity(0.16))
                            .active(tone.opacity(0.22)),
                    )
                    .w_full()
                    .h_auto()
                    .border_1()
                    .border_color(tone.opacity(0.4))
                    .rounded(theme::size::RADIUS_LG)
                    .px(space(4.))
                    .py(space(3.))
                    .accessibility_label(format!("{headline}. {why}"))
                    .on_click(move |_, window, cx| on_open(id, window, cx))
                    .child(
                        align_row(3.)
                            .w_full()
                            .items_start()
                            .child(
                                icon(IconName::ShieldAlert)
                                    .size_4()
                                    .text_color(tone)
                                    .mt(space(0.5))
                                    .flex_none(),
                            )
                            .child(
                                col()
                                    .flex_1()
                                    .min_w_0()
                                    .align_start()
                                    .text_start()
                                    .child(div().font_weight(FontWeight::SEMIBOLD).child(isolate(headline)))
                                    .child(
                                        div()
                                            .text_size(theme::size::XS)
                                            .text_color(t.muted_foreground)
                                            .child(why),
                                    ),
                            ),
                    )
            }))
    }
}

// ---- Tests -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn disk(n: u32, model: &str) -> DriveHealth {
        DriveHealth {
            disk: n,
            model: model.into(),
            bus: "nvme".into(),
            letters: vec!["C:\\".into(), "D:\\".into()],
            verdict: "good",
            temperature: Some(41),
            temperature_warning: Some(70),
            wear: Some(7),
            power_on_hours: Some(480),
            power_cycles: Some(120),
            unsafe_shutdowns: Some(3),
            media_errors: Some(0),
            spare: Some(100),
            spare_threshold: Some(10),
            written: Some(5 * 1024 * 1024 * 1024),
            readable: true,
            ..Default::default()
        }
    }

    fn failing_disk() -> DriveHealth {
        DriveHealth {
            verdict: "bad",
            reasons: vec![
                "The drive predicts it will fail soon".into(),
                "3 sectors moved aside after failing".into(),
            ],
            predicts_failure: true,
            reallocated: Some(3),
            ..disk(1, "Samsung SSD 970")
        }
    }

    /// A source answering with `disks`, counting how often it's asked.
    fn fake(disks: Result<Vec<DriveHealth>, String>) -> (Source, Arc<AtomicUsize>) {
        let asked = Arc::new(AtomicUsize::new(0));
        let counter = asked.clone();
        let source: Source = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            let answer = disks.clone();
            Box::pin(async move { answer })
        });
        (source, asked)
    }

    /// Completes a frame, then asks the accessibility tree. Queries read "the last completed frame".
    fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    /// Lets the model's read (on core's runtime, another thread) come back to the foreground until `done`.
    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the drive health reading never arrived");
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    /// An app with the skeleton's globals, and a window with the dialog layer.
    fn window(cx: &mut TestAppContext) -> VisualTestContext {
        cx.update(testing::init);
        testing::open_window(cx, |_, cx| cx.new(|_| Host)).0
    }

    #[test]
    fn rows_follow_the_typescript_and_skip_what_the_drive_did_not_say() {
        let _guard = testing::lock();
        let labels: Vec<_> = rows(&disk(0, "A"))
            .into_iter()
            .map(|(label, _)| label.to_string())
            .collect();
        assert_eq!(
            labels,
            [
                "Temperature",
                "Life used",
                "Spare blocks",
                "Written to it",
                "Hours on",
                "Times turned on",
                "Power lost while on",
                "Errors it couldn't fix"
            ]
        );
        let mut r = rows(&disk(0, "A")).into_iter().map(|(_, v)| v.to_string());
        assert_eq!(r.next().as_deref(), Some("41 °C (warns at 70 °C)"));
        assert_eq!(r.next().as_deref(), Some("7% of what it's rated for"));
        assert_eq!(r.next().as_deref(), Some("100% left (warns below 10%)"));
        assert_eq!(r.next().as_deref(), Some("5.0 GB"));
        assert_eq!(r.next().as_deref(), Some("480 hours (about 20 days)"));
        // A drive that says nothing has no rows; zero thresholds are not "warns at 0".
        assert!(rows(&DriveHealth::default()).is_empty());
        let silent = DriveHealth {
            temperature: Some(30),
            temperature_warning: Some(0),
            power_on_hours: Some(5),
            ..Default::default()
        };
        let r = rows(&silent);
        assert_eq!(r[0].1, "30 °C");
        assert_eq!(r[1].1, "5 hours");
    }

    #[test]
    fn verdicts_buses_and_letters_read_as_the_typescript_did() {
        let _guard = testing::lock();
        assert_eq!(Verdict::of(&failing_disk()), Verdict::Bad);
        assert_eq!(
            Verdict::of(&DriveHealth {
                verdict: "warning",
                ..Default::default()
            }),
            Verdict::Warning
        );
        assert_eq!(Verdict::of(&DriveHealth::default()), Verdict::Good);
        assert_eq!(bus_name("nvme"), "NVMe");
        assert_eq!(bus_name("sd"), "SD card");
        assert_eq!(bus_name("other"), "");
        assert_eq!(letters_text(&disk(0, "A")), "C:, D:");
    }

    #[test]
    fn failing_skips_good_and_unreadable_disks() {
        let mut model = Health::new(fake(Ok(vec![])).0);
        model.disks = vec![
            disk(0, "Good"),
            failing_disk(),
            DriveHealth {
                readable: false,
                ..failing_disk()
            },
        ];
        let failing = model.failing();
        assert_eq!(failing.len(), 1);
        assert_eq!(failing[0].model, "Samsung SSD 970");
        assert_eq!(model.for_drive("c:\\").map(|d| d.disk), Some(0));
        assert!(model.for_drive("Z:\\").is_none());
    }

    #[gpui_kit::test]
    fn model_loads_in_the_background_then_reuses_the_reading_for_a_minute(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, asked) = fake(Ok(vec![disk(0, "Fake SSD")]));
        let model = visual.update(|_, cx| cx.new(|_| Health::new(source)));
        assert_eq!(model.read_with(&visual, |m, _| m.state().clone()), State::Idle);
        visual.update(|_, cx| model.update(cx, |m, cx| m.refresh(false, cx)));
        // Loading is visible straight away: the UI thread didn't wait for the answer.
        assert_eq!(model.read_with(&visual, |m, _| m.state().clone()), State::Loading);
        settle(&mut visual, |cx| model.read_with(cx, |m, _| m.state() == &State::Ready));
        assert_eq!(model.read_with(&visual, |m, _| m.disks().len()), 1);
        // A second ask within the minute reads nothing; forcing does.
        visual.update(|_, cx| model.update(cx, |m, cx| m.refresh(false, cx)));
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        visual.update(|_, cx| model.update(cx, |m, cx| m.refresh(true, cx)));
        settle(&mut visual, |_| asked.load(Ordering::SeqCst) == 2);
    }

    #[gpui_kit::test]
    fn dialog_shows_loading_then_labelled_rows(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, _) = fake(Ok(vec![disk(0, "Fake SSD")]));
        let model = visual.update(|_, cx| cx.new(|_| Health::new(source)));
        visual.update(|window, cx| open_with(window, cx, model.clone(), None));
        visual.run_until_parked();

        // The dialog layer is up, with the role a screen reader needs, and the model has been asked.
        visual.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            window.render_frame(cx);
            assert!(
                !window.find_all_by_role(Role::Dialog).is_empty(),
                "no element has the Dialog role"
            );
        });
        assert_eq!(model.read_with(&visual, |m, _| m.state().clone()), State::Loading);
        // While the drives are being read there's a status line, not a blank dialog.
        assert_eq!(role_of(&mut visual, "Reading the drives…"), Some(Role::Status));

        settle(&mut visual, |cx| has(cx, "Temperature: 41 °C (warns at 70 °C)"));
        // The content is named after the dialog, and each disk and row is a labelled group.
        assert_eq!(role_of(&mut visual, "Drive health"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "Fake SSD"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "Healthy"), Some(Role::Group));
        for label in [
            "Life used: 7% of what it's rated for",
            "Spare blocks: 100% left (warns below 10%)",
            "Hours on: 480 hours (about 20 days)",
            "Times turned on: 120",
            "Power lost while on: 3",
        ] {
            assert_eq!(role_of(&mut visual, label), Some(Role::Group), "{label}");
        }
        // Rows the drive didn't report aren't there, and the loading line is gone.
        assert!(!has(&mut visual, "Sectors moved aside: 0"));
        assert!(!has(&mut visual, "Reading the drives…"));
    }

    #[gpui_kit::test]
    fn dialog_for_one_failing_disk_lists_the_reasons(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, _) = fake(Ok(vec![disk(0, "Fake SSD"), failing_disk()]));
        let model = visual.update(|_, cx| cx.new(|_| Health::new(source)));
        visual.update(|window, cx| open_with(window, cx, model.clone(), Some(1)));
        settle(&mut visual, |cx| has(cx, "Samsung SSD 970"));
        assert!(!has(&mut visual, "Fake SSD"), "only the disk that was clicked");
        assert_eq!(role_of(&mut visual, "May be failing"), Some(Role::Group));
        assert_eq!(
            role_of(&mut visual, "The drive predicts it will fail soon"),
            Some(Role::ListItem)
        );
        visual.update(|window, _| assert_eq!(window.find_all_by_role(Role::ListItem).len(), 2));
    }

    #[gpui_kit::test]
    fn dialog_shows_the_error_and_a_retry_that_asks_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, asked) = fake(Err("access denied".into()));
        let model = visual.update(|_, cx| cx.new(|_| Health::new(source)));
        visual.update(|window, cx| open_with(window, cx, model.clone(), None));
        settle(&mut visual, |cx| has(cx, "Couldn't read the drives: access denied"));
        assert_eq!(
            role_of(&mut visual, "Couldn't read the drives: access denied"),
            Some(Role::Alert)
        );
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("drive-health-retry", cx);
        });
        settle(&mut visual, |_| asked.load(Ordering::SeqCst) == 2);
    }

    #[gpui_kit::test]
    fn escape_closes_the_dialog_and_focus_starts_inside_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, _) = fake(Ok(vec![disk(0, "Fake SSD")]));
        let model = visual.update(|_, cx| cx.new(|_| Health::new(source)));
        visual.update(|window, cx| open_with(window, cx, model.clone(), None));
        visual.run_until_parked();
        visual.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            // Focus moved into the dialog layer when it opened.
            assert!(
                window.focused(cx).is_some(),
                "nothing has focus after the dialog opened"
            );
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "Escape must close the dialog"));
    }

    /// A view with a banner and a badge, for the two small parts.
    struct Parts {
        opened: Rc<RefCell<Vec<u32>>>,
    }

    impl Render for Parts {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let opened = self.opened.clone();
            let opened_too = self.opened.clone();
            col()
                .size_full()
                .p(space(4.))
                .gap(space(4.))
                .child(DriveHealthBanner::new(vec![failing_disk()], move |disk, _, _| {
                    opened.borrow_mut().push(disk)
                }))
                .child(DriveHealthBadge::new(disk(0, "Fake SSD"), move |disk, _, _| {
                    opened_too.borrow_mut().push(disk)
                }))
        }
    }

    #[gpui_kit::test]
    fn banner_and_badge_are_buttons_with_names_that_open_the_dialog(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.update(testing::init);
        let opened = Rc::new(RefCell::new(Vec::new()));
        let shared = opened.clone();
        let (mut visual, _) = testing::open_window(cx, move |_, cx| cx.new(|_| Parts { opened: shared }));
        let banner = "Samsung SSD 970 (C:, D:) may be failing. The drive predicts it will fail soon · 3 sectors moved aside after failing. Copy what matters off it soon.";
        assert_eq!(role_of(&mut visual, banner), Some(Role::Button));
        assert_eq!(
            role_of(&mut visual, "Fake SSD: healthy. 41 °C. 7% of its life used"),
            Some(Role::Button)
        );
        visual.update(|window, cx| {
            // Ids with a number are (name, number): the disk's number, here.
            window.click(("drive-health-banner-item", 1_usize), cx);
            window.click(("drive-health-badge", 0_usize), cx);
        });
        assert_eq!(*opened.borrow(), vec![1, 0]);
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_render_without_trouble(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        let arabic = DriveHealth {
            letters: vec!["ص:\\".into()],
            ..disk(0, "قرص التخزين السريع")
        };
        let (source, _) = fake(Ok(vec![arabic]));
        let model = visual.update(|_, cx| cx.new(|_| Health::new(source)));
        visual.update(|window, cx| open_with(window, cx, model.clone(), None));
        settle(&mut visual, |cx| has(cx, "قرص التخزين السريع"));
        // The name is exposed as the drive reports it, without the layout-only isolates.
        assert_eq!(role_of(&mut visual, "قرص التخزين السريع"), Some(Role::Group));
        testing::set_rtl(false);
    }
}
