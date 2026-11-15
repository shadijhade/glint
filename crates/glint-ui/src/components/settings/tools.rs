//! Settings → Tools: the programs Glint drives (`ToolsSection` and `ToolRow` of SettingsView.tsx; the state of
//! tools.ts).
//!
//! rg and fd come with Glint (and pdu's engine is built in); the others install when a feature needs them, from the
//! tool's own pinned release (winget only when that can't be had). The page sorts them by what they're for, says
//! how many are ready and how many have an update, updates them all with one button, and shows an install's
//! progress as a bar under its row. Clouds also reads this model (is rclone there?) and draws [`needed`], the card
//! that offers to install a missing tool (`ToolNeeded`).

use super::backend::{Backend, Fetch, ToolInfo};
use super::widgets::{self, bad, good};
use crate::kit::prelude::*;
use crate::{t, theme};
use glint_model::format::format_size;
use std::collections::HashMap;
use std::sync::Arc;

/// "Downloading · 2.1 MB of 7.4 MB", or what the install is doing ("Unpacking…").
pub fn describe_progress(fetch: &Fetch) -> SharedString {
    if fetch.total == 0 {
        return if fetch.stage.ends_with('…') {
            fetch.stage.clone().into()
        } else {
            format!("{}…", fetch.stage.trim_end_matches('.')).into()
        };
    }
    t!(
        "{stage} · {done} of {total}",
        stage = fetch.stage,
        done = format_size(fetch.done),
        total = format_size(fetch.total)
    )
}

/// How far a download has come, 0 to 1; None while the install does something it can't measure.
fn fraction(fetch: &Fetch) -> Option<f32> {
    (fetch.total > 0).then(|| (fetch.done as f64 / fetch.total as f64).clamp(0., 1.) as f32)
}

/// What Install will do, for its tooltip (`installHint`).
pub fn install_hint(tool: &ToolInfo) -> SharedString {
    let version = tool.pinned.clone().unwrap_or_default();
    t!(
        "Downloads {name} {version} from its own release and checks it before anything runs",
        name = tool.name,
        version = version
    )
    .replace("  ", " ")
    .into()
}

/// What a tool is for, as the page groups them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Media,
    Documents,
    Files,
    Sharing,
    Developers,
    BuiltIn,
}

impl Group {
    /// The groups in the order the page shows them.
    pub const ALL: [Group; 6] = [
        Group::Media,
        Group::Documents,
        Group::Files,
        Group::Sharing,
        Group::Developers,
        Group::BuiltIn,
    ];

    /// The group a tool is in, by its id.
    pub fn of(id: &str) -> Group {
        match id {
            "ripgrep" | "fd" => Group::BuiltIn,
            "ffmpeg" | "whisper" | "czkawka" | "rimage" | "exiftool" => Group::Media,
            "pdfcpu" | "pandoc" | "typst" => Group::Documents,
            "7zip" | "restic" | "duckdb" => Group::Files,
            "rclone" | "sendme" | "dufs" => Group::Sharing,
            _ => Group::Developers,
        }
    }

    fn slot(self) -> &'static str {
        match self {
            Group::Media => "media",
            Group::Documents => "documents",
            Group::Files => "files",
            Group::Sharing => "sharing",
            Group::Developers => "developers",
            Group::BuiltIn => "built-in",
        }
    }

    fn name(self) -> SharedString {
        match self {
            Group::Media => t!("Photos, videos and music"),
            Group::Documents => t!("Documents and PDFs"),
            Group::Files => t!("Archives, backups and data"),
            Group::Sharing => t!("Clouds and sharing"),
            Group::Developers => t!("For developers"),
            Group::BuiltIn => t!("Comes with Glint"),
        }
    }

    fn icon(self) -> IconName {
        match self {
            Group::Media => IconName::Film,
            Group::Documents => IconName::FileText,
            Group::Files => IconName::Archive,
            Group::Sharing => IconName::Cloud,
            Group::Developers => IconName::SquareTerminal,
            Group::BuiltIn => IconName::Zap,
        }
    }
}

/// How many tools are ready, have an update, and are missing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub ready: usize,
    pub updates: usize,
    pub missing: usize,
}

/// The tools, and the installs under way.
pub struct Tools {
    backend: Arc<dyn Backend>,
    tools: Vec<ToolInfo>,
    /// Installs and removals running now, by tool id.
    busy: HashMap<String, Fetch>,
    checking: bool,
    /// Update all as soon as the tools are known (asked before they were).
    update_when_known: bool,
}

impl Tools {
    /// Looks for every tool at once (`useEffect(check)`).
    pub fn new(backend: Arc<dyn Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            backend,
            tools: Vec::new(),
            busy: HashMap::new(),
            checking: false,
            update_when_known: false,
        };
        this.refresh(window, cx);
        this
    }

    /// A tool's status once known.
    pub fn tool(&self, id: &str) -> Option<&ToolInfo> {
        self.tools.iter().find(|t| t.id == id)
    }

    /// Whether the tools have been looked for yet.
    pub fn loaded(&self) -> bool {
        !self.tools.is_empty()
    }

    /// The installs and removals under way.
    pub fn busy(&self, id: &str) -> Option<&Fetch> {
        self.busy.get(id)
    }

    /// Asks where each tool is and its version (`check`).
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        self.checking = true;
        cx.notify();
        let tools = self.backend.tools_status();
        cx.spawn_in(window, async move |this, cx| {
            let tools = tools.await;
            this.update_in(cx, |this, window, cx| {
                this.checking = false;
                if let Ok(tools) = tools {
                    this.tools = tools;
                }
                if std::mem::take(&mut this.update_when_known) {
                    this.update_all(window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Updates every tool that has an update, now if the tools are known, else as soon as they are.
    pub fn update_all_once_known(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loaded() && !self.checking {
            self.update_all(window, cx);
        } else {
            self.update_when_known = true;
        }
    }

    /// Installs a tool, showing how far it has got (`installTool`).
    pub fn install(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.contains_key(&id) {
            return;
        }
        let name = self.tool(&id).map_or_else(|| id.clone(), |t| t.name.clone());
        self.busy.insert(
            id.clone(),
            Fetch {
                stage: t!("Starting").to_string(),
                done: 0,
                total: 0,
            },
        );
        cx.notify();
        let (progress, heard) = async_channel::unbounded();
        let asked = self.backend.tool_install(id.clone(), progress);
        let watching = id.clone();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(fetch) = heard.recv().await {
                let watching = watching.clone();
                if this
                    .update(cx, |this, cx| {
                        this.busy.insert(watching, fetch);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |this, window, cx| {
                this.busy.remove(&id);
                match answer {
                    Ok(()) => {
                        this.refresh(window, cx);
                        good(window, cx, t!("{name} is installed", name = name), None);
                    }
                    Err(reason) if reason == "cancelled" => {}
                    Err(reason) => bad(window, cx, t!("Couldn't install {name}", name = name), reason),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Removes a copy Glint downloaded (`removeTool`).
    pub fn remove(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.tool(&id).map_or_else(|| id.clone(), |t| t.name.clone());
        self.busy.insert(
            id.clone(),
            Fetch {
                stage: t!("Removing").to_string(),
                done: 0,
                total: 0,
            },
        );
        cx.notify();
        let asked = self.backend.tool_remove(id.clone());
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |this, window, cx| {
                this.busy.remove(&id);
                match answer {
                    Ok(()) => {
                        this.refresh(window, cx);
                        good(window, cx, t!("{name} is removed", name = name), None);
                    }
                    Err(reason) => bad(window, cx, t!("Couldn't remove {name}", name = name), reason),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Stops the install of `id` (`cancelInstall`).
    pub fn cancel(&self, id: &str) {
        self.backend.cancel_install(id.to_string());
    }

    /// How many tools are ready, have an update, and are missing.
    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for tool in &self.tools {
            if tool.origin.is_none() {
                counts.missing += 1;
            } else {
                counts.ready += 1;
            }
            if tool.update.is_some() {
                counts.updates += 1;
            }
        }
        counts
    }

    /// Updates every tool that has an update, each with its own progress (Update all).
    pub fn update_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .tools
            .iter()
            .filter(|tool| tool.update.is_some() && !self.busy.contains_key(&tool.id))
            .map(|tool| tool.id.clone())
            .collect();
        for id in ids {
            self.install(id, window, cx);
        }
    }

    /// One program Glint drives: its version, where it came from, and Install, Update, Stop or Remove, with the
    /// install's progress as a bar under it (`ToolRow`).
    fn tool_row(&self, tool: &ToolInfo, cx: &mut Context<Self>, t: &theme::Tokens) -> AnyElement {
        let slot = format!("tool-{}", tool.id);
        let where_from = match tool.origin.as_deref() {
            Some("downloaded") => t!("Downloaded by Glint"),
            Some("installed") => t!("Installed on this PC"),
            Some(_) => t!("Comes with Glint"),
            None => t!("Not installed"),
        };
        let label: SharedString = match &tool.version {
            Some(version) => format!("{} {}", tool.name, version).into(),
            None => tool.name.clone().into(),
        };
        let hint: SharedString = format!("{} · {}", tool.used_for, where_from).into();
        let busy = self.busy.get(&tool.id);
        let controls: Vec<AnyElement> = match busy {
            Some(fetch) => {
                let id = tool.id.clone();
                let described = describe_progress(fetch);
                vec![
                    div()
                        .id(widgets::id(&["settings-progress", &slot]))
                        .role(Role::Status)
                        .aria_label(described.clone())
                        .test_support()
                        .max_w(space(56.))
                        .min_w_0()
                        .truncate()
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(described)
                        .into_any_element(),
                    widgets::button(&format!("{slot}-stop"), t!("Stop"), None)
                        .on_click(cx.listener(move |this, _, _, _| this.cancel(&id)))
                        .into_any_element(),
                ]
            }
            None => {
                let mut controls = Vec::new();
                if tool.origin.is_some() && tool.update.is_none() {
                    controls.push(status_pill(&slot, t!("Ready"), t.success, t));
                }
                if tool.origin.is_none() || tool.update.is_some() {
                    let id = tool.id.clone();
                    let label = match &tool.update {
                        Some(version) => t!("Update to {version}", version = version),
                        None => t!("Install"),
                    };
                    controls.push(
                        widgets::primary_button(&format!("{slot}-install"), label, Some(IconName::Download))
                            .tooltip(install_hint(tool))
                            .on_click(cx.listener(move |this, _, window, cx| this.install(id.clone(), window, cx)))
                            .into_any_element(),
                    );
                }
                if tool.removable {
                    let id = tool.id.clone();
                    let mut button = widgets::quiet_button(&format!("{slot}-remove"), t!("Remove"))
                        .accessibility_label(t!("Remove {name}", name = tool.name))
                        .on_click(cx.listener(move |this, _, window, cx| this.remove(id.clone(), window, cx)));
                    if let Some(path) = &tool.path {
                        button = button.tooltip(path.clone());
                    }
                    controls.push(button.into_any_element());
                }
                controls
            }
        };
        let row = widgets::setting_row(label, Some(hint), controls, t);
        match busy {
            Some(fetch) => col()
                .w_full()
                .child(row)
                .child(div().px(space(4.)).pb(space(3.)).child(progress_bar(&slot, fetch, cx)))
                .into_any_element(),
            None => row,
        }
    }

    /// The line at the top: how many are ready and have an update, with Update all and Refresh.
    fn summary(&self, cx: &mut Context<Self>, t: &theme::Tokens) -> AnyElement {
        let counts = self.counts();
        let said = if !self.loaded() {
            t!("Looking for them…")
        } else if counts.updates > 0 {
            t!(
                "{ready} ready · {updates} with an update",
                ready = counts.ready,
                updates = counts.updates
            )
        } else {
            t!("{ready} ready, all up to date", ready = counts.ready)
        };
        let checking = self.checking;
        let mut actions: Vec<AnyElement> = Vec::new();
        if counts.updates > 0 {
            actions.push(
                widgets::primary_button(
                    "tools-update-all",
                    t!("Update all ({count})", count = counts.updates),
                    Some(IconName::Download),
                )
                .on_click(cx.listener(|this, _, window, cx| this.update_all(window, cx)))
                .into_any_element(),
            );
        }
        actions.push(
            widgets::quiet_button("tools-refresh", t!("Refresh"))
                .icon(IconName::RefreshCw)
                .loading(checking)
                .disabled(checking)
                .tooltip(t!("Look for each tool again"))
                .on_click(cx.listener(|this, _, window, cx| this.refresh(window, cx)))
                .into_any_element(),
        );
        row_start()
            .id("settings-tools-summary")
            .role(Role::Status)
            .aria_label(said.clone())
            .test_support()
            .w_full()
            .justify_between()
            .gap(space(3.))
            .px(space(4.))
            .py(space(3.))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.border)
            .bg(t.card)
            .child(
                row_start()
                    .min_w_0()
                    .gap(space(2.5))
                    .child(icon(IconName::Wrench).size_4().flex_none().text_color(t.primary))
                    .child(div().min_w_0().truncate().text_size(theme::size::SM).child(said)),
            )
            .child(row_start().flex_none().gap(space(1.5)).children(actions))
            .into_any_element()
    }
}

/// A small rounded label with a coloured dot ("Ready").
fn status_pill(slot: &str, text: SharedString, color: Hsla, t: &theme::Tokens) -> AnyElement {
    row_start()
        .id(widgets::id(&["settings-pill", slot]))
        .role(Role::Status)
        .aria_label(text.clone())
        .test_support()
        .flex_none()
        .gap(space(1.5))
        .h(space(6.))
        .px(space(2.5))
        .rounded_full()
        .bg(color.opacity(0.12))
        .text_size(theme::size::XS)
        .text_color(t.foreground)
        .child(div().size(space(1.5)).rounded_full().bg(color))
        .child(text)
        .into_any_element()
}

/// The bar under an install: how much of the download has come, or the glint sweeping while it unpacks or winget
/// works (nothing to measure).
fn progress_bar(slot: &str, fetch: &Fetch, cx: &App) -> AnyElement {
    let Some(done) = fraction(fetch) else {
        return loading_sweep(widgets::id(&["settings-sweep", slot]), cx);
    };
    let t = theme::tokens(cx);
    row_start()
        .id(widgets::id(&["settings-bar", slot]))
        .role(Role::ProgressIndicator)
        .aria_label(describe_progress(fetch))
        .test_support()
        .h(px(3.))
        .w_full()
        .overflow_hidden()
        .rounded_full()
        .bg(t.primary.opacity(0.16))
        .child(div().h_full().w(relative(done)).rounded_full().bg(theme::glint_fill(cx)))
        .into_any_element()
}

impl Render for Tools {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let mut sections: Vec<AnyElement> = vec![self.summary(cx, &t)];
        let tools = self.tools.clone();
        for group in Group::ALL {
            let rows: Vec<AnyElement> = tools
                .iter()
                .filter(|tool| Group::of(&tool.id) == group)
                .map(|tool| self.tool_row(tool, cx, &t))
                .collect();
            if rows.is_empty() {
                continue;
            }
            sections.push(group_card(group, rows, &t));
        }
        sections.push(
            widgets::note(
                t!("ripgrep and fd come with Glint, and pdu's engine is built in. Other tools install when a feature needs them, for your account only, from the tool's own release, which Glint checks against the checksum it knows for it before unpacking anything. winget is only tried when that download can't be had."),
                &t,
            )
            .into_any_element(),
        );
        col()
            .id("settings-tools")
            .role(Role::Group)
            .aria_label(t!("Tools"))
            .test_support()
            .w_full()
            .gap(space(4.))
            .children(sections)
    }
}

/// One group's card: its sign and name over its tools' rows.
fn group_card(group: Group, rows: Vec<AnyElement>, t: &theme::Tokens) -> AnyElement {
    let divided = rows.into_iter().enumerate().map(|(i, row)| {
        div()
            .when(i > 0, |this| this.border_t_1().border_color(t.border))
            .child(row)
    });
    col()
        .id(widgets::id(&["settings-tools-group", group.slot()]))
        .role(Role::Group)
        .aria_label(group.name())
        .test_support()
        .w_full()
        .child(
            row_start()
                .h(space(7.))
                .px(space(1.))
                .mb(space(2.))
                .gap(space(2.))
                .child(icon(group.icon()).size_4().flex_none().text_color(t.muted_foreground))
                .child(
                    div()
                        .text_size(theme::size::SM)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(group.name()),
                ),
        )
        .child(
            col()
                .w_full()
                .rounded(theme::size::RADIUS_LG)
                .border_1()
                .border_color(t.border)
                .bg(t.card)
                .overflow_hidden()
                .children(divided),
        )
        .into_any_element()
}

/// The card that says a feature needs a tool and installs it with one click, with its progress and Stop
/// (`ToolNeeded`). `None` once the tool is there, or before the tools are known.
pub fn needed(tools: &Entity<Tools>, id: &str, why: SharedString, icon_name: IconName, cx: &App) -> Option<AnyElement> {
    let t = theme::tokens(cx);
    let model = tools.read(cx);
    let tool = model.tool(id)?;
    if tool.origin.is_some() {
        return None;
    }
    let name = tool.name.clone();
    let hint = install_hint(tool);
    let tool_id = id.to_string();
    let busy = model.busy(id).cloned();
    let controls: Vec<AnyElement> = match &busy {
        Some(fetch) => {
            let tools = tools.clone();
            let stop_id = tool_id.clone();
            vec![
                // Held to a width of its own: a long line used to take the whole card and squeeze what the card
                // says into a column one letter wide.
                div()
                    .max_w(space(48.))
                    .min_w_0()
                    .truncate()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(describe_progress(fetch))
                    .into_any_element(),
                widgets::button(&format!("needed-{id}-stop"), t!("Stop"), None)
                    .on_click(move |_, _, cx| tools.read(cx).cancel(&stop_id))
                    .into_any_element(),
            ]
        }
        None => {
            let tools = tools.clone();
            vec![widgets::primary_button(
                &format!("needed-{id}-install"),
                t!("Install {name}", name = name),
                Some(IconName::Download),
            )
            .tooltip(hint)
            .on_click(move |_, window, cx| {
                let id = tool_id.clone();
                tools.update(cx, |tools, cx| tools.install(id, window, cx));
            })
            .into_any_element()]
        }
    };
    Some(
        col()
            .id(widgets::id(&["settings-needed", id]))
            .role(Role::Group)
            .aria_label(why.clone())
            .test_support()
            .m(space(3.))
            .gap(space(2.5))
            .px(space(4.))
            .py(space(3.))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.border)
            .bg(t.primary.opacity(0.05))
            .child(
                row_start()
                    .w_full()
                    .gap(space(3.))
                    .child(icon(icon_name).size_4().flex_none().text_color(t.primary))
                    .child(
                        div()
                            .flex_1()
                            .min_w(space(32.))
                            .text_size(theme::size::SM)
                            .text_start()
                            .child(why),
                    )
                    .child(row_start().flex_none().gap(space(1.5)).children(controls)),
            )
            .children(busy.as_ref().map(|fetch| progress_bar(&format!("needed-{id}"), fetch, cx)))
            .into_any_element(),
    )
}
