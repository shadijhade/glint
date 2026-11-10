//! Media files: convert, trim, extract, subtitles (MediaDialog.tsx).
//!
//! Ported from src/components/explorer/MediaDialog.tsx, with the parts of ActionParts.tsx, actions.ts, studio.ts and
//! tools.ts it used. **[`parts`] is shared:** the song, metadata and media-tools dialogs (SongDialog.tsx,
//! MetadataDialog.tsx, MediaTools.tsx) build their runs, choosers, progress, results and buttons from it, so the
//! four files are one family (see the note at [`parts`]).
//!
//! | TypeScript                                     | Here                                                      |
//! |------------------------------------------------|-----------------------------------------------------------|
//! | `openMedia(entries, mode)`                     | [`open`] (the menus' `Media { paths, tool }`)             |
//! | `MediaDialog` / `MediaModal`                   | [`MediaDialog`]                                           |
//! | `mediaProbe`, `mediaEncoders`, `mediaRun`, `mediaCancel`, `media_ai_*` | [`Backend`] (a fake in the tests)         |
//! | `isVideoFile`, `isAudioFile`, `isMediaFile`    | [`is_video_file`], [`is_audio_file`], [`is_media_file`]   |
//! | `seconds`, `clock`, `encoderName`              | [`seconds`], [`clock`], [`encoder_name`]                  |
//! | `last` (the choices of last time)              | `LAST`, kept while Glint runs                             |
//! | `useActionRun`, `WhereChooser`, `RunProgress`, `RunResult`, `RunFooter`, `Choices`, `ToolNeeded` | [`parts`]        |
//!
//! **How the coordinator wires it.** `MenuEvent::Media { paths, tool }` (the Media submenu's items and the
//! actions Explorer.tsx offered): call `dialogs::media::open(window, cx, paths, tool)`. `MediaTool::Convert` and
//! `Gif` take the videos among the paths, the others the videos and recordings; when none is left it says so in
//! a notification, as the TypeScript's toast did. The menus have already checked that the paths are on disk (not
//! in an archive, closed, online-only or in the Recycle Bin).
//!
//! NOTE(port): the TypeScript's drop-downs (Height, GIF width, Frames) are rows of choices here: the kit has no
//! drop-down, and five or six short options read as well as a list does.

use crate::components::menus::MediaTool;
use crate::components::settings::tools::{self, Tools};
use crate::components::settings::{Backend as SettingsBackend, CoreBackend as SettingsCoreBackend};
use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use glint_core::actions::{Output, Progress, Report};
use glint_core::host::Manager as _;
use glint_core::media_tools::{AudioFormat, Encoders, Probe, Quality, Task, VideoFormat};
use glint_model::format::{format_count, format_size, plural};
use glint_model::paths::{base_name, parent_path};
use parts::{Choice, Choices, Fut, Run, ToolGate, Where, WhereChooser};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

// ---- Shared by the four media dialogs ----------------------------------------------------------------------

/// What the media, song, metadata and media-tools dialogs share: ActionParts.tsx (`useActionRun`,
/// `WhereChooser`, `RunProgress`, `RunResult`, `RunFooter`, `Choices`), `offerActionUndo` and `badOutput` of
/// actions.ts, `ToolNeeded` and `revealInApp`.
///
/// TODO(coordinator): this belongs in `dialogs/action_parts.rs`, where the convert, shrink, PDF and data dialogs
/// can use it too. It is inline here because those files are not mine. Until it moves, `dialog-song`,
/// `dialog-metadata` and `dialog-media-tools` need `dialog-media` (they `use super::media::parts`).
pub mod parts {
    use super::*;
    use crate::explorer;
    use gpui_kit::px as pixels;
    use std::rc::Rc;

    pub use crate::dialogs::vault::{bad_output, Fut, Run, Where};

    // ---- ToolNeeded ----

    /// The tools the dialog's install card reads (`useTool`, `ToolNeeded`): none in tests that don't care, and
    /// then every tool counts as there.
    pub struct ToolGate {
        tools: Option<Entity<Tools>>,
        /// Tests: the tools are never known, as before the first look.
        #[cfg(test)]
        never: bool,
    }

    impl ToolGate {
        /// Looks for the tools through `backend` (none: nothing to wait for).
        pub fn new<V: 'static>(
            backend: Option<Arc<dyn SettingsBackend>>,
            window: &mut Window,
            cx: &mut Context<V>,
        ) -> Self {
            let tools = backend.map(|backend| cx.new(|cx| Tools::new(backend, window, cx)));
            Self {
                tools,
                #[cfg(test)]
                never: false,
            }
        }

        /// Tests: a gate whose tools are never found, so [`ToolGate::ready`] stays false (the state before the
        /// first look for the tools, or with one missing).
        #[cfg(test)]
        pub fn never() -> Self {
            Self {
                tools: None,
                never: true,
            }
        }

        /// Runs `on_change` whenever the tools are looked for again or one is installed. Keep the subscription.
        pub fn observe<V: 'static>(
            &self,
            cx: &mut Context<V>,
            on_change: impl Fn(&mut V, &mut Context<V>) + 'static,
        ) -> Option<Subscription> {
            let tools = self.tools.as_ref()?;
            Some(cx.observe(tools, move |this, _, cx| on_change(this, cx)))
        }

        /// Whether the tool is there (`!!tool?.origin`). Before the tools are known it is not.
        pub fn ready(&self, id: &str, cx: &App) -> bool {
            #[cfg(test)]
            if self.never {
                return false;
            }
            match &self.tools {
                None => true,
                Some(tools) => tools.read(cx).tool(id).is_some_and(|tool| tool.origin.is_some()),
            }
        }

        /// The card that says a tool is needed and installs it, or nothing once it is there.
        pub fn card(&self, id: &str, why: SharedString, icon: IconName, cx: &App) -> Option<AnyElement> {
            tools::needed(self.tools.as_ref()?, id, why, icon, cx)
        }
    }

    // ---- Choices ----

    /// One choice of a [`Choices`] row.
    pub struct Choice<T> {
        value: T,
        label: SharedString,
        hint: Option<SharedString>,
        disabled: bool,
    }

    impl<T> Choice<T> {
        /// A choice with its name.
        pub fn new(value: T, label: impl Into<SharedString>) -> Self {
            Self {
                value,
                label: label.into(),
                hint: None,
                disabled: false,
            }
        }

        /// A line under the name.
        pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
            self.hint = Some(hint.into());
            self
        }

        /// Can't be picked.
        pub fn disabled(mut self, disabled: bool) -> Self {
            self.disabled = disabled;
            self
        }
    }

    /// What picking a choice does.
    type OnPick<T> = Rc<dyn Fn(&T, &mut Window, &mut App)>;

    /// A row of choices, one picked, as Convert's formats are (`Choices`): a radio group of buttons with a name
    /// and a hint each, `columns` to a row, in the reading direction.
    #[derive(IntoElement)]
    pub struct Choices<T: Copy + PartialEq + 'static> {
        id: &'static str,
        label: SharedString,
        value: T,
        options: Vec<Choice<T>>,
        columns: usize,
        on_pick: OnPick<T>,
    }

    impl<T: Copy + PartialEq + 'static> Choices<T> {
        /// The group `label`, with `id` for its buttons (`(id, index)`), `value` picked.
        pub fn new(id: &'static str, label: impl Into<SharedString>, value: T) -> Self {
            Self {
                id,
                label: label.into(),
                value,
                options: Vec::new(),
                columns: 2,
                on_pick: Rc::new(|_, _, _| {}),
            }
        }

        /// What can be picked.
        pub fn options(mut self, options: Vec<Choice<T>>) -> Self {
            self.options = options;
            self
        }

        /// How many to a row (the TypeScript's `columns`, 2 unless said).
        pub fn columns(mut self, columns: usize) -> Self {
            self.columns = columns.max(1);
            self
        }

        /// Runs with the value picked.
        pub fn on_pick(mut self, on_pick: impl Fn(&T, &mut Window, &mut App) + 'static) -> Self {
            self.on_pick = Rc::new(on_pick);
            self
        }
    }

    impl<T: Copy + PartialEq + 'static> RenderOnce for Choices<T> {
        fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
            let t = theme::tokens(cx).clone();
            let columns = self.columns.max(1);
            let id = self.id;
            let mut options = self.options.into_iter().enumerate().peekable();
            let mut rows = Vec::new();
            while options.peek().is_some() {
                let mut row = row_start().items_stretch().gap(space(1.5));
                let mut filled = 0;
                for (ix, option) in options.by_ref().take(columns) {
                    filled += 1;
                    let chosen = option.value == self.value;
                    let value = option.value;
                    let on_pick = self.on_pick.clone();
                    let name = match &option.hint {
                        Some(hint) => format!("{}. {hint}", option.label),
                        None => option.label.to_string(),
                    };
                    row = row.child(
                        Button::new((id, ix))
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .color(if chosen {
                                        t.primary.opacity(0.1)
                                    } else {
                                        t.background.opacity(0.)
                                    })
                                    .foreground(t.foreground)
                                    .hover(if chosen { t.primary.opacity(0.15) } else { t.accent })
                                    .active(t.accent),
                            )
                            .flex_1()
                            .min_w_0()
                            .h_auto()
                            .border_1()
                            .border_color(if chosen { t.primary } else { t.border })
                            .px(space(2.))
                            .py(space(1.5))
                            .role(Role::RadioButton)
                            .toggled(chosen)
                            .disabled(option.disabled)
                            .accessibility_label(name)
                            .on_click(move |_, window, cx| on_pick(&value, window, cx))
                            .child(
                                col()
                                    .w_full()
                                    .align_start()
                                    .text_start()
                                    .child(div().font_weight(FontWeight::MEDIUM).child(option.label))
                                    .when_some(option.hint, |this, hint| {
                                        this.child(
                                            div()
                                                .max_w_full()
                                                .truncate()
                                                .text_size(theme::size::XXS)
                                                .text_color(t.muted_foreground)
                                                .child(hint),
                                        )
                                    }),
                            ),
                    );
                }
                // A short last row keeps its buttons as wide as the others.
                for _ in filled..columns {
                    row = row.child(div().flex_1().min_w_0());
                }
                rows.push(row);
            }
            col()
                .id(id)
                .role(Role::RadioGroup)
                .aria_label(self.label)
                .test_support()
                .gap(space(1.5))
                .children(rows)
        }
    }

    // ---- Where what is made goes ----

    /// "Where": next to each file, into a folder there, or in their place (`WhereChooser`), with the two boxes.
    /// An entity of its own: the dialog reads [`WhereChooser::output`] and re-draws when it changes.
    pub struct WhereChooser {
        slot: &'static str,
        kind: WhereKind,
        suffix: Entity<InputState>,
        folder: Entity<InputState>,
        noun: SharedString,
        note: Option<SharedString>,
        replace: bool,
        _subscriptions: Vec<Subscription>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum WhereKind {
        Beside,
        Folder,
        Replace,
    }

    impl WhereChooser {
        /// A chooser starting at `initial`, its elements named `slot`-where…
        pub fn new(slot: &'static str, initial: &Where, window: &mut Window, cx: &mut Context<Self>) -> Self {
            let (kind, suffix, folder) = match initial {
                Where::Beside { suffix } => (WhereKind::Beside, suffix.clone(), "Converted".to_string()),
                Where::Folder { name } => (WhereKind::Folder, String::new(), name.clone()),
                Where::Replace => (WhereKind::Replace, String::new(), "Converted".to_string()),
            };
            let suffix = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(t!("nothing"))
                    .default_value(suffix)
            });
            let folder = cx.new(|cx| InputState::new(window, cx).default_value(folder));
            let subscriptions = [&suffix, &folder]
                .into_iter()
                .map(|input| {
                    cx.subscribe_in(input, window, |_, _, event: &InputEvent, _, cx| {
                        if matches!(event, InputEvent::Change) {
                            cx.notify();
                        }
                    })
                })
                .collect();
            Self {
                slot,
                kind,
                suffix,
                folder,
                noun: t!("file"),
                note: None,
                replace: true,
                _subscriptions: subscriptions,
            }
        }

        /// What the chooser says now.
        pub fn output(&self, cx: &App) -> Where {
            match self.kind {
                WhereKind::Beside => Where::Beside {
                    suffix: self.suffix.read(cx).value().to_string(),
                },
                WhereKind::Folder => Where::Folder {
                    name: self.folder.read(cx).value().to_string(),
                },
                WhereKind::Replace => Where::Replace,
            }
        }

        /// Whether the output can't be used (`badOutput`).
        pub fn bad(&self, cx: &App) -> bool {
            bad_output(&self.output(cx))
        }

        /// The suffix box, for focusing and tests.
        pub fn suffix_input(&self) -> &Entity<InputState> {
            &self.suffix
        }

        /// "Next to each {noun}…" and "the {noun}s moved to the Recycle Bin".
        pub fn set_noun(&mut self, noun: SharedString, cx: &mut Context<Self>) {
            if self.noun != noun {
                self.noun = noun;
                cx.notify();
            }
        }

        /// The line under the choices (the default says a taken name gets "(2)").
        pub fn set_note(&mut self, note: SharedString, cx: &mut Context<Self>) {
            self.note = Some(note);
            cx.notify();
        }

        /// Whether "In their place" is offered. Picked while it goes away, the choice falls back to "Next to".
        pub fn set_replace_allowed(&mut self, allowed: bool, cx: &mut Context<Self>) {
            self.replace = allowed;
            if !allowed && self.kind == WhereKind::Replace {
                self.kind = WhereKind::Beside;
            }
            cx.notify();
        }

        /// Changes the suffix when "Next to" is the choice (the dialog's mode asks for another).
        ///
        /// NOTE(port): the TypeScript changed its output but left the box showing the old suffix (the box kept
        /// its own state from when it was made). Here the box shows what is used.
        pub fn set_suffix(&mut self, suffix: &str, window: &mut Window, cx: &mut Context<Self>) {
            if self.kind == WhereKind::Beside {
                self.suffix.update(cx, |input, cx| input.set_value(suffix, window, cx));
                cx.notify();
            }
        }

        fn radio(&self, ix: usize, kind: WhereKind, label: SharedString, cx: &mut Context<Self>) -> impl IntoElement {
            let chosen = self.kind == kind;
            Button::new((self.slot, ix))
                .ghost()
                .compact()
                .role(Role::RadioButton)
                .toggled(chosen)
                .accessibility_label(label)
                .icon(if chosen { IconName::CircleDot } else { IconName::Circle })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.kind = kind;
                    cx.notify();
                }))
        }
    }

    impl Render for WhereChooser {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let t = theme::tokens(cx).clone();
            let bad = self.bad(cx);
            let small_box = |input: &Entity<InputState>, aria: SharedString, wide: f32, on: bool| {
                div()
                    .w(space(wide))
                    .flex_none()
                    .when(on && bad, |d| {
                        d.border_1().border_color(t.destructive).rounded(theme::size::RADIUS)
                    })
                    .child(Input::new(input).aria_label(aria))
            };
            let beside = t!("Next to each {noun}, its name and", noun = self.noun.clone());
            let folder = t!("Into a folder next to them:");
            let replace = t!(
                "In their place, the {noun}s moved to the Recycle Bin",
                noun = self.noun.clone()
            );
            let note = self
                .note
                .clone()
                .unwrap_or_else(|| t!("A name that's taken gets “(2)”, and Ctrl+Z takes it all back."));
            col()
                .id(SharedString::from(format!("{}-group", self.slot)))
                .role(Role::RadioGroup)
                .aria_label(t!("Where"))
                .test_support()
                .gap(space(1.5))
                .child(
                    div()
                        .text_size(theme::size::XS)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.muted_foreground)
                        .child(t!("Where")),
                )
                .child(
                    align_row(2.)
                        .child(self.radio(0, WhereKind::Beside, beside.clone(), cx))
                        .child(div().flex_none().child(beside))
                        .child(small_box(
                            &self.suffix,
                            t!("Added to the name"),
                            28.,
                            self.kind == WhereKind::Beside,
                        )),
                )
                .child(
                    align_row(2.)
                        .child(self.radio(1, WhereKind::Folder, folder.clone(), cx))
                        .child(div().flex_none().child(folder))
                        .child(small_box(
                            &self.folder,
                            t!("Folder name"),
                            36.,
                            self.kind == WhereKind::Folder,
                        )),
                )
                .when(self.replace, |this| {
                    this.child(
                        align_row(2.)
                            .child(self.radio(2, WhereKind::Replace, replace.clone(), cx))
                            .child(div().flex_1().min_w_0().child(replace)),
                    )
                })
                .child(
                    div()
                        .ps(6.)
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(note),
                )
        }
    }

    // ---- A run under way, its result and its buttons ----

    /// The progress a run starts at.
    pub fn starting(total: usize) -> Progress {
        Progress {
            done: 0,
            total,
            current: String::new(),
            part: 0.,
        }
    }

    /// How far a run is, as one line: "Working on 2 of 5 · clip.mp4 · 40%" (`RunProgress`).
    pub fn progress_text(verb: &str, progress: &Progress) -> String {
        let mut text = verb.to_string();
        if progress.total > 1 {
            let at = (progress.done + 1).min(progress.total);
            text.push_str(&format!(" {} of {}", format_count(at), format_count(progress.total)));
        }
        if !progress.current.is_empty() {
            text.push_str(&format!(" · {}", progress.current));
        }
        if progress.part > 0. {
            text.push_str(&format!(" · {}%", (progress.part * 100.).round()));
        }
        text
    }

    /// The share of the whole run done: the jobs finished and how far the one under way is.
    pub fn whole(progress: &Progress) -> f32 {
        if progress.total == 0 {
            0.
        } else {
            ((progress.done as f32 + progress.part.min(0.999)) / progress.total as f32).clamp(0., 1.)
        }
    }

    /// A run under way: which of how many, and how far the one being made is (`RunProgress`).
    pub fn progress_view(slot: &'static str, verb: &str, progress: &Progress, t: &theme::Tokens) -> impl IntoElement {
        let text: SharedString = progress_text(verb, progress).into();
        col()
            .id(SharedString::from(format!("{slot}-progress")))
            .role(Role::ProgressIndicator)
            .aria_label(text.clone())
            .test_support()
            .gap(space(2.))
            .py(space(6.))
            .child(
                align_row(2.)
                    .text_color(t.muted_foreground)
                    .child(Spinner::new())
                    .child(div().flex_1().min_w_0().truncate().child(isolate(text))),
            )
            .child(
                row_start()
                    .h(space(1.5))
                    .w_full()
                    .overflow_hidden()
                    .rounded_full()
                    .bg(t.muted)
                    .child(
                        div()
                            .h_full()
                            .w(gpui_kit::relative(whole(progress)))
                            .rounded_full()
                            .bg(t.primary),
                    ),
            )
    }

    /// "12 MB → 4.1 MB (−66%)": the sizes before and after a run, when they are known.
    pub fn sizes_line(before: u64, after: u64) -> Option<String> {
        if before == 0 {
            return None;
        }
        // Math.round of the share, then its size: halves go up, the sign is shown by itself.
        let share = (100. * (before as f64 - after as f64) / before as f64 + 0.5)
            .floor()
            .abs();
        Some(format!(
            "{} → {} ({}{share}%)",
            format_size(before),
            format_size(after),
            if after <= before { "−" } else { "+" }
        ))
    }

    /// What a finished run did: made how many, the sizes before and after (when `sizes`), how long it took,
    /// and what failed (`RunResult`).
    pub fn result_view(
        slot: &'static str,
        noun: &str,
        sizes: bool,
        report: &Report,
        t: &theme::Tokens,
    ) -> impl IntoElement {
        let made = report.made.len();
        let mut headline = if made > 0 {
            t!("Made {what}", what = plural(made, noun)).to_string()
        } else {
            t!("Nothing was made").to_string()
        };
        if report.stopped {
            headline.push_str(&t!(" before it was stopped"));
        }
        let headline: SharedString = headline.into();
        let detail = if made > 0 {
            let took = glint_model::format::format_duration(report.elapsed_ms);
            match sizes.then(|| sizes_line(report.before, report.after)).flatten() {
                Some(sizes) => format!("{sizes} · {took}"),
                None => took,
            }
        } else {
            String::new()
        };
        let failed = report.failed.clone();
        col()
            .id(SharedString::from(format!("{slot}-result")))
            .role(Role::Status)
            .aria_label(headline.clone())
            .test_support()
            .gap(space(3.))
            .child(
                align_row(2.)
                    .items_start()
                    .child(
                        icon(IconName::CircleCheck)
                            .size_5()
                            .flex_none()
                            .text_color(if made > 0 { t.success } else { t.muted_foreground }),
                    )
                    .child(
                        col()
                            .align_start()
                            .child(div().font_weight(FontWeight::MEDIUM).child(headline))
                            .when(made > 0, |this| {
                                this.child(
                                    div()
                                        .text_size(theme::size::XS)
                                        .text_color(t.muted_foreground)
                                        .child(detail),
                                )
                            }),
                    ),
            )
            .when(!failed.is_empty(), |this| {
                let title = t!("{what} not done:", what = plural(failed.len(), "file"));
                this.child(
                    col()
                        .id(SharedString::from(format!("{slot}-failed")))
                        .role(Role::List)
                        .aria_label(title.clone())
                        .test_support()
                        .gap(space(0.5))
                        .rounded(theme::size::RADIUS)
                        .bg(t.warning.opacity(0.1))
                        .px(space(3.))
                        .py(space(2.))
                        .text_size(theme::size::XS)
                        .child(div().font_weight(FontWeight::MEDIUM).child(title))
                        .children(failed.iter().take(6).enumerate().map(|(ix, (path, why))| {
                            let line: SharedString = format!("{}: {why}", base_name(path)).into();
                            div()
                                .id((SharedString::from(format!("{slot}-failed")), ix))
                                .role(Role::ListItem)
                                .aria_label(line.clone())
                                .test_support()
                                .truncate()
                                .child(isolate(line))
                        }))
                        .when(failed.len() > 6, |this| {
                            this.child(t!("and {n} more", n = format_count(failed.len() - 6)))
                        }),
                )
            })
    }

    /// What a run that failed as a whole shows (`RunResult`'s error).
    pub fn error_view(
        slot: &'static str,
        failed_what: SharedString,
        reason: &SharedString,
        t: &theme::Tokens,
    ) -> impl IntoElement {
        col()
            .id(SharedString::from(format!("{slot}-error")))
            .role(Role::Alert)
            .aria_label(format!("{failed_what}. {reason}"))
            .test_support()
            .items_center()
            .gap(space(2.))
            .py(space(6.))
            .text_center()
            .child(icon(IconName::TriangleAlert).size_6().text_color(t.destructive))
            .child(div().font_weight(FontWeight::MEDIUM).child(failed_what))
            .child(
                div()
                    .max_w(space(112.))
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(reason.clone()),
            )
    }

    /// The dialog's buttons (`RunFooter`): Stop while it runs; Show and Close after; Cancel and the action
    /// before. The ids are `slot`-stop, -show, -close, -cancel and -start.
    pub struct Footer<V: 'static> {
        /// The dialog's name for ids.
        pub slot: &'static str,
        /// Where the run stands.
        pub run: Run,
        /// The action's name ("Convert").
        pub label: SharedString,
        /// Its icon.
        pub icon: IconName,
        /// Whether the action can be pressed now (the TypeScript's `!disabled`).
        pub can_start: bool,
        /// Starts the run.
        pub start: fn(&mut V, &mut Window, &mut Context<V>),
        /// Stops it (`onStop`).
        pub stop: fn(&mut V),
    }

    impl<V: 'static> Footer<V> {
        /// The row of buttons.
        pub fn render(self, t: &theme::Tokens, cx: &mut Context<V>) -> Div {
            let running = matches!(self.run, Run::Running(_));
            let finished = matches!(self.run, Run::Done(_) | Run::Failed(_));
            let first_made = match &self.run {
                Run::Done(report) => report.made.first().cloned(),
                _ => None,
            };
            let slot = self.slot;
            let (start, stop) = (self.start, self.stop);
            row_start()
                .justify_end()
                .gap(space(2.))
                .pt(space(3.))
                .border_t_1()
                .border_color(t.border)
                .when(running, |this| {
                    this.child(
                        Button::new(SharedString::from(format!("{slot}-stop")))
                            .outline()
                            .icon(IconName::Square)
                            .label(t!("Stop"))
                            .on_click(cx.listener(move |this, _, _, _| stop(this))),
                    )
                })
                .when_some(first_made, |this, made| {
                    this.child(
                        Button::new(SharedString::from(format!("{slot}-show")))
                            .outline()
                            .icon(IconName::FolderInput)
                            .label(t!("Show"))
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                reveal(&made, cx);
                            }),
                    )
                })
                .when(finished, |this| {
                    this.child(
                        Button::new(SharedString::from(format!("{slot}-close")))
                            .primary()
                            .label(t!("Close"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                })
                .when(!finished, |this| {
                    this.child(
                        Button::new(SharedString::from(format!("{slot}-cancel")))
                            .outline()
                            .label(t!("Cancel"))
                            .disabled(running)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new(SharedString::from(format!("{slot}-start")))
                            .primary()
                            .icon(self.icon)
                            .label(self.label)
                            .disabled(running || !self.can_start)
                            .on_click(cx.listener(move |this, _, window, cx| start(this, window, cx))),
                    )
                })
        }
    }

    // ---- Running ----

    /// A run to start: how many jobs, core's call, and its progress.
    pub struct Launch {
        /// How many jobs there are (the run's total until progress says).
        pub total: usize,
        /// Core's call.
        pub call: Fut<Report>,
        /// Its progress messages.
        pub progress: async_channel::Receiver<Progress>,
    }

    /// Starts a run (`useActionRun().start`): sets the dialog's [`Run`] (found by `state`) to running, keeps it
    /// up to date from the progress (at most once a frame), and when the call ends sets it to done or failed. A
    /// run that made something offers Undo, worded `undo_label(report)` (`offerActionUndo`).
    pub fn drive<V: 'static>(
        this: &mut V,
        window: &mut Window,
        cx: &mut Context<V>,
        launch: Launch,
        state: fn(&mut V) -> &mut Run,
        undo_label: impl FnOnce(&Report) -> String + 'static,
    ) {
        let Launch { total, call, progress } = launch;
        *state(this) = Run::Running(starting(total));
        cx.notify();
        // Progress: only the newest message each time the task wakes, so a chatty run redraws once.
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(mut latest) = progress.recv().await {
                while let Ok(newer) = progress.try_recv() {
                    latest = newer;
                }
                let alive = this.update(cx, |this, cx| {
                    let run = state(this);
                    if matches!(run, Run::Running(_)) {
                        *run = Run::Running(latest);
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let result = call.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(report) => {
                        if !report.made.is_empty() {
                            offer_undo(&report, &undo_label(&report), window, cx);
                        }
                        *state(this) = Run::Done(report);
                    }
                    Err(reason) => *state(this) = Run::Failed(reason.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The Undo offer of an action's notification.
    struct ActionUndoToast;

    /// Keeps how to take back what `report` made, for Ctrl+Z, and offers Undo in a notification
    /// (`offerActionUndo`): what it made is recycled and the files it replaced come back from the Recycle Bin.
    ///
    /// NOTE(port): the TypeScript took both back in one step. The explorer's undo steps hold one action each, so
    /// Ctrl+Z recycles what was made and the next one brings the originals back; the notification's Undo does
    /// both.
    pub fn offer_undo(report: &Report, what: &str, window: &mut Window, cx: &mut App) {
        if report.made.is_empty() {
            return;
        }
        let Some(explorer) = explorer::try_explorer(cx) else {
            return;
        };
        let back: Vec<explorer::MoveItem> = report
            .recycled
            .iter()
            .map(|r| explorer::MoveItem {
                path: r.bin.clone(),
                folder: parent_path(&r.path),
                name: base_name(&r.path).to_string(),
            })
            .collect();
        let made = report.made.clone();
        let (restore, recycle) = explorer.update(cx, |explorer, cx| {
            let restore =
                (!back.is_empty()).then(|| explorer.remember_undo(what, explorer::UndoAction::Restore(back), cx));
            (
                restore,
                explorer.remember_undo(what, explorer::UndoAction::Recycle(made), cx),
            )
        });
        let mut label = what.to_string();
        if let Some(first) = label.get(..1) {
            label = first.to_uppercase() + &label[1..];
        }
        let key = recycle.0 as usize;
        let note = Notification::success(SharedString::from(label))
            .id1::<ActionUndoToast>(key)
            .action(move |_, _, _| {
                let explorer = explorer.clone();
                Button::new("action-undo")
                    .label(t!("Undo"))
                    .on_click(move |_, window, cx| {
                        let first = explorer.update(cx, |e, cx| e.undo(Some(recycle), cx));
                        let explorer = explorer.clone();
                        cx.spawn(async move |cx| {
                            first.await;
                            if let Some(restore) = restore {
                                let second = explorer.update(cx, |e, cx| e.undo(Some(restore), cx));
                                second.await;
                            }
                        })
                        .detach();
                        window.remove_notification1::<ActionUndoToast>(key, cx);
                    })
            });
        window.push_notification(note, cx);
        // The web app's toast stayed 8 s.
        let handle = window.window_handle();
        cx.spawn(async move |cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(8)).await;
            let _ = handle.update(cx, |_, window, cx| {
                window.remove_notification1::<ActionUndoToast>(key, cx)
            });
        })
        .detach();
    }

    /// Shows `path` in its folder, selected (`revealInApp`).
    pub fn reveal(path: &str, cx: &mut App) {
        let Some(explorer) = explorer::try_explorer(cx) else {
            return;
        };
        let path = path.to_string();
        explorer.update(cx, |explorer, cx| {
            let options = explorer::NavOptions::default().selecting(path.clone());
            explorer.navigate(parent_path(&path), options, cx);
        });
    }

    // ---- Small pieces of layout ----

    /// A labelled row: a fixed-width label, then the content.
    pub fn field(label: SharedString, content: impl IntoElement, t: &theme::Tokens) -> Div {
        align_row(3.)
            .child(
                div()
                    .w(space(20.))
                    .flex_none()
                    .text_size(theme::size::XS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.muted_foreground)
                    .child(label),
            )
            .child(div().flex_1().min_w_0().child(content))
    }

    /// A small heading over a section (`h3 text-xs font-semibold text-muted-foreground`).
    pub fn heading(text: SharedString, t: &theme::Tokens) -> Div {
        div()
            .text_size(theme::size::XS)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(t.muted_foreground)
            .child(text)
    }

    /// A line of small, muted help text.
    pub fn note(text: impl Into<SharedString>, t: &theme::Tokens) -> Div {
        div()
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(text.into())
    }

    /// The folder line under a dialog's title: the folder they are in, or how many folders.
    pub fn folder_line(id: &'static str, files: &[String], t: &theme::Tokens) -> impl IntoElement {
        let text = crate::dialogs::vault::where_they_are(files);
        div()
            .id(id)
            .role(Role::Group)
            .aria_label(text.clone())
            .test_support()
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .truncate()
            .child(isolate(text))
    }

    /// The names of the distinct folders among `files`, lower-cased.
    pub fn folders_of(files: &[String]) -> HashSet<String> {
        files.iter().map(|f| parent_path(f).to_lowercase()).collect()
    }

    /// The height of a dialog's scrolling body: a share of the window, so a short window scrolls it.
    pub fn body_height(window: &Window) -> Pixels {
        (window.viewport_size().height * 0.72).max(pixels(240.))
    }

    /// The main window, which core's calls take as their owner, or why there is none.
    pub fn main_window(app: &glint_core::host::AppHandle) -> Result<glint_core::host::Window, String> {
        app.get_webview_window(crate::workspace::MAIN)
            .ok_or_else(|| t!("Glint's window isn't open").to_string())
    }

    /// Runs `future` on core's runtime and flattens the join error into the call's own.
    pub fn on_core<T: Send + 'static>(
        future: impl std::future::Future<Output = Result<T, String>> + Send + 'static,
    ) -> Fut<T> {
        let joined = core_bridge::run(future);
        Box::pin(async move { joined.await.and_then(|inner| inner) })
    }

    /// The tools backend core offers (the install cards), or none when core isn't started.
    pub fn core_tools(cx: &App) -> Option<Arc<dyn SettingsBackend>> {
        let core = cx.try_global::<core_bridge::Core>()?;
        Some(Arc::new(SettingsCoreBackend::new(core.app.clone())))
    }
}

// ---- What FFmpeg reads (studio.ts) ------------------------------------------------------------------------

/// Videos FFmpeg reads (`VIDEO_EXTS`).
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv", "ts", "mts", "m2ts", "mpg", "mpeg", "3gp", "ogv", "vob",
    "asf", "f4v",
];
/// Recordings FFmpeg reads (`AUDIO_EXTS`).
pub const AUDIO_EXTS: &[&str] = &[
    "mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "wma", "amr", "aiff", "aif", "ape", "mka", "ac3", "dts", "caf",
];

/// The lower-case extension of a file's name, "" when it has none.
fn ext_of(path: &str) -> String {
    match base_name(path).rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_lowercase(),
        _ => String::new(),
    }
}

/// Whether FFmpeg reads `path` as a video (`isVideoFile`, by its name; the menus have checked it is on disk).
pub fn is_video_file(path: &str) -> bool {
    VIDEO_EXTS.contains(&ext_of(path).as_str())
}

/// Whether FFmpeg reads `path` as a recording (`isAudioFile`).
pub fn is_audio_file(path: &str) -> bool {
    AUDIO_EXTS.contains(&ext_of(path).as_str())
}

/// A video or a recording (`isMediaFile`).
pub fn is_media_file(path: &str) -> bool {
    is_video_file(path) || is_audio_file(path)
}

/// What JavaScript's `Number(text)` makes of a piece of a time: an empty piece is 0, and what isn't a finite
/// number is nothing.
fn js_number(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return Some(0.);
    }
    text.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// "1:05" or "12.5" as seconds; `None` when it isn't one (`seconds`).
pub fn seconds(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut total = 0.;
    for part in text.split(':') {
        let n = js_number(part)?;
        if n < 0. {
            return None;
        }
        total = total * 60. + n;
    }
    Some(total)
}

/// 65.4 as "1:05" (`clock`).
pub fn clock(s: f64) -> String {
    let s = s.max(0.);
    format!("{}:{:02}", (s / 60.).floor() as u64, (s % 60.).floor() as u64)
}

/// What a hardware encoder is called, for people (`encoderName`).
pub fn encoder_name(encoder: Option<&str>) -> String {
    let Some(encoder) = encoder else {
        return String::new();
    };
    if encoder.ends_with("_nvenc") {
        "NVIDIA GPU".into()
    } else if encoder.ends_with("_qsv") {
        "Intel Quick Sync".into()
    } else if encoder.ends_with("_amf") {
        "AMD GPU".into()
    } else {
        match encoder {
            "libx264" => "x264".into(),
            "libx265" => "x265".into(),
            "libsvtav1" => "SVT-AV1".into(),
            other => other.into(),
        }
    }
}

// ---- Modes and the choices of last time -------------------------------------------------------------------

/// What the dialog makes (`MediaMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaMode {
    /// Another format, smaller.
    Convert,
    /// The sound alone.
    Audio,
    /// An animated GIF or WebP.
    Gif,
    /// An .srt of what is said.
    Subtitles,
    /// The volume evened out.
    Volume,
}

impl From<MediaTool> for MediaMode {
    fn from(tool: MediaTool) -> Self {
        match tool {
            MediaTool::Convert => MediaMode::Convert,
            MediaTool::Audio => MediaMode::Audio,
            MediaTool::Gif => MediaMode::Gif,
            MediaTool::Subtitles => MediaMode::Subtitles,
            MediaTool::Volume => MediaMode::Volume,
        }
    }
}

impl MediaMode {
    const ALL: [MediaMode; 5] = [
        MediaMode::Convert,
        MediaMode::Audio,
        MediaMode::Gif,
        MediaMode::Subtitles,
        MediaMode::Volume,
    ];

    /// The tab's name.
    fn label(self) -> SharedString {
        match self {
            MediaMode::Convert => t!("Convert & shrink"),
            MediaMode::Audio => t!("Sound only"),
            MediaMode::Gif => t!("GIF"),
            MediaMode::Subtitles => t!("Subtitles"),
            MediaMode::Volume => t!("Even volume"),
        }
    }

    fn icon(self) -> IconName {
        match self {
            MediaMode::Convert => IconName::Film,
            MediaMode::Audio => IconName::AudioLines,
            MediaMode::Gif => IconName::Clapperboard,
            MediaMode::Subtitles => IconName::Captions,
            MediaMode::Volume => IconName::Volume2,
        }
    }

    /// The action button's name.
    fn start_label(self) -> SharedString {
        match self {
            MediaMode::Convert => t!("Convert"),
            MediaMode::Audio => t!("Take the sound out"),
            MediaMode::Gif => t!("Make it"),
            MediaMode::Subtitles => t!("Make subtitles"),
            MediaMode::Volume => t!("Even it out"),
        }
    }

    /// Completes "Undid the …" and the Undo offer: "conversion of 2 files".
    fn undo_verb(self) -> SharedString {
        match self {
            MediaMode::Convert => t!("conversion"),
            MediaMode::Audio => t!("sound taken out"),
            MediaMode::Gif => t!("GIF"),
            MediaMode::Subtitles => t!("subtitles"),
            MediaMode::Volume => t!("volume evened out"),
        }
    }

    /// What "Next to each file" adds to a name to begin with.
    fn suffix(self) -> &'static str {
        if self == MediaMode::Convert {
            " (converted)"
        } else {
            ""
        }
    }
}

/// How good Convert makes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QualityKind {
    Good,
    High,
    Size,
}

/// The choices made in the dialog, kept for the next one while Glint runs (`last`).
#[derive(Clone, Debug, PartialEq)]
struct Picked {
    format: VideoFormat,
    quality: QualityKind,
    mb: f64,
    height: u32,
    audio: AudioFormat,
    gif_width: u32,
    fps: u32,
    webp: bool,
    translate: bool,
    music: bool,
}

impl Default for Picked {
    fn default() -> Self {
        Self {
            format: VideoFormat::H264,
            quality: QualityKind::Good,
            mb: 25.,
            height: 0,
            audio: AudioFormat::Same,
            gif_width: 480,
            fps: 12,
            webp: false,
            translate: false,
            music: false,
        }
    }
}

static LAST: Mutex<Option<Picked>> = Mutex::new(None);

/// What `Number(text) || 1` makes of the megabytes box.
fn parse_mb(text: &str) -> f64 {
    js_number(text).filter(|n| *n != 0.).unwrap_or(1.)
}

/// The megabytes as the box shows them: whole numbers without a point.
fn mb_text(mb: f64) -> String {
    if mb.fract() == 0. {
        format!("{}", mb as i64)
    } else {
        mb.to_string()
    }
}

// ---- What the dialog asks of the PC ------------------------------------------------------------------------

/// The calls into glint-core's media studio and the speech model. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// What the first file holds, and its size (`mediaProbe`; the size was the entry's).
    fn probe(&self, path: String) -> Fut<(Probe, u64)>;
    /// The encoders this FFmpeg has (`mediaEncoders`).
    fn encoders(&self) -> Fut<Encoders>;
    /// Runs `task` on each of `files` as `output` says, reporting progress on `progress` (`mediaRun`).
    fn run(
        &self,
        files: Vec<String>,
        task: Task,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report>;
    /// Stops the run under way (`mediaCancel`).
    fn cancel(&self);
    /// Whether the speech model (and whisper.cpp) are there (`mediaStatus().speechInstalled`).
    fn speech_installed(&self) -> Fut<bool>;
    /// Downloads them, telling `progress` how far it is ("Downloading 40%"); whether they are there now
    /// (`media_ai_install`).
    fn install_speech(&self, progress: async_channel::Sender<String>) -> Fut<bool>;
    /// What the install cards ask about tools (none: every tool counts as there).
    fn tools(&self) -> Option<Arc<dyn SettingsBackend>>;
}

/// glint-core's media studio, owned by the main window as the shell dialogs it opens are.
pub struct CoreBackend {
    app: glint_core::host::AppHandle,
}

impl CoreBackend {
    /// The backend over core's handle (from `core_bridge::Core`).
    pub fn new(app: glint_core::host::AppHandle) -> Self {
        Self { app }
    }
}

impl Backend for CoreBackend {
    fn probe(&self, path: String) -> Fut<(Probe, u64)> {
        parts::on_core(async move {
            let probe = glint_core::media_tools::media_probe(path.clone()).await?;
            let size = glint_core::fsx::blocking(move || Ok(std::fs::metadata(&path).map_or(0, |m| m.len()))).await?;
            Ok((probe, size))
        })
    }

    fn encoders(&self) -> Fut<Encoders> {
        parts::on_core(glint_core::media_tools::media_encoders())
    }

    fn run(
        &self,
        files: Vec<String>,
        task: Task,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report> {
        let app = self.app.clone();
        parts::on_core(async move {
            let window = parts::main_window(&app)?;
            glint_core::media_tools::media_run(window, files, task, output, progress).await
        })
    }

    fn cancel(&self) {
        glint_core::media_tools::media_cancel();
    }

    fn speech_installed(&self) -> Fut<bool> {
        let status = SettingsCoreBackend::new(self.app.clone()).media_status();
        Box::pin(async move { status.await.map(|status| status.speech_installed) })
    }

    fn install_speech(&self, progress: async_channel::Sender<String>) -> Fut<bool> {
        let (fetch, heard) = async_channel::unbounded();
        let install = SettingsCoreBackend::new(self.app.clone()).media_install("speech", fetch);
        // "Downloading 40%": the stage and how far, as the TypeScript's `setFetching` worded it.
        glint_core::host::async_runtime::spawn(async move {
            while let Ok(p) = heard.recv().await {
                let text = if p.total > 0 {
                    format!("{} {}%", p.stage, (100. * p.done as f64 / p.total as f64).round())
                } else {
                    p.stage
                };
                if progress.send(text).await.is_err() {
                    break;
                }
            }
        });
        Box::pin(async move { install.await.map(|status| status.speech_installed) })
    }

    fn tools(&self) -> Option<Arc<dyn SettingsBackend>> {
        Some(Arc::new(SettingsCoreBackend::new(self.app.clone())))
    }
}

/// Stands in when core isn't started (never in the app).
struct Unavailable;

impl Backend for Unavailable {
    fn probe(&self, _: String) -> Fut<(Probe, u64)> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn encoders(&self) -> Fut<Encoders> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn run(&self, _: Vec<String>, _: Task, _: Output, _: glint_core::host::ipc::Channel<Progress>) -> Fut<Report> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn cancel(&self) {}
    fn speech_installed(&self) -> Fut<bool> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn install_speech(&self, _: async_channel::Sender<String>) -> Fut<bool> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn tools(&self) -> Option<Arc<dyn SettingsBackend>> {
        None
    }
}

// ---- The dialog -------------------------------------------------------------------------------------------

/// Media studio on some videos or recordings: what to make, its choices, where it goes, the run and its result.
pub struct MediaDialog {
    files: Vec<String>,
    mode: MediaMode,
    picked: Picked,
    encoders: Option<Encoders>,
    probe: Option<(Probe, u64)>,
    /// FFmpeg is there and the encoders and the first file have been asked for.
    asked: bool,
    /// Whether the speech model is there, once known.
    speech: Option<bool>,
    speech_asked: bool,
    /// How far the speech download is, while it runs.
    fetching: Option<SharedString>,
    from: Entity<InputState>,
    to: Entity<InputState>,
    mb: Entity<InputState>,
    place: Entity<WhereChooser>,
    run: Run,
    backend: Arc<dyn Backend>,
    gate: ToolGate,
    _subscriptions: Vec<Subscription>,
}

impl MediaDialog {
    /// A dialog for `files` (all usable for `mode`), starting at `mode`, calling `backend`.
    pub fn new(
        files: Vec<String>,
        mode: MediaMode,
        backend: Arc<dyn Backend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picked = LAST.lock().ok().and_then(|last| last.clone()).unwrap_or_default();
        let from = cx.new(|cx| InputState::new(window, cx).placeholder("0:00"));
        let to = cx.new(|cx| InputState::new(window, cx).placeholder(t!("the end")));
        let mb = cx.new(|cx| InputState::new(window, cx).default_value(mb_text(picked.mb)));
        let start = Where::Beside {
            suffix: mode.suffix().to_string(),
        };
        let place = cx.new(|cx| WhereChooser::new("media-where", &start, window, cx));
        let gate = ToolGate::new(backend.tools(), window, cx);
        let mut subscriptions = vec![cx.observe(&place, |_, _, cx| cx.notify())];
        for input in [&from, &to] {
            subscriptions.push(cx.subscribe_in(input, window, |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }
        subscriptions.push(cx.subscribe_in(&mb, window, |this, input, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                let mb = parse_mb(&input.read(cx).value());
                this.choose(|p| p.mb = mb, cx);
            }
        }));
        subscriptions.extend(gate.observe(cx, |this, cx| {
            this.look(cx);
            cx.notify();
        }));
        let mut this = Self {
            files,
            mode,
            picked,
            encoders: None,
            probe: None,
            asked: false,
            speech: None,
            speech_asked: false,
            fetching: None,
            from,
            to,
            mb,
            place,
            run: Run::Setup,
            backend,
            gate,
            _subscriptions: subscriptions,
        };
        this.sync_place(false, window, cx);
        this.look(cx);
        this.look_for_speech(cx);
        this
    }

    /// The files being worked on.
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// What the dialog makes now.
    pub fn mode(&self) -> MediaMode {
        self.mode
    }

    /// Where the run stands.
    pub fn run_state(&self) -> &Run {
        &self.run
    }

    /// The "Part" boxes of a GIF: from and to.
    pub fn range_inputs(&self) -> (&Entity<InputState>, &Entity<InputState>) {
        (&self.from, &self.to)
    }

    /// The megabytes box of "Fit under a size".
    pub fn megabytes_input(&self) -> &Entity<InputState> {
        &self.mb
    }

    /// The chooser of where results go.
    pub fn place(&self) -> &Entity<WhereChooser> {
        &self.place
    }

    fn run_mut(&mut self) -> &mut Run {
        &mut self.run
    }

    /// Changes a choice and keeps it for the next dialog (`choose`).
    fn choose(&mut self, change: impl FnOnce(&mut Picked), cx: &mut Context<Self>) {
        change(&mut self.picked);
        if let Ok(mut last) = LAST.lock() {
            *last = Some(self.picked.clone());
        }
        cx.notify();
    }

    /// Asks for the encoders and what the first file holds, once FFmpeg is there (the TypeScript's effect on
    /// `ready` and `files`).
    fn look(&mut self, cx: &mut Context<Self>) {
        if self.asked || !self.gate.ready("ffmpeg", cx) {
            return;
        }
        self.asked = true;
        let encoders = self.backend.encoders();
        let first = self.files.first().cloned().unwrap_or_default();
        let probe = self.backend.probe(first);
        cx.spawn(async move |this, cx| {
            let encoders = encoders.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(encoders) = encoders {
                    this.encoders = Some(encoders);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let probe = probe.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(probe) = probe {
                    this.probe = Some(probe);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Asks whether the speech model is there when Subtitles is the mode.
    fn look_for_speech(&mut self, cx: &mut Context<Self>) {
        if self.mode != MediaMode::Subtitles || self.speech_asked {
            return;
        }
        self.speech_asked = true;
        let installed = self.backend.speech_installed();
        cx.spawn(async move |this, cx| {
            let installed = installed.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(installed) = installed {
                    this.speech = Some(installed);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Tells the chooser of where results go what this mode needs: its suffix when it is "Next to", the word for
    /// what is made, and whether "In their place" is offered.
    fn sync_place(&self, suffix: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.mode;
        self.place.update(cx, |place, cx| {
            place.set_noun(
                if mode == MediaMode::Convert {
                    t!("video")
                } else {
                    t!("file")
                },
                cx,
            );
            place.set_replace_allowed(matches!(mode, MediaMode::Convert | MediaMode::Volume), cx);
            if suffix {
                place.set_suffix(mode.suffix(), window, cx);
            }
        });
    }

    fn set_mode(&mut self, mode: MediaMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == mode || !matches!(self.run, Run::Setup) {
            return;
        }
        self.mode = mode;
        self.sync_place(true, window, cx);
        self.look_for_speech(cx);
        cx.notify();
    }

    fn videos(&self) -> Vec<String> {
        self.files.iter().filter(|f| is_video_file(f)).cloned().collect()
    }

    /// The files the mode can work on: videos for Convert and GIF, videos and recordings for the rest.
    pub fn usable(&self) -> Vec<String> {
        let wants_video = matches!(self.mode, MediaMode::Convert | MediaMode::Gif);
        self.files
            .iter()
            .filter(|f| {
                if wants_video {
                    is_video_file(f)
                } else {
                    is_media_file(f)
                }
            })
            .cloned()
            .collect()
    }

    /// The part of a GIF: its start and end in seconds, where given.
    fn range(&self, cx: &App) -> (Option<f64>, Option<f64>) {
        (seconds(&self.from.read(cx).value()), seconds(&self.to.read(cx).value()))
    }

    /// Whether the part typed isn't one (`rangeBad`).
    pub fn range_bad(&self, cx: &App) -> bool {
        let (from, to) = (self.from.read(cx).value(), self.to.read(cx).value());
        let (start, end) = self.range(cx);
        (!from.trim().is_empty() && start.is_none())
            || (!to.trim().is_empty() && end.is_none())
            || matches!((start, end), (Some(start), Some(end)) if end <= start)
    }

    fn speech_missing(&self) -> bool {
        self.mode == MediaMode::Subtitles && self.speech == Some(false)
    }

    /// What the mode asks core to do with the choices made.
    pub fn task(&self, cx: &App) -> Task {
        let p = &self.picked;
        match self.mode {
            MediaMode::Convert => Task::Convert {
                format: p.format,
                quality: match p.quality {
                    QualityKind::Good => Quality::Good,
                    QualityKind::High => Quality::High,
                    QualityKind::Size => Quality::Size { mb: p.mb.max(1.) },
                },
                height: (p.height != 0).then_some(p.height),
            },
            MediaMode::Audio => Task::Audio { format: p.audio },
            MediaMode::Gif => {
                let (start, end) = self.range(cx);
                Task::Gif {
                    width: p.gif_width,
                    fps: p.fps,
                    start,
                    end,
                    webp: p.webp,
                }
            }
            MediaMode::Volume => Task::Loudness { music: p.music },
            MediaMode::Subtitles => Task::Subtitles { translate: p.translate },
        }
    }

    /// Whether the action can be pressed: FFmpeg is there, there is something to work on, the output and the
    /// part are usable, and Subtitles has its model.
    pub fn can_start(&self, cx: &App) -> bool {
        matches!(self.run, Run::Setup)
            && self.gate.ready("ffmpeg", cx)
            && !self.usable().is_empty()
            && !self.place.read(cx).bad(cx)
            && !(self.mode == MediaMode::Gif && self.range_bad(cx))
            && !self.speech_missing()
    }

    /// Starts the run on core's runtime; progress and the result come back here (`start`).
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_start(cx) {
            return;
        }
        let task = self.task(cx);
        let output = self.place.read(cx).output(cx).output();
        let usable = self.usable();
        let (channel, receiver) = core_bridge::stream::<Progress>();
        let call = self.backend.run(usable.clone(), task, output, channel);
        let verb = self.mode.undo_verb();
        parts::drive(
            self,
            window,
            cx,
            parts::Launch {
                total: usable.len(),
                call,
                progress: receiver,
            },
            Self::run_mut,
            move |report| {
                t!(
                    "{verb} of {what}",
                    verb = verb,
                    what = plural(report.made.len(), "file")
                )
                .to_string()
            },
        );
    }

    fn stop(&mut self) {
        self.backend.cancel();
    }

    /// Downloads the speech model and whisper.cpp (`fetchSpeech`).
    fn fetch_speech(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fetching.is_some() {
            return;
        }
        self.fetching = Some(t!("Starting"));
        cx.notify();
        let (progress, heard) = async_channel::unbounded();
        let call = self.backend.install_speech(progress);
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(text) = heard.recv().await {
                let alive = this.update(cx, |this, cx| {
                    this.fetching = Some(text.into());
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let result = call.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.fetching = None;
                match result {
                    Ok(installed) => this.speech = Some(installed),
                    Err(reason) if reason != "cancelled" => window.push_notification(
                        Notification::error(SharedString::from(reason)).title(t!("Couldn't download the speech model")),
                        cx,
                    ),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }
}

// ---- Drawing ----------------------------------------------------------------------------------------------

impl MediaDialog {
    /// The row of tabs that says what to make.
    fn tabs(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let no_videos = self.videos().is_empty();
        row_start()
            .id("media-modes")
            .role(Role::TabList)
            .aria_label(t!("What to make"))
            .test_support()
            .gap(space(1.))
            .p(space(1.))
            .rounded(theme::size::RADIUS_LG)
            .bg(t.muted)
            .children(MediaMode::ALL.into_iter().enumerate().map(|(ix, mode)| {
                let chosen = self.mode == mode;
                Button::new(("media-mode", ix))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(if chosen { t.background } else { t.background.opacity(0.) })
                            .foreground(if chosen { t.foreground } else { t.muted_foreground })
                            .hover(if chosen { t.background } else { t.accent })
                            .active(t.accent),
                    )
                    .flex_1()
                    .min_w_0()
                    .compact()
                    .text_size(theme::size::XS)
                    .role(Role::Tab)
                    .toggled(chosen)
                    .disabled(matches!(mode, MediaMode::Convert | MediaMode::Gif) && no_videos)
                    .icon(mode.icon())
                    .label(mode.label())
                    .on_click(cx.listener(move |this, _, window, cx| this.set_mode(mode, window, cx)))
            }))
    }

    /// "H264 1920×1080 at 30 fps · AAC sound · 1:05 · 12 MB" for one file.
    pub fn probe_line(&self) -> Option<SharedString> {
        let (probe, size) = self.probe.as_ref().filter(|_| self.files.len() == 1)?;
        let mut line = Vec::new();
        if !probe.video.is_empty() {
            let fps = if probe.fps > 0. {
                t!(" at {fps} fps", fps = (probe.fps + 0.5).floor() as u64).to_string()
            } else {
                String::new()
            };
            line.push(format!(
                "{} {}×{}{fps}",
                probe.video.to_uppercase(),
                probe.width,
                probe.height
            ));
        }
        if !probe.audio.is_empty() {
            line.push(t!("{codec} sound", codec = probe.audio.to_uppercase()).to_string());
        }
        if probe.duration > 0. {
            line.push(clock(probe.duration));
        }
        line.push(format_size(*size));
        Some(line.join(" · ").into())
    }

    fn convert_options(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let encoders = self.encoders.as_ref();
        let chosen_encoder = encoders.and_then(|e| match self.picked.format {
            VideoFormat::H264 => e.h264.as_deref(),
            VideoFormat::Hevc => e.hevc.as_deref(),
            VideoFormat::Av1 => e.av1.as_deref(),
        });
        let no = |has: fn(&Encoders) -> bool| encoders.is_some_and(|e| !has(e));
        let sizes = [8., 10., 25., 50., 100.];
        col()
            .gap(space(4.))
            .child(
                col()
                    .gap(space(1.5))
                    .child(parts::heading(t!("Into"), t))
                    .child(
                        Choices::new("media-format", t!("Format"), self.picked.format)
                            .columns(3)
                            .options(vec![
                                Choice::new(VideoFormat::H264, t!("MP4 (H.264)"))
                                    .hint(t!("Plays everywhere"))
                                    .disabled(no(|e| e.h264.is_some())),
                                Choice::new(VideoFormat::Hevc, t!("MP4 (HEVC)"))
                                    .hint(t!("About half the size"))
                                    .disabled(no(|e| e.hevc.is_some())),
                                Choice::new(VideoFormat::Av1, t!("MP4 (AV1)"))
                                    .hint(if no(|e| e.av1.is_some()) {
                                        t!("No AV1 encoder here")
                                    } else {
                                        t!("Smallest, newer devices")
                                    })
                                    .disabled(no(|e| e.av1.is_some())),
                            ])
                            .on_pick(
                                cx.listener(|this, format: &VideoFormat, _, cx| {
                                    this.choose(|p| p.format = *format, cx)
                                }),
                            ),
                    )
                    .when_some(chosen_encoder, |this, encoder| {
                        this.child(parts::note(
                            t!("Encoded with {name}.", name = encoder_name(Some(encoder))),
                            t,
                        ))
                    }),
            )
            .child(
                col()
                    .gap(space(1.5))
                    .child(parts::heading(t!("Quality"), t))
                    .child(
                        Choices::new("media-quality", t!("Quality"), self.picked.quality)
                            .columns(3)
                            .options(vec![
                                Choice::new(QualityKind::Good, t!("Good")).hint(t!("Small, fine to share")),
                                Choice::new(QualityKind::High, t!("High")).hint(t!("Close to the original")),
                                Choice::new(QualityKind::Size, t!("Fit under a size")).hint(t!("For mail and chats")),
                            ])
                            .on_pick(cx.listener(|this, quality: &QualityKind, _, cx| {
                                this.choose(|p| p.quality = *quality, cx)
                            })),
                    )
                    .when(self.picked.quality == QualityKind::Size, |this| {
                        this.child(
                            align_row(1.5)
                                .flex_wrap()
                                .pt(space(1.))
                                .children(sizes.into_iter().enumerate().map(|(ix, mb)| {
                                    let on = self.picked.mb == mb;
                                    let button = Button::new(("media-size", ix))
                                        .compact()
                                        .label(t!("{mb} MB", mb = mb as u32))
                                        .toggled(on)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            // Setting the box's text says nothing to the dialog, so it is told.
                                            this.mb.update(cx, |input, cx| input.set_value(mb_text(mb), window, cx));
                                            this.choose(|p| p.mb = mb, cx);
                                        }));
                                    if on {
                                        button.primary()
                                    } else {
                                        button.outline()
                                    }
                                }))
                                .child(
                                    div()
                                        .w(space(20.))
                                        .child(Input::new(&self.mb).small().aria_label(t!("Megabytes"))),
                                )
                                .child(parts::note(t!("MB each"), t)),
                        )
                    }),
            )
            .child(parts::field(
                t!("Height"),
                Choices::new("media-height", t!("Height"), self.picked.height)
                    .columns(3)
                    .options(vec![
                        Choice::new(0, t!("Keep it")),
                        Choice::new(2160, t!("2160p (4K) at most")),
                        Choice::new(1080, t!("1080p at most")),
                        Choice::new(720, t!("720p at most")),
                        Choice::new(480, t!("480p at most")),
                    ])
                    .on_pick(cx.listener(|this, height: &u32, _, cx| this.choose(|p| p.height = *height, cx))),
                t,
            ))
    }

    fn audio_options(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        col()
            .gap(space(1.5))
            .child(parts::heading(t!("The sound, as"), t))
            .child(
                Choices::new("media-audio", t!("Format"), self.picked.audio)
                    .columns(3)
                    .options(vec![
                        Choice::new(AudioFormat::Same, t!("As it is")).hint(t!("No quality lost, at once")),
                        Choice::new(AudioFormat::Mp3, t!("MP3")).hint(t!("Plays everywhere")),
                        Choice::new(AudioFormat::M4a, t!("M4A (AAC)")).hint(t!("Phones and iTunes")),
                        Choice::new(AudioFormat::Opus, t!("Opus")).hint(t!("Smallest for speech")),
                        Choice::new(AudioFormat::Flac, t!("FLAC")).hint(t!("Lossless")),
                        Choice::new(AudioFormat::Wav, t!("WAV")).hint(t!("Uncompressed, for editing")),
                    ])
                    .on_pick(cx.listener(|this, audio: &AudioFormat, _, cx| this.choose(|p| p.audio = *audio, cx))),
            )
    }

    fn gif_options(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let no_webp = self.encoders.as_ref().is_some_and(|e| !e.webp);
        let bad = self.range_bad(cx);
        col()
            .gap(space(3.))
            .child(
                Choices::new("media-kind", t!("Kind"), self.picked.webp)
                    .options(vec![
                        Choice::new(false, t!("GIF")).hint(t!("Works in every chat and page")),
                        Choice::new(true, t!("Animated WebP"))
                            .hint(if no_webp {
                                t!("This FFmpeg can't make WebP")
                            } else {
                                t!("Far smaller, modern browsers")
                            })
                            .disabled(no_webp),
                    ])
                    .on_pick(cx.listener(|this, webp: &bool, _, cx| this.choose(|p| p.webp = *webp, cx))),
            )
            .child(parts::field(
                t!("Width"),
                Choices::new("media-gif-width", t!("Width"), self.picked.gif_width)
                    .columns(3)
                    .options(
                        [320, 480, 640, 800, 1080]
                            .into_iter()
                            .map(|w| Choice::new(w, t!("{w} pixels", w = w)))
                            .collect(),
                    )
                    .on_pick(cx.listener(|this, w: &u32, _, cx| this.choose(|p| p.gif_width = *w, cx))),
                t,
            ))
            .child(parts::field(
                t!("Frames"),
                Choices::new("media-gif-fps", t!("Frames a second"), self.picked.fps)
                    .columns(3)
                    .options(
                        [8, 10, 12, 15, 20, 24]
                            .into_iter()
                            .map(|f| Choice::new(f, t!("{f} a second", f = f)))
                            .collect(),
                    )
                    .on_pick(cx.listener(|this, f: &u32, _, cx| this.choose(|p| p.fps = *f, cx))),
                t,
            ))
            .child(
                align_row(2.)
                    .child(
                        div()
                            .w(space(20.))
                            .flex_none()
                            .text_size(theme::size::XS)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.muted_foreground)
                            .child(t!("Part")),
                    )
                    .child(
                        div()
                            .w(space(20.))
                            .child(Input::new(&self.from).small().aria_label(t!("From"))),
                    )
                    .child(div().text_color(t.muted_foreground).child(t!("to")))
                    .child(
                        div()
                            .w(space(20.))
                            .child(Input::new(&self.to).small().aria_label(t!("To"))),
                    )
                    .when(bad, |this| {
                        this.child(
                            div()
                                .id("media-range-bad")
                                .role(Role::Alert)
                                .aria_label(t!("Not a part of it"))
                                .test_support()
                                .text_size(theme::size::XS)
                                .text_color(t.destructive)
                                .child(t!("Not a part of it")),
                        )
                    }),
            )
            .child(parts::note(
                t!("Keep it short: a GIF of a few seconds is already a few megabytes."),
                t,
            ))
    }

    fn volume_options(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        col()
            .gap(space(1.5))
            .child(
                Choices::new("media-loud", t!("Loud as"), self.picked.music)
                    .options(vec![
                        Choice::new(false, t!("Speech and podcasts")).hint(t!("−16 LUFS, as podcast apps play")),
                        Choice::new(true, t!("Music")).hint(t!("−14 LUFS, as streaming services play")),
                    ])
                    .on_pick(cx.listener(|this, music: &bool, _, cx| this.choose(|p| p.music = *music, cx))),
            )
            .child(parts::note(
                t!("Measured first, then turned up or down in one even step (EBU R128), so recordings from different places sound as loud as each other without clipping. A video's picture is kept as it is."),
                t,
            ))
    }

    fn subtitle_options(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let fetching = self.fetching.clone();
        col()
            .gap(space(3.))
            .child(
                Choices::new("media-language", t!("Language"), self.picked.translate)
                    .options(vec![
                        Choice::new(false, t!("As spoken")).hint(t!("In the language heard")),
                        Choice::new(true, t!("In English")).hint(t!("Translated as it's heard")),
                    ])
                    .on_pick(cx.listener(|this, translate: &bool, _, cx| {
                        this.choose(|p| p.translate = *translate, cx)
                    })),
            )
            .child(parts::note(
                t!("Heard on this PC by whisper.cpp (speech search's model); nothing is sent anywhere. Players find an .srt named like its video by themselves."),
                t,
            ))
            .when(self.speech_missing(), |this| {
                let why = t!("Subtitles need whisper.cpp and its speech model (60 MB).");
                this.child(
                    row_start()
                        .id("media-speech")
                        .role(Role::Group)
                        .aria_label(why.clone())
                        .test_support()
                        .gap(space(3.))
                        .px(space(4.))
                        .py(space(3.))
                        .rounded(theme::size::RADIUS_LG)
                        .border_1()
                        .border_color(t.border)
                        .bg(t.primary.opacity(0.05))
                        .child(icon(IconName::Captions).size_4().flex_none().text_color(t.primary))
                        .child(div().flex_1().min_w_0().text_start().child(why))
                        .child(
                            Button::new("media-speech-download")
                                .primary()
                                .icon(IconName::Download)
                                .label(fetching.clone().unwrap_or_else(|| t!("Download")))
                                .loading(fetching.is_some())
                                .disabled(fetching.is_some())
                                .on_click(cx.listener(|this, _, window, cx| this.fetch_speech(window, cx))),
                        ),
                )
            })
    }

    /// The choices, before the run starts.
    fn setup(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let usable = self.usable().len();
        let left_out = self.files.len().saturating_sub(usable);
        col()
            .gap(space(4.))
            .child(self.tabs(t, cx))
            .when_some(self.probe_line(), |this, line| {
                this.child(
                    div()
                        .id("media-probe")
                        .role(Role::Status)
                        .aria_label(line.clone())
                        .test_support()
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(line),
                )
            })
            .child(match self.mode {
                MediaMode::Convert => self.convert_options(t, cx).into_any_element(),
                MediaMode::Audio => self.audio_options(t, cx).into_any_element(),
                MediaMode::Gif => self.gif_options(t, cx).into_any_element(),
                MediaMode::Volume => self.volume_options(t, cx).into_any_element(),
                MediaMode::Subtitles => self.subtitle_options(t, cx).into_any_element(),
            })
            .when(left_out > 0, |this| {
                this.child(
                    div()
                        .id("media-left-out")
                        .role(Role::Status)
                        .aria_label(t!(
                            "{what} without a video will be left out.",
                            what = plural(left_out, "file")
                        ))
                        .test_support()
                        .text_size(theme::size::XS)
                        .text_color(t.warning)
                        .child(t!(
                            "{what} without a video will be left out.",
                            what = plural(left_out, "file")
                        )),
                )
            })
            .child(self.place.clone())
            .into_any_element()
    }
}

/// The dialog's title: the file's name, or how many videos or files.
pub fn title(files: &[String]) -> SharedString {
    match files {
        [one] => base_name(one).to_string().into(),
        _ if files.iter().all(|f| is_video_file(f)) => t!("{n} videos", n = format_count(files.len())),
        _ => t!("{n} files", n = format_count(files.len())),
    }
}

impl Render for MediaDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let body = match self.run.clone() {
            Run::Setup => self.setup(&t, cx),
            Run::Running(progress) => {
                let verb = if self.mode == MediaMode::Subtitles {
                    t!("Listening to")
                } else {
                    t!("Working on")
                };
                parts::progress_view("media", &verb, &progress, &t).into_any_element()
            }
            Run::Done(report) => {
                parts::result_view("media", "file", self.mode == MediaMode::Convert, &report, &t).into_any_element()
            }
            Run::Failed(reason) => parts::error_view("media", t!("Couldn't make it"), &reason, &t).into_any_element(),
        };
        let card = self.gate.card(
            "ffmpeg",
            t!("FFmpeg converts, shrinks and cuts videos and sound, with your graphics card when it can."),
            IconName::Film,
            cx,
        );
        let footer = parts::Footer::<Self> {
            slot: "media",
            run: self.run.clone(),
            label: self.mode.start_label(),
            icon: self.mode.icon(),
            can_start: self.can_start(cx),
            start: Self::start,
            stop: Self::stop,
        }
        .render(&t, cx);
        col()
            .id("media")
            .role(Role::Group)
            .aria_label(t!("Media: {title}", title = title(&self.files)))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .child(parts::folder_line("media-folder", &self.files, &t))
            .child(
                div()
                    .id("media-body")
                    .max_h(parts::body_height(window))
                    .overflow_y_scrollbar()
                    .child(col().gap(space(4.)).children(card).child(body)),
            )
            .child(footer)
    }
}

// ---- Opening it -------------------------------------------------------------------------------------------

/// Opens Media studio on the videos and recordings among `paths`, at the mode `tool` names (`openMedia`). When
/// none is left it says so in a notification instead.
pub fn open(window: &mut Window, cx: &mut App, paths: Vec<String>, tool: MediaTool) {
    let mode = MediaMode::from(tool);
    let wants_video = matches!(mode, MediaMode::Convert | MediaMode::Gif);
    let files: Vec<String> = paths
        .into_iter()
        .filter(|p| {
            if wants_video {
                is_video_file(p)
            } else {
                is_media_file(p)
            }
        })
        .collect();
    if files.is_empty() {
        let message = if wants_video {
            t!("No videos chosen")
        } else {
            t!("No videos or recordings chosen")
        };
        window.push_notification(Notification::info(message), cx);
        return;
    }
    let backend: Arc<dyn Backend> = match cx.try_global::<core_bridge::Core>() {
        Some(core) => Arc::new(CoreBackend::new(core.app.clone())),
        None => Arc::new(Unavailable),
    };
    open_with(window, cx, backend, files, mode);
}

/// [`open`] for a given backend, without filtering `files` (tests pass a fake). Returns the dialog's view.
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    files: Vec<String>,
    mode: MediaMode,
) -> Entity<MediaDialog> {
    let shown = isolate(title(&files));
    let view = cx.new(|cx| MediaDialog::new(files, mode, backend, window, cx));
    super::show(window, cx, shown, px(576.), view.clone());
    view
}

// ---- Tests ------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use glint_core::actions::Recycled;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::time::Duration;

    /// What the fake was asked to run.
    type RunCall = (Vec<String>, Task, Output);

    /// A backend with canned answers; a run waits for `release` when it has one.
    struct Fake {
        probe: Probe,
        size: u64,
        encoders: Encoders,
        report: Report,
        error: Option<String>,
        release: Option<async_channel::Receiver<()>>,
        speech: Mutex<bool>,
        downloads: Mutex<usize>,
        calls: Mutex<Vec<RunCall>>,
        cancelled: Mutex<usize>,
    }

    impl Fake {
        fn new(report: Report) -> Self {
            Self {
                probe: Probe {
                    duration: 65.4,
                    width: 1920,
                    height: 1080,
                    fps: 29.97,
                    video: "h264".into(),
                    audio: "aac".into(),
                    container: "mov,mp4".into(),
                    bitrate: 1,
                    playable: true,
                },
                size: 12 * 1024 * 1024,
                encoders: Encoders {
                    h264: Some("h264_nvenc".into()),
                    hevc: None,
                    av1: None,
                    webp: false,
                    hardware: vec!["nvenc".into()],
                },
                report,
                error: None,
                release: None,
                speech: Mutex::new(true),
                downloads: Mutex::new(0),
                calls: Mutex::new(Vec::new()),
                cancelled: Mutex::new(0),
            }
        }

        fn calls(&self) -> Vec<RunCall> {
            self.calls.lock().map(|c| c.clone()).unwrap_or_default()
        }
    }

    impl Backend for Fake {
        fn probe(&self, _: String) -> Fut<(Probe, u64)> {
            let answer = (self.probe.clone(), self.size);
            Box::pin(async move { Ok(answer) })
        }

        fn encoders(&self) -> Fut<Encoders> {
            let encoders = self.encoders.clone();
            Box::pin(async move { Ok(encoders) })
        }

        fn run(
            &self,
            files: Vec<String>,
            task: Task,
            output: Output,
            progress: glint_core::host::ipc::Channel<Progress>,
        ) -> Fut<Report> {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push((files.clone(), task, output));
            }
            let _ = progress.send(Progress {
                done: 0,
                total: files.len(),
                current: "clip.mp4".into(),
                part: 0.5,
            });
            let (report, error, release) = (self.report.clone(), self.error.clone(), self.release.clone());
            Box::pin(async move {
                if let Some(release) = release {
                    let _ = release.recv().await;
                }
                match error {
                    Some(error) => Err(error),
                    None => Ok(report),
                }
            })
        }

        fn cancel(&self) {
            if let Ok(mut n) = self.cancelled.lock() {
                *n += 1;
            }
        }

        fn speech_installed(&self) -> Fut<bool> {
            let there = self.speech.lock().map(|s| *s).unwrap_or(true);
            Box::pin(async move { Ok(there) })
        }

        fn install_speech(&self, progress: async_channel::Sender<String>) -> Fut<bool> {
            if let Ok(mut n) = self.downloads.lock() {
                *n += 1;
            }
            if let Ok(mut there) = self.speech.lock() {
                *there = true;
            }
            Box::pin(async move {
                let _ = progress.send("Downloading 50%".into()).await;
                Ok(true)
            })
        }

        fn tools(&self) -> Option<Arc<dyn SettingsBackend>> {
            None
        }
    }

    fn report(made: &[&str]) -> Report {
        Report {
            made: made.iter().map(|s| (*s).to_string()).collect(),
            before: 12 * 1024 * 1024,
            after: 4 * 1024 * 1024,
            elapsed_ms: 1234.,
            ..Default::default()
        }
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the media dialog never reached the state the test waits for");
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    /// Whether the button is disabled (an enabled one says nothing at all).
    fn disabled(cx: &mut VisualTestContext, id: impl Into<ElementId>) -> bool {
        let id = id.into();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.find(id).disabled() == Some(true)
        })
    }

    fn click(cx: &mut VisualTestContext, id: impl Into<ElementId>) {
        let id = id.into();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        cx.run_until_parked();
    }

    fn type_into(cx: &mut VisualTestContext, input: &Entity<InputState>, text: &str) {
        let input = input.clone();
        let text = text.to_string();
        cx.update(|window, cx| {
            input.update(cx, |input, cx| input.focus(window, cx));
            window.press("ctrl-a", cx);
            window.input(&text, cx);
        });
        cx.run_until_parked();
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn window(cx: &mut TestAppContext) -> VisualTestContext {
        cx.update(testing::init);
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
        testing::open_window(cx, |_, cx| cx.new(|_| Host)).0
    }

    fn open_view(cx: &mut VisualTestContext, files: &[&str], mode: MediaMode, fake: &Arc<Fake>) -> Entity<MediaDialog> {
        let backend: Arc<dyn Backend> = fake.clone();
        let files = strings(files);
        let view = cx.update(|window, cx| open_with(window, cx, backend, files, mode));
        cx.run_until_parked();
        view
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::Film,
            IconName::AudioLines,
            IconName::Clapperboard,
            IconName::Captions,
            IconName::Volume2,
            IconName::Download,
            IconName::Square,
            IconName::FolderInput,
            IconName::CircleCheck,
            IconName::CircleDot,
            IconName::Circle,
            IconName::TriangleAlert,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    #[test]
    fn times_clocks_encoders_and_kinds_read_as_the_typescript_did() {
        // seconds(): "1:05" and "12.5", and what isn't a time.
        assert_eq!(seconds("1:05"), Some(65.));
        assert_eq!(seconds("  12.5 "), Some(12.5));
        assert_eq!(seconds("1:02:03"), Some(3723.));
        assert_eq!(seconds(""), None);
        assert_eq!(seconds("abc"), None);
        assert_eq!(seconds("-5"), None);
        assert_eq!(seconds("1:-5"), None);
        assert_eq!(seconds("inf"), None);
        // Number("") is 0, so a piece left empty counts as 0, as it did.
        assert_eq!(seconds("1:"), Some(60.));
        assert_eq!(seconds(":30"), Some(30.));
        assert_eq!(clock(65.4), "1:05");
        assert_eq!(clock(0.), "0:00");
        assert_eq!(clock(3599.9), "59:59");
        assert_eq!(clock(-4.), "0:00");
        assert_eq!(encoder_name(Some("h264_nvenc")), "NVIDIA GPU");
        assert_eq!(encoder_name(Some("hevc_qsv")), "Intel Quick Sync");
        assert_eq!(encoder_name(Some("av1_amf")), "AMD GPU");
        assert_eq!(encoder_name(Some("libx264")), "x264");
        assert_eq!(encoder_name(Some("libx265")), "x265");
        assert_eq!(encoder_name(Some("libsvtav1")), "SVT-AV1");
        assert_eq!(encoder_name(Some("libaom-av1")), "libaom-av1");
        assert_eq!(encoder_name(None), "");
        assert!(is_video_file("C:\\a\\Clip.MP4") && !is_video_file("C:\\a\\song.mp3"));
        assert!(is_audio_file("C:\\a\\song.FLAC") && is_media_file("C:\\a\\song.flac"));
        assert!(!is_media_file("C:\\a\\notes.txt") && !is_media_file("C:\\a\\mp4"));
        // Number(text) || 1 for the megabytes, and how they are shown.
        assert_eq!(
            (parse_mb("25"), parse_mb("0"), parse_mb(""), parse_mb("x")),
            (25., 1., 1., 1.)
        );
        assert_eq!((mb_text(25.), mb_text(1.5)), ("25".to_string(), "1.5".to_string()));
    }

    #[test]
    fn progress_lines_sizes_and_shares_read_as_the_typescript_did() {
        let _guard = testing::lock();
        let at = |done, total, current: &str, part| Progress {
            done,
            total,
            current: current.into(),
            part,
        };
        assert_eq!(parts::progress_text("Working on", &at(0, 1, "", 0.)), "Working on");
        assert_eq!(
            parts::progress_text("Working on", &at(1, 3, "a.mp4", 0.5)),
            "Working on 2 of 3 · a.mp4 · 50%"
        );
        // The count never runs past the total.
        assert_eq!(parts::progress_text("Saving", &at(3, 3, "", 0.)), "Saving 3 of 3");
        assert!((parts::whole(&at(1, 4, "", 0.5)) - 0.375).abs() < 1e-6);
        assert_eq!(parts::whole(&at(0, 0, "", 0.5)), 0.);
        assert_eq!(
            parts::sizes_line(12 * 1024 * 1024, 4 * 1024 * 1024).as_deref(),
            Some("12 MB → 4.0 MB (−67%)")
        );
        assert_eq!(parts::sizes_line(1000, 1500).as_deref(), Some("1000 B → 1.5 KB (+50%)"));
        assert_eq!(parts::sizes_line(0, 5), None);
    }

    #[test]
    fn the_title_names_the_file_or_counts_videos_and_files() {
        let _guard = testing::lock();
        assert_eq!(title(&strings(&["C:\\a\\clip.mp4"])), "clip.mp4");
        assert_eq!(title(&strings(&["C:\\a\\a.mp4", "C:\\a\\b.mkv"])), "2 videos");
        assert_eq!(title(&strings(&["C:\\a\\a.mp4", "C:\\a\\b.mp3"])), "2 files");
        assert_eq!(MediaMode::from(MediaTool::Gif), MediaMode::Gif);
        assert_eq!(MediaMode::from(MediaTool::Volume), MediaMode::Volume);
    }

    #[gpui_kit::test]
    fn opening_with_nothing_it_takes_says_so_instead_of_opening(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        // Convert and GIF want videos; a recording and a text file give none.
        visual.update(|window, cx| {
            open(
                window,
                cx,
                strings(&["C:\\a\\song.mp3", "C:\\a\\x.txt"]),
                MediaTool::Convert,
            )
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        visual.update(|window, cx| open(window, cx, strings(&["C:\\a\\x.txt"]), MediaTool::Subtitles));
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn converting_a_video_shows_what_it_has_runs_with_the_choices_and_reports(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(report(&["C:\\a\\clip (converted).mp4"])));
        let view = open_view(&mut visual, &["C:\\a\\clip.mp4"], MediaMode::Convert, &fake);
        visual.update(|window, cx| assert!(window.has_active_dialog(cx)));
        assert_eq!(role_of(&mut visual, "Media: clip.mp4"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "What to make"), Some(Role::TabList));
        assert_eq!(role_of(&mut visual, "Format"), Some(Role::RadioGroup));
        assert_eq!(role_of(&mut visual, "Where"), Some(Role::RadioGroup));
        // What the file holds, once the probe answers.
        settle(&mut visual, |cx| {
            has(cx, "H264 1920×1080 at 30 fps · AAC sound · 1:05 · 12 MB")
        });
        // Only the encoder this FFmpeg has can be picked.
        assert!(!disabled(&mut visual, ("media-format", 0_usize)));
        assert!(disabled(&mut visual, ("media-format", 1_usize)));
        assert!(disabled(&mut visual, ("media-format", 2_usize)));
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find(("media-format", 0_usize)).checked(), Some(true));
            assert_eq!(window.find(("media-quality", 0_usize)).checked(), Some(true));
        });
        // Fit under a size shows the megabytes; 10 MB is picked from the row.
        click(&mut visual, ("media-quality", 2_usize));
        click(&mut visual, ("media-size", 1_usize));
        click(&mut visual, ("media-height", 2_usize));
        assert!(!disabled(&mut visual, "media-start"));
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        let calls = fake.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, ["C:\\a\\clip.mp4"]);
        assert_eq!(
            calls[0].1,
            Task::Convert {
                format: VideoFormat::H264,
                quality: Quality::Size { mb: 10. },
                height: Some(1080),
            }
        );
        assert_eq!(
            calls[0].2,
            Output::Beside {
                suffix: " (converted)".into()
            }
        );
        assert_eq!(role_of(&mut visual, "Made 1 file"), Some(Role::Status));
        // After a run: Show and Close, no Cancel or Convert.
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("media-show").is_some());
            assert!(window.try_find("media-start").is_none() && window.try_find("media-cancel").is_none());
        });
        assert!(matches!(
            view.read_with(&visual, |v, _| v.run_state().clone()),
            Run::Done(_)
        ));
        // The choices are kept for the next dialog while Glint runs.
        let last = LAST.lock().ok().and_then(|l| l.clone()).unwrap_or_default();
        assert_eq!((last.quality, last.mb, last.height), (QualityKind::Size, 10., 1080));
        click(&mut visual, "media-close");
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
    }

    #[gpui_kit::test]
    fn the_tabs_change_what_is_made_and_a_gif_needs_a_real_part(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(report(&["C:\\a\\clip.gif"])));
        let view = open_view(&mut visual, &["C:\\a\\clip.mp4"], MediaMode::Convert, &fake);
        let suffix = |cx: &VisualTestContext, view: &Entity<MediaDialog>| {
            view.read_with(cx, |v, cx| v.place().read(cx).output(cx))
        };
        assert_eq!(
            suffix(&visual, &view),
            Where::Beside {
                suffix: " (converted)".into()
            }
        );
        // The GIF tab: the suffix goes (only Convert adds one) and "In their place" is no longer offered.
        click(&mut visual, ("media-mode", 2_usize));
        assert_eq!(view.read_with(&visual, |v, _| v.mode()), MediaMode::Gif);
        assert_eq!(suffix(&visual, &view), Where::Beside { suffix: String::new() });
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find(("media-where", 2_usize)).is_none(),
                "no in-place radio for a GIF"
            );
            assert!(window.try_find(("media-where", 1_usize)).is_some());
        });
        let (from, to) = view.read_with(&visual, |v, _| {
            let (from, to) = v.range_inputs();
            (from.clone(), to.clone())
        });
        // An end before the start is not a part.
        type_into(&mut visual, &from, "5");
        type_into(&mut visual, &to, "3");
        assert_eq!(role_of(&mut visual, "Not a part of it"), Some(Role::Alert));
        assert!(disabled(&mut visual, "media-start"));
        // Words are not times either.
        type_into(&mut visual, &to, "soon");
        assert!(has(&mut visual, "Not a part of it") && disabled(&mut visual, "media-start"));
        type_into(&mut visual, &to, "0:09");
        assert!(!has(&mut visual, "Not a part of it"));
        click(&mut visual, ("media-gif-width", 2_usize));
        click(&mut visual, ("media-gif-fps", 4_usize));
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        assert_eq!(
            fake.calls()[0].1,
            Task::Gif {
                width: 640,
                fps: 20,
                start: Some(5.),
                end: Some(9.),
                webp: false
            }
        );
        assert_eq!(fake.calls()[0].2, Output::Beside { suffix: String::new() });
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
    }

    #[gpui_kit::test]
    fn recordings_get_sound_and_volume_and_videos_without_one_are_left_out(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(report(&["C:\\a\\talk.opus"])));
        let view = open_view(&mut visual, &["C:\\a\\talk.wav"], MediaMode::Audio, &fake);
        // A recording has no picture to convert or make a GIF of.
        assert!(disabled(&mut visual, ("media-mode", 0_usize)));
        assert!(disabled(&mut visual, ("media-mode", 2_usize)));
        assert!(!disabled(&mut visual, ("media-mode", 1_usize)));
        click(&mut visual, ("media-audio", 3_usize));
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        assert_eq!(
            fake.calls()[0].1,
            Task::Audio {
                format: AudioFormat::Opus
            }
        );
        drop(view);
        click(&mut visual, "media-close");

        // Two files, one a recording: Convert leaves it out and says so; the run gets the video only.
        let fake = Arc::new(Fake::new(report(&["C:\\a\\clip (converted).mp4"])));
        let _view = open_view(
            &mut visual,
            &["C:\\a\\clip.mp4", "C:\\a\\talk.wav"],
            MediaMode::Convert,
            &fake,
        );
        assert_eq!(
            role_of(&mut visual, "1 file without a video will be left out."),
            Some(Role::Status)
        );
        assert!(has(&mut visual, "Media: 2 files"));
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        assert_eq!(fake.calls()[0].0, ["C:\\a\\clip.mp4"]);
        assert_eq!(parts::folders_of(&strings(&["C:\\a\\x", "C:\\A\\y"])).len(), 1);
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
    }

    #[gpui_kit::test]
    fn subtitles_wait_for_the_speech_model_which_the_card_downloads(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(report(&["C:\\a\\clip.srt"])));
        if let Ok(mut there) = fake.speech.lock() {
            *there = false;
        }
        let _view = open_view(&mut visual, &["C:\\a\\clip.mp4"], MediaMode::Subtitles, &fake);
        settle(&mut visual, |cx| {
            has(cx, "Subtitles need whisper.cpp and its speech model (60 MB).")
        });
        assert_eq!(
            role_of(&mut visual, "Subtitles need whisper.cpp and its speech model (60 MB)."),
            Some(Role::Group)
        );
        assert!(disabled(&mut visual, "media-start"));
        click(&mut visual, "media-speech-download");
        settle(&mut visual, |cx| {
            !has(cx, "Subtitles need whisper.cpp and its speech model (60 MB).")
        });
        assert_eq!(fake.downloads.lock().map(|n| *n).unwrap_or(0), 1);
        assert!(!disabled(&mut visual, "media-start"));
        click(&mut visual, ("media-language", 1_usize));
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        assert_eq!(fake.calls()[0].1, Task::Subtitles { translate: true });
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
    }

    #[gpui_kit::test]
    fn progress_shows_while_it_runs_stop_cancels_and_a_stopped_run_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (release, held) = async_channel::bounded(1);
        let mut fake = Fake::new(Report {
            stopped: true,
            ..Default::default()
        });
        fake.release = Some(held);
        let fake = Arc::new(fake);
        let _view = open_view(&mut visual, &["C:\\a\\clip.mp4"], MediaMode::Volume, &fake);
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Working on · clip.mp4 · 50%"));
        assert_eq!(
            role_of(&mut visual, "Working on · clip.mp4 · 50%"),
            Some(Role::ProgressIndicator)
        );
        assert!(disabled(&mut visual, "media-cancel"));
        click(&mut visual, "media-stop");
        assert_eq!(fake.cancelled.lock().map(|n| *n).unwrap_or(0), 1);
        // Core ends the run and says it was stopped.
        release.try_send(()).ok();
        settle(&mut visual, |cx| has(cx, "Nothing was made before it was stopped"));
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
    }

    #[gpui_kit::test]
    fn start_waits_for_ffmpeg_and_a_failed_run_says_why(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(report(&[])));
        let view = open_view(&mut visual, &["C:\\a\\clip.mp4"], MediaMode::Convert, &fake);
        // Before the tools are known (or with FFmpeg missing) nothing starts.
        view.update(&mut visual, |v, cx| {
            v.gate = ToolGate::never();
            cx.notify();
        });
        assert!(disabled(&mut visual, "media-start"));
        visual.update(|window, cx| view.update(cx, |v, cx| v.start(window, cx)));
        assert!(fake.calls().is_empty());
        visual.update(|window, cx| window.close_dialog(cx));
        visual.run_until_parked();

        let mut broken = Fake::new(report(&[]));
        broken.error = Some("FFmpeg isn't installed".into());
        let broken = Arc::new(broken);
        let _view = open_view(&mut visual, &["C:\\a\\other.mp4"], MediaMode::Convert, &broken);
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Couldn't make it. FFmpeg isn't installed"));
        assert_eq!(
            role_of(&mut visual, "Couldn't make it. FFmpeg isn't installed"),
            Some(Role::Alert)
        );
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_work_through_a_run(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        let mut partial = report(&["C:\\a\\تقرير (converted).mp4"]);
        partial.failed = (0..8)
            .map(|i| (format!("C:\\a\\ملف{i}.mp4"), "Couldn't read it".to_string()))
            .collect();
        let fake = Arc::new(Fake::new(partial));
        let _view = open_view(&mut visual, &["C:\\a\\تقرير.mp4"], MediaMode::Convert, &fake);
        // The accessible names carry the file name as it is, without direction marks.
        assert_eq!(role_of(&mut visual, "Media: تقرير.mp4"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "C:\\a"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "What to make"), Some(Role::TabList));
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        assert_eq!(role_of(&mut visual, "8 files not done:"), Some(Role::List));
        assert_eq!(role_of(&mut visual, "ملف0.mp4: Couldn't read it"), Some(Role::ListItem));
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find_all_by_role(Role::ListItem).len(),
                6,
                "six shown, the rest counted"
            );
        });
        testing::set_rtl(false);
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
    }

    #[gpui_kit::test]
    fn what_a_run_made_can_be_taken_back_with_the_files_it_replaced(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.update(testing::init);
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
        cx.update(|cx| crate::explorer::init_with(cx, crate::explorer::fake::FakeDisk::new().source()));
        let mut visual = testing::open_window(cx, |_, cx| cx.new(|_| Host)).0;
        let mut done = report(&["C:\\a\\clip.mp4"]);
        done.recycled = vec![Recycled {
            path: "C:\\a\\clip.mp4".into(),
            bin: "C:\\$Recycle.Bin\\S-1\\$R1.mp4".into(),
        }];
        let fake = Arc::new(Fake::new(done));
        let _view = open_view(&mut visual, &["C:\\a\\clip.mp4"], MediaMode::Convert, &fake);
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        let explorer = visual.update(|_, cx| crate::explorer::explorer(cx));
        // Two steps: what was made recycled, then the original restored.
        let label = explorer.read_with(&visual, |e, _| e.next_undo().map(|(_, label)| label.to_string()));
        assert_eq!(label.as_deref(), Some("conversion of 1 file"));
        visual.update(|_, cx| explorer.update(cx, |e, cx| e.undo(None, cx).detach()));
        visual.run_until_parked();
        let label = explorer.read_with(&visual, |e, _| e.next_undo().map(|(_, label)| label.to_string()));
        assert_eq!(
            label.as_deref(),
            Some("conversion of 1 file"),
            "the restore of the original is next"
        );
    }
}
