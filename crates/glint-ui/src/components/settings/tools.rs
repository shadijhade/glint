//! Settings → Tools: the programs Glint drives, and the meaning models (`ToolsSection`, `ToolRow`,
//! `MeaningRow` and `MultilingualRow` of SettingsView.tsx; the state of tools.ts and meaning.ts).
//!
//! rg, fd and pdu come with Glint; the others install when a feature needs them, with winget or from the
//! tool's own release. Clouds also reads this model (is rclone there?) and draws [`needed`], the card that
//! offers to install a missing tool (`ToolNeeded`).

use super::backend::{Backend, Fetch, ToolInfo};
use super::widgets::{self, bad, good};
use crate::kit::prelude::*;
use crate::{t, theme};
use glint_model::format::format_size;
use std::collections::HashMap;
use std::sync::Arc;

/// "Downloading 2.1 MB of 7.4 MB…", or what winget is doing (`describeProgress`).
pub fn describe_progress(fetch: &Fetch) -> SharedString {
    if fetch.total == 0 {
        return if fetch.stage.ends_with('…') {
            fetch.stage.clone().into()
        } else {
            format!("{}…", fetch.stage.trim_end_matches('.')).into()
        };
    }
    t!(
        "{stage} {done} of {total}…",
        stage = fetch.stage,
        done = format_size(fetch.done),
        total = format_size(fetch.total)
    )
}

/// What Install will do, for its tooltip (`installHint`).
pub fn install_hint(tool: &ToolInfo) -> SharedString {
    if tool.source == "release" {
        let version = tool.pinned.clone().unwrap_or_default();
        t!(
            "Downloads {name} {version} from its GitHub release and checks it",
            name = tool.name,
            version = version
        )
        .replace("  ", " ")
        .into()
    } else {
        t!("Installs {name} with winget", name = tool.name)
    }
}

/// The tools, and the installs under way.
pub struct Tools {
    backend: Arc<dyn Backend>,
    tools: Vec<ToolInfo>,
    /// Installs and removals running now, by tool id.
    busy: HashMap<String, Fetch>,
    checking: bool,
}

impl Tools {
    /// Looks for every tool at once (`useEffect(check)`).
    pub fn new(backend: Arc<dyn Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            backend,
            tools: Vec::new(),
            busy: HashMap::new(),
            checking: false,
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
            this.update(cx, |this, cx| {
                this.checking = false;
                if let Ok(tools) = tools {
                    this.tools = tools;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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

    /// One program Glint drives: its version, where it came from, and Install, Stop or Remove (`ToolRow`).
    fn tool_row(&self, tool: &ToolInfo, cx: &mut Context<Self>, t: &theme::Tokens) -> AnyElement {
        let slot = format!("tool-{}", tool.id);
        let where_from = match tool.origin.as_deref() {
            Some("downloaded") => t!("Downloaded by Glint"),
            Some("installed") => t!("Installed on this PC"),
            Some(_) => t!("Comes with Glint"),
            None => t!("Not installed"),
        };
        let outdated =
            tool.origin.as_deref() == Some("downloaded") && tool.pinned.is_some() && tool.version != tool.pinned;
        let label: SharedString = match &tool.version {
            Some(version) => format!("{} {}", tool.name, version).into(),
            None => tool.name.clone().into(),
        };
        let hint: SharedString = format!("{} · {}", tool.used_for, where_from).into();
        let controls: Vec<AnyElement> = match self.busy.get(&tool.id) {
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
                if tool.origin.is_none() || outdated {
                    let id = tool.id.clone();
                    let label = match (&tool.origin, &tool.pinned) {
                        (Some(_), Some(pinned)) => t!("Update to {version}", version = pinned),
                        _ => t!("Install"),
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
                    let mut button = widgets::button(&format!("{slot}-remove"), t!("Remove"), None)
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
        widgets::setting_row(label, Some(hint), controls, t).into_any_element()
    }
}

impl Render for Tools {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let mut rows: Vec<AnyElement> = Vec::new();
        if self.tools.is_empty() {
            rows.push(
                div()
                    .id("settings-looking-for-tools")
                    .role(Role::Status)
                    .aria_label(t!("Looking for them…"))
                    .test_support()
                    .px(space(4.))
                    .py(space(3.))
                    .text_size(theme::size::SM)
                    .text_color(t.muted_foreground)
                    .child(t!("Looking for them…"))
                    .into_any_element(),
            );
        }
        let tools = self.tools.clone();
        for tool in &tools {
            rows.push(self.tool_row(tool, cx, &t));
        }
        rows.push(
            widgets::note(
                t!("ripgrep, fd and pdu come with Glint. Other tools install when a feature needs them, for your account only: with winget, or from the tool's own GitHub release, which Glint checks against the checksum it knows for it before unpacking anything."),
                &t,
            )
            .into_any_element(),
        );
        let checking = self.checking;
        let refresh = widgets::quiet_button("tools-refresh", t!("Refresh"))
            .icon(IconName::RefreshCw)
            .loading(checking)
            .disabled(checking)
            .tooltip(t!("Look for each tool again"))
            .on_click(cx.listener(|this, _, window, cx| this.refresh(window, cx)));
        widgets::section("tools", t!("Tools"), Some(refresh.into_any_element()), rows, &t)
    }
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
    let controls: Vec<AnyElement> = match model.busy(id) {
        Some(fetch) => {
            let tools = tools.clone();
            let stop_id = tool_id.clone();
            vec![
                div()
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
        row_start()
            .id(widgets::id(&["settings-needed", id]))
            .role(Role::Group)
            .aria_label(why.clone())
            .test_support()
            .m(space(3.))
            .gap(space(3.))
            .px(space(4.))
            .py(space(3.))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.border)
            .bg(t.primary.opacity(0.05))
            .child(icon(icon_name).size_4().flex_none().text_color(t.primary))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(theme::size::SM)
                    .text_start()
                    .child(why),
            )
            .children(controls)
            .into_any_element(),
    )
}
