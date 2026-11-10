//! A program's sections, imports and exports (`BinaryPreview` in BinaryPreview.tsx): what a `.exe`, `.dll`,
//! `.sys`… says about itself in its PE header.
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                                   | Here                                                          |
//! |----------------------------------------------|---------------------------------------------------------------|
//! | `BinaryPreview`                              | [`ProgramPreview`]                                            |
//! | `inspectPe`                                  | [`PeSource`] (glint-core's `inspect_pe`), read in the background |
//! | `filteredImports`, `filteredExports`         | `hex::filter_imports`, `hex::filter_exports` (shared)         |
//! | `isPeCandidate`                              | `hex::is_pe_candidate` (shared)                               |
//! | the Sections, Imports and Exports tabs       | [`PeTab`]                                                     |
//! | `SignatureChip` in the header                | [`ProgramPreview::set_signature`]: the chip is another module's (`signature.rs`) |
//!
//! `preview::hex` already holds a port of this previewer (`BinaryPreview`, written with the hex view). This
//! one is the same screen with two differences: the Imports tab is virtualized (a library can import 64 names
//! and a program 128 libraries, and every name is a button, so drawing them all each frame was thousands of
//! buttons), and the header has a place for the signature chip. The pieces that need no change are used from
//! `hex` rather than copied. Once the frame builds this one, `hex::BinaryPreview` is not needed.
//!
//! The read is glint-core's `inspect_pe`, which reads the headers only (the first bytes of the file), so a
//! program of any size is quick. Sections are capped at 96, imports at 128 libraries of 64 names each and
//! exports at 400 by glint-core; the footer of the Exports tab says when there were more.

use super::hex::{core_pe_source, filter_exports, filter_imports, PeSource, PeTab};
use super::text::{big_icon, name_of, page_list, page_scroll, redraw_after_delay, waiting};
use crate::kit::prelude::*;
use crate::{actions, core_bridge, i18n, t, theme};
use glint_core::pe::{PeExport, PeImportModule, PeInfo};
use glint_model::format::{format_count, format_date, format_duration, format_size};
use gpui_kit::{list, ClipboardItem, ListAlignment, ListState, ScrollHandle, Task, UniformListScrollHandle};
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

/// `0x` and the number in upper-case hex, padded to `width` digits (`hex` in BinaryPreview.tsx).
pub fn hex_pad(n: u64, width: usize) -> String {
    format!("0x{n:0width$X}")
}

/// How wide a section's bar is, in percent (`pct`): its bigger size against the biggest section's, at least 2.
/// JavaScript's `Math.round` rounds half up.
pub fn section_percent(raw_size: u32, virtual_size: u32, biggest: u32) -> f32 {
    let widest = f64::from(raw_size.max(virtual_size));
    let share = widest / f64::from(biggest.max(1)) * 100.;
    (share + 0.5).floor().max(2.) as f32
}

/// What the header read says.
enum Load {
    Waiting,
    /// "No binary header preview for this file".
    Failed,
    Ready(Arc<PeInfo>),
}

/// A program's or library's header: its machine, format and subsystem, sections, imports and exports.
pub struct ProgramPreview {
    path: String,
    source: PeSource,
    load: Load,
    tab: PeTab,
    started: Instant,
    generation: u64,
    query: Entity<InputState>,
    /// The imports that match the filter, and the list that draws them (their cards differ in height).
    imports: Arc<Vec<PeImportModule>>,
    imports_list: ListState,
    /// The exports that match the filter.
    exports: Arc<Vec<PeExport>>,
    exports_scroll: UniformListScrollHandle,
    body_scroll: ScrollHandle,
    signature: Option<AnyView>,
    _read: Task<()>,
    _query_events: Subscription,
}

impl ProgramPreview {
    /// The previewer for the program at `path`. Reading happens in the background.
    pub fn new(path: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_source(path, core_pe_source(), window, cx)
    }

    /// [`ProgramPreview::new`] reading through `source` (tests pass fakes).
    pub fn with_source(path: String, source: PeSource, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Filter imported DLLs or functions…")));
        let events = cx.subscribe(&query, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.apply_filter(cx);
            }
        });
        let mut this = Self {
            path,
            source,
            load: Load::Waiting,
            tab: PeTab::Overview,
            started: Instant::now(),
            generation: 0,
            query,
            imports: Arc::new(Vec::new()),
            imports_list: ListState::new(0, ListAlignment::Top, px(400.)),
            exports: Arc::new(Vec::new()),
            exports_scroll: UniformListScrollHandle::new(),
            body_scroll: ScrollHandle::new(),
            signature: None,
            _read: Task::ready(()),
            _query_events: events,
        };
        this.read(cx);
        this
    }

    /// The file shown.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Shows another file.
    pub fn set_path(&mut self, path: String, cx: &mut Context<Self>) {
        if self.path != path {
            self.path = path;
            self.read(cx);
        }
    }

    /// Reads the header again (the file changed on disk).
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.read(cx);
    }

    /// The tab showing.
    pub fn tab(&self) -> PeTab {
        self.tab
    }

    /// The header, once read.
    pub fn info(&self) -> Option<&PeInfo> {
        match &self.load {
            Load::Ready(info) => Some(info),
            _ => None,
        }
    }

    /// How many import cards and export rows the filter leaves.
    pub fn shown_counts(&self) -> (usize, usize) {
        (self.imports.len(), self.exports.len())
    }

    /// The signature chip (`SignatureChip`: who signed it, and whether Windows trusts them), a view of its own
    /// that sits among the header's chips. The frame builds it for this path and hands it over.
    pub fn set_signature(&mut self, chip: Option<AnyView>, cx: &mut Context<Self>) {
        self.signature = chip;
        cx.notify();
    }

    /// Switches tab; the filter box says what it filters.
    pub fn set_tab(&mut self, tab: PeTab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        let placeholder = if tab == PeTab::Exports {
            t!("Filter exported symbols…")
        } else {
            t!("Filter imported DLLs or functions…")
        };
        self.query
            .update(cx, |query, cx| query.set_placeholder(placeholder, window, cx));
        cx.notify();
    }

    /// Scrolls the tab a page (Page Down and Page Up in Quick Look). Returns whether it moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let moved = match self.tab {
            PeTab::Exports => page_list(&self.exports_scroll, down),
            PeTab::Imports => {
                let step = self.imports_list.viewport_bounds().size.height * 0.85;
                self.imports_list.scroll_by(if down { step } else { -step });
                true
            }
            PeTab::Overview => page_scroll(&self.body_scroll, down),
        };
        if moved {
            cx.notify();
        }
        moved
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.load = Load::Waiting;
        self.started = Instant::now();
        let read = (self.source)(self.path.clone());
        self._read = cx.spawn(async move |this, cx| {
            let result = core_bridge::run(read).await.and_then(|inner| inner);
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.load = match result {
                        Ok(info) => Load::Ready(Arc::new(info)),
                        Err(_) => Load::Failed,
                    };
                    this.apply_filter(cx);
                }
            })
            .ok();
        });
        redraw_after_delay(cx);
        cx.notify();
    }

    /// The filter text changed, or the header arrived: the imports and exports that match.
    fn apply_filter(&mut self, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value().to_string();
        if let Load::Ready(info) = &self.load {
            self.imports = Arc::new(filter_imports(&info.imports, &query));
            self.exports = Arc::new(filter_exports(&info.exports, &query));
        } else {
            self.imports = Arc::new(Vec::new());
            self.exports = Arc::new(Vec::new());
        }
        self.imports_list.reset(self.imports.len());
        cx.notify();
    }

    /// Copies a symbol's name, with a toast.
    fn copy_symbol(text: &str, window: &mut Window, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        window.push_notification(Notification::success(t!("Copied “{text}”", text = text)), cx);
    }

    /// A small label on a tinted or outlined ground.
    fn chip(
        id: &'static str,
        label: SharedString,
        color: Hsla,
        ground: Hsla,
        outline: Option<Hsla>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .role(Role::Group)
            .aria_label(label.clone())
            .test_support()
            .px(space(2.))
            .py(space(0.5))
            .rounded(theme::size::RADIUS)
            .bg(ground)
            .text_color(color)
            .when_some(outline, |this, border| this.border_1().border_color(border))
            .child(label)
    }

    /// One tab of the strip.
    fn tab_button(&self, tab: PeTab, label: SharedString, cx: &mut Context<Self>) -> impl IntoElement {
        let id = match tab {
            PeTab::Overview => "program-tab-sections",
            PeTab::Imports => "program-tab-imports",
            PeTab::Exports => "program-tab-exports",
        };
        Button::new(id)
            .ghost()
            .small()
            .label(label.clone())
            .selected(self.tab == tab)
            .accessibility_label(label)
            .on_click(cx.listener(move |this, _, window, cx| this.set_tab(tab, window, cx)))
    }

    /// The header: machine, format, subsystem, signature, .NET, and how long the read took.
    fn chips(&self, info: &PeInfo, t: &theme::Tokens) -> impl IntoElement {
        let violet = theme::tailwind("violet-500").unwrap_or(t.primary);
        let kind = if info.is_dll { t!("DLL") } else { t!("Executable") };
        row_start()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(space(1.5))
            .px(space(3.))
            .py(space(2.))
            .border_b_1()
            .border_color(t.border)
            .bg(t.chrome.opacity(0.5))
            .child(
                row_start()
                    .id("program-machine")
                    .role(Role::Group)
                    .aria_label(SharedString::from(info.machine.clone()))
                    .test_support()
                    .gap(space(1.))
                    .px(space(2.))
                    .py(space(0.5))
                    .rounded(theme::size::RADIUS)
                    .bg(t.primary.opacity(0.15))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.primary)
                    .child(icon(IconName::Cpu).size_3())
                    .child(SharedString::from(info.machine.clone())),
            )
            .child(Self::chip(
                "program-format",
                format!("{} · {kind}", info.format).into(),
                t.foreground,
                t.background,
                Some(t.border),
            ))
            .child(Self::chip(
                "program-subsystem",
                info.subsystem.clone().into(),
                t.muted_foreground,
                t.background,
                Some(t.border),
            ))
            .children(self.signature.clone())
            .when(info.is_dotnet, |this| {
                this.child(Self::chip(
                    "program-dotnet",
                    t!(".NET CLR"),
                    violet,
                    violet.opacity(0.15),
                    None,
                ))
            })
            .child(
                row_start()
                    .id("program-elapsed")
                    .role(Role::Group)
                    .aria_label(t!("Parsed directly from the PE header"))
                    .test_support()
                    .ms(0.)
                    .flex_1()
                    .justify_end()
                    .gap(space(1.))
                    .text_size(theme::size::XXS)
                    .text_color(t.muted_foreground)
                    .child(icon(IconName::Zap).size_3().text_color(t.primary))
                    .child(format_duration(info.elapsed_ms)),
            )
    }

    /// The Sections tab: the header's facts, the security features, and a card for each section.
    fn overview(&self, info: &PeInfo, t: &theme::Tokens, mono: &SharedString) -> AnyElement {
        let biggest = info
            .sections
            .iter()
            .map(|s| s.raw_size.max(s.virtual_size))
            .max()
            .unwrap_or(1)
            .max(1);
        let mut facts: Vec<(SharedString, SharedString, bool)> = Vec::new();
        if let Some(ms) = info.timestamp_ms.filter(|&ms| ms != 0) {
            facts.push((t!("Compiled"), format_date(ms, None, i18n::language()).into(), false));
        }
        facts.push((t!("Entry point"), hex_pad(u64::from(info.entry_point), 8).into(), true));
        facts.push((t!("Image base"), hex_pad(info.image_base, 12).into(), true));
        facts.push((
            t!("Target OS / Linker"),
            t!(
                "Windows {os} · Linker {linker}",
                os = info.os_version,
                linker = info.linker_version
            ),
            false,
        ));
        if info.checksum != 0 {
            facts.push((t!("PE checksum"), hex_pad(u64::from(info.checksum), 8).into(), true));
        }
        let features = (!info.features.is_empty()).then(|| {
            row_start()
                .id("program-features")
                .role(Role::List)
                .aria_label(t!("Security features"))
                .test_support()
                .flex_wrap()
                .items_center()
                .gap(space(1.5))
                .child(
                    icon(IconName::ShieldCheck)
                        .size_3p5()
                        .flex_none()
                        .text_color(t.muted_foreground),
                )
                .children(info.features.iter().enumerate().map(|(i, feature)| {
                    div()
                        .id(("program-feature", i))
                        .role(Role::ListItem)
                        .aria_label(SharedString::from(feature.clone()))
                        .test_support()
                        .px(space(1.5))
                        .py(space(0.5))
                        .rounded(theme::size::RADIUS_SM)
                        .border_1()
                        .border_color(t.border)
                        .bg(t.muted.opacity(0.5))
                        .text_size(theme::size::XXS)
                        .text_color(t.muted_foreground)
                        .child(SharedString::from(feature.clone()))
                }))
        });
        let facts_view = col()
            .id("program-facts")
            .role(Role::DescriptionList)
            .aria_label(t!("Header"))
            .test_support()
            .gap(space(1.))
            .p(space(2.5))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.border)
            .bg(t.card.opacity(0.5))
            .children(facts.into_iter().enumerate().map(|(i, (name, value, is_mono))| {
                row_start()
                    .id(("program-fact", i))
                    .role(Role::Group)
                    .aria_label(format!("{name}: {value}"))
                    .test_support()
                    .gap(space(3.))
                    .child(
                        div()
                            .flex_none()
                            .w(space(36.))
                            .text_color(t.muted_foreground)
                            .child(name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when(is_mono, |this| this.font_family(mono.clone()))
                            .child(value),
                    )
            }));
        let amber = theme::tailwind(if t.dark { "amber-400" } else { "amber-700" }).unwrap_or(t.foreground);
        let sections = col()
            .id("program-sections")
            .role(Role::List)
            .aria_label(t!("PE Sections"))
            .test_support()
            .gap(space(1.5))
            .child(
                align_row(1.5)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.foreground)
                    .child(icon(IconName::Layers).size_3p5().text_color(t.primary))
                    .child(t!("PE Sections")),
            )
            .children(info.sections.iter().enumerate().map(|(i, s)| {
                let percent = section_percent(s.raw_size, s.virtual_size, biggest);
                let name: SharedString = if s.name.is_empty() {
                    t!("(unnamed)")
                } else {
                    s.name.clone().into()
                };
                let sizes = t!(
                    "Raw {raw} · Virtual {virtual_size}",
                    raw = format_size(u64::from(s.raw_size)),
                    virtual_size = format_size(u64::from(s.virtual_size))
                );
                col()
                    .id(("program-section", i))
                    .role(Role::ListItem)
                    .aria_label(format!("{name}: {sizes}"))
                    .test_support()
                    .px(space(2.5))
                    .py(space(1.5))
                    .rounded(theme::size::RADIUS)
                    .border_1()
                    .border_color(t.border)
                    .bg(t.card.opacity(0.4))
                    .child(
                        row_start()
                            .justify_between()
                            .gap(space(2.))
                            .child(
                                div()
                                    .font_family(mono.clone())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.foreground)
                                    .child(name),
                            )
                            .child(align_row(1.).children(s.flags.iter().map(|flag| {
                                let execute = flag == "X";
                                div()
                                    .px(space(1.))
                                    .rounded(theme::size::RADIUS_SM)
                                    .font_family(mono.clone())
                                    .text_size(theme::size::XXS)
                                    .bg(if execute { amber.opacity(0.15) } else { t.muted })
                                    .text_color(if execute { amber } else { t.muted_foreground })
                                    .child(SharedString::from(flag.clone()))
                            }))),
                    )
                    .child(
                        row_start()
                            .mt(space(1.))
                            .justify_between()
                            .gap(space(2.))
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground)
                            .child(sizes)
                            .child(
                                div()
                                    .font_family(mono.clone())
                                    .child(format!("RVA {}", hex_pad(u64::from(s.virtual_address), 6))),
                            ),
                    )
                    .child(
                        row_start()
                            .mt(space(1.))
                            .h(space(1.))
                            .w_full()
                            .overflow_hidden()
                            .rounded_full()
                            .bg(t.muted)
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui_kit::relative(percent / 100.))
                                    .rounded_full()
                                    .bg(t.primary.opacity(0.7)),
                            ),
                    )
            }));
        col()
            .gap(space(3.))
            .children(features)
            .child(facts_view)
            .child(sections)
            .into_any_element()
    }

    /// The Imports tab: a card for each library, with its functions as buttons that copy their names. The cards
    /// differ in height (the functions wrap), so a `list` draws only the ones on screen.
    fn imports_tab(&self, t: &theme::Tokens, mono: &SharedString) -> AnyElement {
        if self.imports.is_empty() {
            return Self::nothing(
                "program-no-imports",
                t!("No matching imported modules or functions."),
                t,
            );
        }
        let imports = self.imports.clone();
        let (tokens, mono) = (t.clone(), mono.clone());
        let cards = list(self.imports_list.clone(), move |i, _, _| {
            let Some(module) = imports.get(i) else {
                return div().into_any_element();
            };
            div()
                .w_full()
                .pb(space(2.))
                .child(import_card(i, module, &tokens, &mono))
                .into_any_element()
        })
        .size_full();
        div()
            .id("program-imports")
            .role(Role::List)
            .aria_label(t!("Imports"))
            .test_support()
            .relative()
            .size_full()
            .child(cards)
            .vertical_scrollbar(&self.imports_list)
            .into_any_element()
    }

    /// A line in the middle of a tab (nothing matches, nothing exported).
    fn nothing(id: &'static str, text: SharedString, t: &theme::Tokens) -> AnyElement {
        div()
            .id(id)
            .role(Role::Status)
            .aria_label(text.clone())
            .test_support()
            .py(space(8.))
            .text_center()
            .text_color(t.muted_foreground)
            .child(text)
            .into_any_element()
    }

    /// The Exports tab: a virtualized list of the symbols, each with its address and a copy button.
    fn exports_tab(&self, info: &PeInfo, t: &theme::Tokens, mono: &SharedString) -> AnyElement {
        if self.exports.is_empty() {
            let text = if info.export_count == 0 {
                t!("This binary exports no symbols.")
            } else {
                t!("No matching exported symbols.")
            };
            return Self::nothing("program-no-exports", text, t);
        }
        let exports = self.exports.clone();
        let (tokens, mono) = (t.clone(), mono.clone());
        let rows = exports.len();
        let rows_list = uniform_list("program-exports", rows, move |range: Range<usize>, _, _| {
            range
                .filter_map(|i| exports.get(i).map(|export| (i, export.clone())))
                .map(|(i, export)| export_row(i, &export, &tokens, &mono))
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.exports_scroll)
        .size_full();
        let cut = (info.export_count > info.exports.len() && self.query_is_empty()).then(|| {
            let text = t!(
                "Showing first {shown} of {total} exports.",
                shown = format_count(info.exports.len()),
                total = format_count(info.export_count)
            );
            div()
                .id("program-exports-cut")
                .role(Role::Status)
                .aria_label(text.clone())
                .test_support()
                .flex_none()
                .pt(space(2.))
                .text_center()
                .text_size(theme::size::XXS)
                .text_color(t.muted_foreground)
                .child(text)
        });
        col()
            .size_full()
            .min_h_0()
            .child(
                div()
                    .id("program-export-list")
                    .role(Role::List)
                    .aria_label(t!("Exports"))
                    .test_support()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(rows_list)
                    .vertical_scrollbar(&self.exports_scroll),
            )
            .children(cut)
            .into_any_element()
    }

    /// Whether the filter box is empty (the footer of the Exports tab is for the unfiltered list).
    fn query_is_empty(&self) -> bool {
        self.imports.len() == self.info().map_or(0, |info| info.imports.len())
            && self.exports.len() == self.info().map_or(0, |info| info.exports.len())
    }
}

/// An import library's card: its name, how many functions, and the ones read as buttons that copy their names.
fn import_card(i: usize, module: &PeImportModule, t: &theme::Tokens, mono: &SharedString) -> impl IntoElement {
    let count = if module.total_count == 1 {
        t!("{n} symbol", n = format_count(module.total_count))
    } else {
        t!("{n} symbols", n = format_count(module.total_count))
    };
    let more = module.total_count.saturating_sub(module.symbols.len());
    col()
        .id(("program-import", i))
        .role(Role::ListItem)
        .aria_label(format!("{}: {count}", module.module))
        .test_support()
        .p(space(2.5))
        .rounded(theme::size::RADIUS_LG)
        .border_1()
        .border_color(t.border)
        .bg(t.card.opacity(0.4))
        .child(
            row_start()
                .justify_between()
                .gap(space(2.))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(mono.clone())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.foreground)
                        .child(SharedString::from(module.module.clone())),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(theme::size::XXS)
                        .text_color(t.muted_foreground)
                        .child(count),
                ),
        )
        .when(!module.symbols.is_empty(), |this| {
            this.child(
                row_start()
                    .mt(space(1.5))
                    .flex_wrap()
                    .gap(space(1.))
                    .children(module.symbols.iter().enumerate().map(|(k, symbol)| {
                        let name = symbol.clone();
                        Button::new(("program-symbol", i * 100_000 + k))
                            .outline()
                            .compact()
                            .font_family(mono.clone())
                            .text_size(theme::size::XXS)
                            .label(SharedString::from(symbol.clone()))
                            .tooltip(t!("Click to copy symbol name"))
                            .on_click(move |_, window, cx| ProgramPreview::copy_symbol(&name, window, cx))
                    }))
                    .when(more > 0, |this| {
                        this.child(
                            div()
                                .id(("program-more", i))
                                .role(Role::Status)
                                .aria_label(t!("+{n} more", n = format_count(more)))
                                .test_support()
                                .px(space(1.))
                                .py(space(0.5))
                                .text_size(theme::size::XXS)
                                .text_color(t.muted_foreground)
                                .child(t!("+{n} more", n = format_count(more))),
                        )
                    }),
            )
        })
}

/// One exported symbol's row (a uniform height): its ordinal, name and address, and a button that copies the name.
fn export_row(i: usize, export: &PeExport, t: &theme::Tokens, mono: &SharedString) -> AnyElement {
    let name = export.name.clone();
    row_start()
        .id(("program-export", i))
        .role(Role::ListItem)
        .aria_label(format!("#{} {}", export.ordinal, export.name))
        .test_support()
        .h(space(8.))
        .px(space(2.5))
        .gap(space(2.))
        .items_center()
        .justify_between()
        .rounded(theme::size::RADIUS)
        .hover(|this| this.bg(t.accent.opacity(0.4)))
        .child(
            row_start()
                .min_w_0()
                .gap(space(2.))
                .child(
                    div()
                        .flex_none()
                        .font_family(mono.clone())
                        .text_size(theme::size::XXS)
                        .text_color(t.muted_foreground)
                        .child(format!("#{}", export.ordinal)),
                )
                .child(
                    div()
                        .truncate()
                        .font_family(mono.clone())
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.foreground)
                        .child(SharedString::from(export.name.clone())),
                ),
        )
        .child(
            row_start()
                .flex_none()
                .gap(space(1.5))
                .child(
                    div()
                        .font_family(mono.clone())
                        .text_size(theme::size::XXS)
                        .text_color(t.muted_foreground)
                        .child(hex_pad(u64::from(export.rva), 6)),
                )
                .child(
                    Button::new(("program-export-copy", i))
                        .ghost()
                        .compact()
                        .icon(IconName::ClipboardCopy)
                        .tooltip(t!("Copy symbol name"))
                        .accessibility_label(t!("Copy symbol name"))
                        .on_click(move |_, window, cx| ProgramPreview::copy_symbol(&name, window, cx)),
                ),
        )
        .into_any_element()
}

impl Render for ProgramPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let root = col()
            .id("program-preview")
            .role(Role::Group)
            .aria_label(t!("Headers of {name}", name = name_of(&self.path)))
            .test_support()
            .size_full()
            .min_h_0()
            .text_start()
            .text_size(theme::size::XS)
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollDown, _, cx| {
                this.page(true, cx);
            }))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollUp, _, cx| {
                this.page(false, cx);
            }));
        let info = match &self.load {
            // The TypeScript drew nothing until the header was read; the spinner shows if it takes a moment.
            Load::Waiting => return root.child(waiting(self.started, None, cx)),
            Load::Failed => {
                return root.child(big_icon(
                    &self.path,
                    Some(t!("No binary header preview for this file")),
                    cx,
                ));
            }
            Load::Ready(info) => info.clone(),
        };
        let mono = theme::mono_font(cx);
        let tabs = align_row(1.)
            .id("program-tabs")
            .role(Role::TabList)
            .aria_label(t!("Header"))
            .test_support()
            .flex_none()
            .px(space(3.))
            .py(space(1.5))
            .border_b_1()
            .border_color(t.border)
            .child(self.tab_button(PeTab::Overview, t!("Sections ({n})", n = info.sections.len()), cx))
            .child(self.tab_button(PeTab::Imports, t!("Imports ({n})", n = info.imports.len()), cx))
            .child(self.tab_button(
                PeTab::Exports,
                t!("Exports ({n})", n = format_count(info.export_count)),
                cx,
            ));
        let filter = (self.tab != PeTab::Overview).then(|| {
            align_row(2.)
                .flex_none()
                .px(space(3.))
                .py(space(1.5))
                .border_b_1()
                .border_color(t.border)
                .child(
                    icon(IconName::Search)
                        .size_3p5()
                        .flex_none()
                        .text_color(t.muted_foreground),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        Input::new(&self.query)
                            .small()
                            .appearance(false)
                            .cleanable(true)
                            .aria_label(if self.tab == PeTab::Exports {
                                t!("Filter exported symbols…")
                            } else {
                                t!("Filter imported DLLs or functions…")
                            }),
                    ),
                )
        });
        let body = match self.tab {
            PeTab::Overview => div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id("program-body")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.body_scroll)
                        .p(space(3.))
                        .child(self.overview(&info, &t, &mono)),
                )
                .vertical_scrollbar(&self.body_scroll),
            PeTab::Imports => div().flex_1().min_h_0().p(space(3.)).child(self.imports_tab(&t, &mono)),
            PeTab::Exports => div()
                .flex_1()
                .min_h_0()
                .p(space(3.))
                .child(self.exports_tab(&info, &t, &mono)),
        };
        root.child(self.chips(&info, &t))
            .child(tabs)
            .children(filter)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use glint_core::pe::PeSection;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::time::Duration;

    fn section(name: &str, raw: u32, virt: u32, flags: &[&str]) -> PeSection {
        PeSection {
            name: name.into(),
            virtual_size: virt,
            virtual_address: 0x1000,
            raw_size: raw,
            raw_offset: 0x400,
            flags: flags.iter().map(|f| (*f).to_string()).collect(),
        }
    }

    fn module(name: &str, symbols: &[&str], total: usize) -> PeImportModule {
        PeImportModule {
            module: name.into(),
            symbols: symbols.iter().map(|s| (*s).to_string()).collect(),
            total_count: total,
        }
    }

    /// A program with an Arabic symbol among its imports and exports (names are bytes, so a library can).
    fn info() -> PeInfo {
        PeInfo {
            format: "PE32+".into(),
            machine: "x64".into(),
            subsystem: "Windows GUI".into(),
            is_dll: false,
            is_dotnet: true,
            is_signed: false,
            timestamp_ms: Some(1_700_000_000_000),
            linker_version: "14.38".into(),
            os_version: "6.0".into(),
            entry_point: 0x1A2B,
            image_base: 0x1_4000_0000,
            checksum: 0,
            features: vec!["ASLR".into(), "DEP".into()],
            sections: vec![
                section(".text", 4096, 3000, &["R", "X"]),
                section(".data", 512, 8192, &["R", "W"]),
                section("", 0, 0, &["R"]),
            ],
            imports: vec![
                module("KERNEL32.dll", &["CreateFileW", "ReadFile", "قراءة"], 120),
                module("USER32.dll", &["MessageBoxW"], 1),
            ],
            export_count: 500,
            exports: (0..400)
                .map(|i| PeExport {
                    ordinal: i + 1,
                    name: if i == 3 {
                        "تصدير".into()
                    } else {
                        format!("Export{i}")
                    },
                    rva: 0x2000 + i * 16,
                })
                .collect(),
            elapsed_ms: 0.42,
        }
    }

    fn fake(answer: Result<PeInfo, String>) -> PeSource {
        Arc::new(move |_| {
            let answer = answer.clone();
            Box::pin(async move { answer })
        })
    }

    fn open(cx: &mut TestAppContext, source: PeSource) -> (Entity<ProgramPreview>, VisualTestContext) {
        cx.update(testing::init);
        let (visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| ProgramPreview::with_source("C:\\apps\\tool.exe".into(), source, window, cx))
        });
        (view, visual)
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the header never arrived");
    }

    fn ready(view: &Entity<ProgramPreview>, visual: &mut VisualTestContext) {
        settle(visual, |cx| view.read_with(cx, |v, _| v.info().is_some()));
        for _ in 0..3 {
            visual.update(|window, cx| window.render_frame(cx));
            visual.run_until_parked();
        }
    }

    #[test]
    fn numbers_are_hex_padded_and_section_bars_are_at_least_two_percent() {
        assert_eq!(hex_pad(0x1A2B, 8), "0x00001A2B");
        assert_eq!(hex_pad(0x1_4000_0000, 12), "0x000140000000");
        assert_eq!(hex_pad(0xABCDEF, 6), "0xABCDEF");
        // The bigger of the two sizes against the biggest section, rounded half up, never under 2.
        assert_eq!(section_percent(4096, 3000, 8192), 50.);
        assert_eq!(section_percent(512, 8192, 8192), 100.);
        assert_eq!(section_percent(0, 0, 8192), 2.);
        assert_eq!(section_percent(0, 0, 0), 2.);
        assert_eq!(section_percent(1, 1, 0), 100.); // nothing bigger than a byte: the biggest is at least 1
        assert_eq!(section_percent(1, 0, 8), 13.); // 12.5 rounds up, as Math.round does
    }

    #[gpui_kit::test]
    fn the_header_shows_its_facts_and_sections_and_arabic_symbols_survive(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        let (view, mut visual) = open(cx, fake(Ok(info())));
        // The read is under way: the UI thread didn't wait for it.
        assert!(view.read_with(&visual, |v, _| v.info().is_none()));
        ready(&view, &mut visual);
        assert!(has(&mut visual, "x64"));
        assert!(has(&mut visual, "PE32+ · Executable"));
        assert!(has(&mut visual, "Windows GUI"));
        assert!(has(&mut visual, ".NET CLR"));
        assert!(has(&mut visual, "Sections (3)"));
        assert!(has(&mut visual, "Imports (2)"));
        assert!(has(&mut visual, "Exports (500)"));
        assert!(has(&mut visual, "ASLR"));
        assert!(has(&mut visual, "Entry point: 0x00001A2B"));
        assert!(has(&mut visual, "Image base: 0x000140000000"));
        assert!(has(&mut visual, "Target OS / Linker: Windows 6.0 · Linker 14.38"));
        // No checksum row when the checksum is zero.
        assert!(!has(&mut visual, "PE checksum: 0x00000000"));
        assert!(has(&mut visual, ".text: Raw 4.0 KB · Virtual 2.9 KB"));
        assert!(has(&mut visual, "(unnamed): Raw 0 B · Virtual 0 B"));
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn imports_filter_by_library_or_function_and_are_drawn_from_a_list(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open(cx, fake(Ok(info())));
        ready(&view, &mut visual);
        visual.update(|window, cx| view.update(cx, |v, cx| v.set_tab(PeTab::Imports, window, cx)));
        ready(&view, &mut visual);
        assert!(has(&mut visual, "KERNEL32.dll: 120 symbols"));
        assert!(has(&mut visual, "USER32.dll: 1 symbol"));
        assert!(
            has(&mut visual, "+117 more"),
            "the symbols past the ones read are counted"
        );
        assert_eq!(view.read_with(&visual, |v, _| v.shown_counts().0), 2);
        // A function name narrows to the library that has it, with only the functions that match.
        visual.update(|window, cx| {
            let query = view.read(cx).query.clone();
            query.update(cx, |q, cx| q.focus(window, cx));
            window.input("قراءة", cx);
        });
        ready(&view, &mut visual);
        assert_eq!(view.read_with(&visual, |v, _| v.shown_counts().0), 1);
        assert!(has(&mut visual, "KERNEL32.dll: 120 symbols"));
        assert!(!has(&mut visual, "USER32.dll: 1 symbol"));
        assert!(has(&mut visual, "قراءة") || has(&mut visual, "Click to copy symbol name"));
    }

    #[gpui_kit::test]
    fn a_program_with_every_import_the_header_allows_only_draws_the_cards_on_screen(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut big = info();
        // 128 libraries of 64 functions each: 8,192 buttons if they were all drawn.
        big.imports = (0..128)
            .map(|m| PeImportModule {
                module: format!("LIB{m}.dll"),
                symbols: (0..64).map(|s| format!("Function{m}_{s}")).collect(),
                total_count: 64,
            })
            .collect();
        let (view, mut visual) = open(cx, fake(Ok(big)));
        ready(&view, &mut visual);
        visual.update(|window, cx| view.update(cx, |v, cx| v.set_tab(PeTab::Imports, window, cx)));
        ready(&view, &mut visual);
        assert!(has(&mut visual, "LIB0.dll: 64 symbols"));
        assert_eq!(view.read_with(&visual, |v, _| v.shown_counts().0), 128);
        let cards = visual.update(|window, _| window.find_all_by_role(Role::ListItem).len());
        assert!(cards > 0 && cards < 20, "{cards} cards drawn");
        let buttons = visual.update(|window, _| window.find_all_by_role(Role::Button).len());
        assert!(buttons < 1000, "{buttons} buttons drawn");
    }

    #[gpui_kit::test]
    fn exports_are_virtualized_and_say_when_there_were_more(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open(cx, fake(Ok(info())));
        ready(&view, &mut visual);
        visual.update(|window, cx| view.update(cx, |v, cx| v.set_tab(PeTab::Exports, window, cx)));
        ready(&view, &mut visual);
        assert!(has(&mut visual, "#1 Export0"));
        assert!(has(&mut visual, "#4 تصدير"), "an Arabic symbol name shows");
        assert!(has(&mut visual, "Showing first 400 of 500 exports."));
        // 400 rows exist; only the ones that fit are drawn.
        let drawn = visual.update(|window, _| window.find_all_by_role(Role::ListItem).len());
        assert!(drawn < 100, "{drawn} rows drawn");
        // The filter takes the ordinal too, and the footer is for the unfiltered list.
        visual.update(|window, cx| {
            let query = view.read(cx).query.clone();
            query.update(cx, |q, cx| q.focus(window, cx));
            window.input("#7", cx);
        });
        ready(&view, &mut visual);
        assert_eq!(view.read_with(&visual, |v, _| v.shown_counts().1), 1);
        assert!(has(&mut visual, "#7 Export6"));
        assert!(!has(&mut visual, "Showing first 400 of 500 exports."));
    }

    #[gpui_kit::test]
    fn a_file_without_a_header_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open(cx, fake(Err("not a PE file".into())));
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.load, Load::Failed))
        });
        assert!(has(&mut visual, "No binary header preview for this file"));
    }

    #[gpui_kit::test]
    fn a_library_that_exports_nothing_says_so_and_the_signature_chip_has_a_place(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut quiet = info();
        quiet.export_count = 0;
        quiet.exports.clear();
        quiet.is_dll = true;
        let (view, mut visual) = open(cx, fake(Ok(quiet)));
        ready(&view, &mut visual);
        assert!(has(&mut visual, "PE32+ · DLL"));
        visual.update(|window, cx| view.update(cx, |v, cx| v.set_tab(PeTab::Exports, window, cx)));
        ready(&view, &mut visual);
        assert!(has(&mut visual, "This binary exports no symbols."));
        // The chip is another view's; the header only gives it room.
        let chip = visual.update(|_, cx| cx.new(|_| Chip).into());
        visual.update(|_, cx| view.update(cx, |v, cx| v.set_signature(Some(chip), cx)));
        assert!(has(&mut visual, "Signed by Rafd"));
    }

    struct Chip;

    impl Render for Chip {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("test-signature")
                .role(Role::Group)
                .aria_label("Signed by Rafd")
                .test_support()
                .child("Signed by Rafd")
        }
    }

    #[test]
    fn every_icon_the_program_view_draws_is_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::Cpu,
            IconName::Layers,
            IconName::ShieldCheck,
            IconName::Zap,
            IconName::Search,
            IconName::ClipboardCopy,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }
}
