//! Settings → AI agents: how agents that speak the Model Context Protocol are told to start Glint's server
//! (mcp.rs), Windows' own agent registry (odr.exe), and what agents did through Glint, with Undo
//! (`AgentsSection`, `WindowsAgentsRow` and `AgentActions` of SettingsView.tsx).

use super::backend::{Backend, McpSetup};
use super::widgets::{self, bad, good};
use crate::kit::prelude::*;
use crate::{t, theme};
use glint_core::agents::Action;
use glint_model::format::format_date;
use std::sync::Arc;

/// How many of the latest actions are listed.
const SHOWN: usize = 20;

/// Which setup text a row copies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Agent {
    ClaudeCode,
    VsCode,
    Json,
}

impl Agent {
    const ALL: [Agent; 3] = [Agent::ClaudeCode, Agent::VsCode, Agent::Json];

    fn label(self) -> SharedString {
        match self {
            Agent::ClaudeCode => t!("Claude Code"),
            Agent::VsCode => t!("VS Code"),
            Agent::Json => t!("Claude Desktop, Cursor and others"),
        }
    }

    fn hint(self) -> SharedString {
        match self {
            Agent::ClaudeCode => t!("Run this in a terminal"),
            Agent::VsCode => t!("Run this in PowerShell: Copilot's agent mode can then use Glint"),
            Agent::Json => t!("Add this to the mcpServers in their settings"),
        }
    }

    fn slot(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude-code",
            Agent::VsCode => "vscode",
            Agent::Json => "json",
        }
    }

    fn text(self, setup: &McpSetup) -> &str {
        match self {
            Agent::ClaudeCode => &setup.claude_code,
            Agent::VsCode => &setup.vscode,
            Agent::Json => &setup.json,
        }
    }
}

/// The AI agents card.
pub struct Agents {
    backend: Arc<dyn Backend>,
    setup: Option<McpSetup>,
    registry_available: bool,
    actions: Vec<Action>,
}

impl Agents {
    /// Asks for the setup texts, whether Windows has an agent registry, and what agents did.
    pub fn new(backend: Arc<dyn Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let setup = backend.mcp_setup();
        let available = backend.agents_registry_available();
        let this = Self {
            backend,
            setup: None,
            registry_available: false,
            actions: Vec::new(),
        };
        cx.spawn_in(window, async move |this, cx| {
            let (setup, available) = (setup.await, available.await);
            this.update(cx, |this, cx| {
                this.setup = setup.ok();
                this.registry_available = available.unwrap_or(false);
                cx.notify();
            })
            .ok();
        })
        .detach();
        let mut this = this;
        this.refresh_actions(window, cx);
        this
    }

    /// What agents did, newest first.
    pub fn actions(&self) -> &[Action] {
        &self.actions
    }

    /// The setup texts, once core has made them.
    pub fn setup(&self) -> Option<&McpSetup> {
        self.setup.as_ref()
    }

    fn refresh_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let asked = self.backend.agent_actions();
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update(cx, |this, cx| {
                if let Ok(actions) = answer {
                    this.actions = actions;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn register(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let asked = self.backend.agents_register();
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |_, window, cx| match answer {
                Ok(said) => good(
                    window,
                    cx,
                    t!("Glint is in Windows' agent registry"),
                    Some(said).filter(|s| !s.is_empty()).map(Into::into),
                ),
                Err(reason) => bad(window, cx, t!("Couldn't add Glint to Windows' agent registry"), reason),
            })
            .ok();
        })
        .detach();
    }

    fn undo(&mut self, id: u64, summary: String, window: &mut Window, cx: &mut Context<Self>) {
        let asked = self.backend.agent_undo(id);
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |this, window, cx| match answer {
                Ok(()) => {
                    good(window, cx, t!("Undone"), Some(summary.into()));
                    this.refresh_actions(window, cx);
                }
                Err(reason) => bad(window, cx, t!("Couldn't undo it"), reason),
            })
            .ok();
        })
        .detach();
    }
}

impl Render for Agents {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let mono = theme::mono_font(cx);
        let language = crate::i18n::language();
        let mut rows: Vec<AnyElement> = vec![
            widgets::note(
                format!(
                    "{} {}",
                    t!("AI agents that speak the Model Context Protocol can ask Glint to find files by name on every drive in milliseconds (with instant search on), measure folders (as a map, in agents that show apps), list the biggest files, say what was moved or deleted lately and search text inside files."),
                    t!("They can also ask to move, rename, make folders and recycle: Glint asks you first each time, in a Windows dialog naming exactly what will happen, and lists what was done below, with Undo. Once told how, the agent starts Glint for itself, and sees the names and text it asks for.")
                ),
                &t,
            )
            .into_any_element(),
            widgets::setting_row(
                t!("Windows' agent registry"),
                Some(if self.registry_available {
                    t!("Adds Glint to the agents Windows itself knows (odr.exe), so its agents and the apps using them find Glint without setup.")
                } else {
                    t!("Newer Windows builds have one (odr.exe): then Glint can be added to it here.")
                }),
                vec![widgets::button("agents-register", t!("Add Glint"), None)
                    .disabled(!self.registry_available)
                    .on_click(cx.listener(|this, _, window, cx| this.register(window, cx)))
                    .into_any_element()],
                &t,
            )
            .into_any_element(),
        ];
        for agent in Agent::ALL {
            let text = self.setup.as_ref().map(|s| agent.text(s).to_string());
            let copied = text.clone();
            let label = agent.label();
            let what = t!("{name}'s setup", name = label);
            rows.push(
                col()
                    .px(space(4.))
                    .py(space(3.))
                    .child(
                        row_start()
                            .justify_between()
                            .gap(space(4.))
                            .child(
                                col()
                                    .min_w_0()
                                    .align_start()
                                    .child(div().text_size(theme::size::SM).text_start().child(agent.label()))
                                    .child(
                                        div()
                                            .mt(space(0.5))
                                            .text_size(theme::size::XS)
                                            .text_start()
                                            .text_color(t.muted_foreground)
                                            .child(agent.hint()),
                                    ),
                            )
                            .child(
                                widgets::button(
                                    &format!("agents-copy-{}", agent.slot()),
                                    t!("Copy"),
                                    Some(IconName::ClipboardCopy),
                                )
                                .disabled(text.is_none())
                                .accessibility_label(t!("Copy {what}", what = what))
                                .on_click(move |_, window, cx| {
                                    if let Some(text) = &copied {
                                        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                                        good(window, cx, t!("Copied {what}", what = what), None);
                                    }
                                }),
                            ),
                    )
                    .children(text.map(|text| {
                        div()
                            .id(widgets::id(&["settings-agent-text", agent.slot()]))
                            .role(Role::Code)
                            .aria_label(agent.label())
                            .test_support()
                            .mt(space(2.))
                            .px(space(2.))
                            .py(space(1.5))
                            .rounded(theme::size::RADIUS_SM)
                            .bg(t.muted)
                            .overflow_x_scroll()
                            .whitespace_nowrap()
                            .text_start()
                            .font_family(mono.clone())
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground)
                            .child(text)
                    }))
                    .into_any_element(),
            );
        }
        if !self.actions.is_empty() {
            rows.push(
                col()
                    .id("settings-agent-actions")
                    .role(Role::List)
                    .aria_label(t!("What agents did"))
                    .test_support()
                    .gap(space(1.))
                    .px(space(4.))
                    .pb(space(3.))
                    .child(div().text_size(theme::size::SM).child(t!("What agents did")))
                    .children(self.actions.iter().take(SHOWN).map(|action| {
                        let id = action.id;
                        let summary = action.summary.clone();
                        let line = format!("{} · {}", format_date(action.at, None, language), action.summary);
                        let items = action
                            .items
                            .iter()
                            .map(|(from, to)| format!("{from} → {}", to.as_deref().unwrap_or("—")))
                            .collect::<Vec<_>>()
                            .join("\n");
                        row_start()
                            .id(("settings-agent-action", id as usize))
                            .role(Role::ListItem)
                            .aria_label(SharedString::from(line.clone()))
                            .test_support()
                            .justify_between()
                            .gap(space(3.))
                            .text_size(theme::size::XS)
                            .child(
                                div()
                                    .id(("settings-agent-action-text", id as usize))
                                    .min_w_0()
                                    .truncate()
                                    .when(action.undone, |this| this.line_through().text_color(t.muted_foreground))
                                    .tooltip(move |window, cx| tooltip(SharedString::from(items.clone()), window, cx))
                                    .child(line),
                            )
                            .when(!action.undone, |this| {
                                this.child(
                                    widgets::quiet_button(&format!("agents-undo-{id}"), t!("Undo"))
                                        .accessibility_label(t!("Undo {what}", what = action.summary))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.undo(id, summary.clone(), window, cx)
                                        })),
                                )
                            })
                    }))
                    .into_any_element(),
            );
        }
        widgets::section("ai-agents", t!("AI agents"), None, rows, &t)
    }
}
