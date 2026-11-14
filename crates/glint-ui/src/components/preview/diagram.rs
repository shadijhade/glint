//! Graphviz diagrams, in Markdown and as files (SmallPreviews.tsx's `Graphviz`; docs/gpui/previews-native.md
//! section 6). (Mermaid was drawn here too, by merman, until 0.12: a ```` ```mermaid ```` block now shows as code
//! and a `.mmd` file as text.)
//!
//! | TypeScript                                          | Here                                                      |
//! |-----------------------------------------------------|-----------------------------------------------------------|
//! | `@hpcc-js/wasm-graphviz` `dot(source)`              | [`dot_svg`]: layout-rs parses and lays out the DOT, then the SVG is themed |
//! | `<img src="data:image/svg+xml,…">`                  | [`super::svg_image`]: rasterised by GPUI's resvg, cached    |
//! | the source and "Not a graph Graphviz can draw: …"   | [`super::svg_image::error_banner`]: the diagnostic, then the source |
//!
//! # How a diagram is made
//!
//! [`request`] builds a job for [`super::svg_image::request`]: on a background thread layout-rs makes an SVG,
//! which is rasterised there too. Each has a **time budget** ([`BUDGET`]) and **size caps** ([`MAX_SOURCE`] bytes
//! of source, [`MAX_NODES`] nodes, [`MAX_EDGES`] edges). layout-rs has no way to be told to stop, so it runs on a
//! thread of its own and is abandoned at the deadline; no more than [`MAX_LAYOUTS`] may be running (or abandoned and
//! still running) at once, a further graph waits for a place within its own budget. A panic inside it (layout-rs
//! asserts on some input) is caught and reported.
//!
//! # Not ported / differences
//!
//! - Graphviz is not Graphviz: layout-rs draws a subset of DOT (no HTML-like labels, `splines=ortho`, ports on every
//!   shape, nested clusters; only the layered engine, whatever `layout=` says). The layouts are not the same as
//!   `dot`'s, and the TypeScript's white page behind the graph is the app's own background here, with the graph's
//!   default lines and text in the app's text colour.
//! - Hyperlinks, tooltips and click handlers of a diagram do nothing (the SVG is a picture, as it was in the
//!   TypeScript's `<img>`).

use super::svg_image::{self, Ctl, Lookup};
use crate::kit::prelude::*;
use crate::t;
use gpui_kit::EntityId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

/// The longest a diagram may take to draw (the TypeScript had none: a stuck diagram froze the page).
pub const BUDGET: Duration = Duration::from_secs(10);
/// The most source a diagram may have, in bytes.
pub const MAX_SOURCE: usize = 200_000;
/// The most nodes a DOT graph may have.
pub const MAX_NODES: usize = 400;
/// The most edges a DOT graph may have.
pub const MAX_EDGES: usize = 1000;
/// The most work layout-rs may be asked for: see [`layout_cost`]. About two seconds in a release build.
pub const MAX_LAYOUT_STEPS: f64 = 2e8;
/// The most DOT layouts running (or abandoned and still running) at once.
pub const MAX_LAYOUTS: usize = 3;
/// The family diagrams are drawn in and measured with: Segoe UI has Arabic and falls back for the rest.
pub const FONT: &str = "Segoe UI";
/// The stack a DOT layout thread gets (layout-rs recurses on big graphs).
const LAYOUT_STACK: usize = 32 << 20;

/// The languages drawn here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    /// Graphviz's DOT.
    Dot,
}

impl Language {
    /// The language a code fence's info word names: `dot`, `graphviz` and `gv`.
    pub fn of_fence(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "dot" | "graphviz" | "gv" => Some(Language::Dot),
            _ => None,
        }
    }

    /// The word the cache and accessibility names use.
    pub fn kind(self) -> &'static str {
        match self {
            Language::Dot => "dot",
        }
    }

    /// What a picture of it is called (the TypeScript's alt text).
    pub fn picture_name(self) -> SharedString {
        match self {
            Language::Dot => t!("A Graphviz graph"),
        }
    }

    /// Why a diagram in a Markdown document couldn't be drawn (`Not a graph Graphviz can draw: …`).
    pub fn failure_in_document(self, error: &str) -> SharedString {
        let error = trimmed(error);
        match self {
            Language::Dot => t!("Not a graph Graphviz can draw: {error}", error = error),
        }
    }

    /// Why a diagram file couldn't be drawn (`Graphviz couldn't draw it: …`).
    pub fn failure_in_file(self, error: &str) -> SharedString {
        let error = trimmed(error);
        match self {
            Language::Dot => t!("Graphviz couldn't draw it: {error}", error = error),
        }
    }
}

/// A diagnostic cut to a reasonable length, on one line.
fn trimmed(error: &str) -> String {
    let flat: String = error.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 400 {
        let cut: String = flat.chars().take(400).collect();
        format!("{cut}…")
    } else {
        flat
    }
}

/// `#rrggbb` for a colour (its alpha is ignored).
pub fn hex(color: Hsla) -> String {
    let rgba: Rgba = color.into();
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(rgba.r), byte(rgba.g), byte(rgba.b))
}

// ---- Right-to-left text, as resvg draws it ------------------------------------------------------------

/// Whether `c` is written right to left (Hebrew, Arabic, Syriac, Thaana, N'Ko and their presentation forms).
pub(super) fn is_rtl_char(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)
}

/// A length in an SVG attribute as user units: `12`, `12px`, `1.5em` (at `size`).
fn user_units(value: &str, size: f64) -> Option<f64> {
    let value = value.trim();
    if let Some(em) = value.strip_suffix("em") {
        return em.trim().parse::<f64>().ok().map(|n| n * size);
    }
    value.strip_suffix("px").unwrap_or(value).trim().parse::<f64>().ok()
}

/// The value of attribute `name` in `tag` and where it sits (the span covers `name="value"` with its space).
fn attribute<'a>(tag: &'a str, name: &str) -> Option<(std::ops::Range<usize>, &'a str)> {
    let needle = format!(" {name}=\"");
    let at = tag.find(&needle)?;
    let value_start = at + needle.len();
    let value_end = value_start + tag[value_start..].find('"')?;
    Some((at..value_end + 1, &tag[value_start..value_end]))
}

/// A `font-size` in a tag: its attribute, or `font-size:12px` in its style.
fn font_size_in(tag: &str) -> Option<f64> {
    if let Some((_, value)) = attribute(tag, "font-size") {
        return user_units(value, 16.);
    }
    let (_, style) = attribute(tag, "style")?;
    let at = style.find("font-size:")? + "font-size:".len();
    let rest = style[at..].trim_start();
    let end = rest.find([';', '"']).unwrap_or(rest.len());
    user_units(&rest[..end], 16.)
}

/// The size the stylesheet gives the diagram's text (`font-size:16px` in the first rule that has one).
fn stylesheet_font_size(svg: &str) -> f64 {
    let start = svg.find("<style").unwrap_or(0);
    svg[start..]
        .find("font-size:")
        .and_then(|at| {
            let rest = svg[start + at + "font-size:".len()..].trim_start();
            let end = rest.find([';', '}', '<']).unwrap_or(rest.len());
            user_units(&rest[..end], 16.)
        })
        .filter(|size| *size > 1.)
        .unwrap_or(16.)
}

/// `value` as a short attribute number.
fn number(value: f64) -> String {
    let text = format!("{value:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Sets (or adds) the `y` of a start tag and drops its `dy`.
fn with_absolute_y(tag: &str, y: f64) -> String {
    let mut tag = tag.to_string();
    if let Some((span, _)) = attribute(&tag, "dy") {
        tag.replace_range(span, "");
    }
    match attribute(&tag, "y") {
        Some((span, _)) => tag.replace_range(span, &format!(" y=\"{}\"", number(y))),
        None => {
            let name_end = tag
                .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                .unwrap_or(tag.len());
            tag.insert_str(name_end, &format!(" y=\"{}\"", number(y)));
        }
    }
    tag
}

/// resvg draws a `dy` on the first character only when the text runs right to left (Arabic and Hebrew labels
/// came out with their first letter a line below the rest). Mermaid and layout-rs place their lines with `dy`
/// (`<tspan x="0" y="-0.1em" dy="1.1em">`), so for every `<text>` that has right-to-left letters the line's `y` is
/// worked out here (the `y` it had, or where the previous line ended, plus `dy`, in user units) and the `dy` is
/// dropped. Text without right-to-left letters is left as it was.
pub fn fix_rtl_text(svg: &str) -> String {
    if !svg.chars().any(is_rtl_char) {
        return svg.to_string();
    }
    let default_size = stylesheet_font_size(svg);
    let mut out = String::with_capacity(svg.len() + 64);
    let mut at = 0;
    // The text being rewritten: its font size and where its last line sat.
    let mut current: Option<(f64, f64)> = None;
    while let Some(open) = svg[at..].find('<') {
        out.push_str(&svg[at..at + open]);
        at += open;
        let Some(close) = svg[at..].find('>') else { break };
        let tag = &svg[at..=at + close];
        at += close + 1;
        if tag.starts_with("<text") && tag[5..].starts_with(|c: char| c.is_whitespace() || c == '>') {
            let rtl = !tag.ends_with("/>")
                && svg[at..]
                    .find("</text>")
                    .is_some_and(|end| svg[at..at + end].chars().any(is_rtl_char));
            if rtl {
                let size = font_size_in(tag).unwrap_or(default_size);
                let y = attribute(tag, "y").and_then(|(_, v)| user_units(v, size)).unwrap_or(0.);
                match attribute(tag, "dy").and_then(|(_, v)| user_units(v, size)) {
                    Some(dy) => {
                        out.push_str(&with_absolute_y(tag, y + dy));
                        current = Some((size, y + dy));
                    }
                    None => {
                        out.push_str(tag);
                        current = Some((size, y));
                    }
                }
                continue;
            }
        } else if tag.starts_with("</text") {
            current = None;
        } else if tag.starts_with("<tspan") {
            if let Some((text_size, cursor)) = current {
                let size = font_size_in(tag).unwrap_or(text_size);
                let own_y = attribute(tag, "y").and_then(|(_, v)| user_units(v, size));
                let dy = attribute(tag, "dy").and_then(|(_, v)| user_units(v, size));
                match dy {
                    Some(dy) => {
                        let y = own_y.unwrap_or(cursor) + dy;
                        out.push_str(&with_absolute_y(tag, y));
                        current = Some((text_size, y));
                    }
                    None => {
                        out.push_str(tag);
                        if let Some(y) = own_y {
                            current = Some((text_size, y));
                        }
                    }
                }
                continue;
            }
        }
        out.push_str(tag);
    }
    out.push_str(&svg[at..]);
    out
}

fn check_source(source: &str) -> Result<(), String> {
    if source.trim().is_empty() {
        return Err("There is nothing in it".to_string());
    }
    if source.len() > MAX_SOURCE {
        return Err(format!("It is too big to draw here (over {} KB)", MAX_SOURCE / 1000));
    }
    Ok(())
}

// ---- Graphviz -------------------------------------------------------------------------------------------

static LAYOUTS: AtomicUsize = AtomicUsize::new(0);

/// Counts a running layout thread; the count drops when the thread ends, even if it was abandoned.
struct Running;

impl Running {
    /// A place to lay a graph out: at once if fewer than [`MAX_LAYOUTS`] are running, else as soon as one ends
    /// (a document with ten graphs draws them three at a time). None when the job is cancelled or out of time first.
    fn acquire(ctl: &Ctl) -> Option<Running> {
        loop {
            if LAYOUTS.fetch_add(1, Ordering::SeqCst) < MAX_LAYOUTS {
                return Some(Running);
            }
            LAYOUTS.fetch_sub(1, Ordering::SeqCst);
            if ctl.cancelled() || ctl.remaining().is_zero() {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        LAYOUTS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// DOT allows identifiers with any non-ASCII letters, unquoted (`مرحبا -> world`); layout-rs's lexer stops at the
/// first one ("Unknown token"). Quotes every such identifier outside strings, comments and HTML-like labels, so
/// `مرحبا -> world` reads as `"مرحبا" -> world`.
pub fn quote_unicode_ids(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len() + 16);
    let mut i = 0;
    let mut line_start = true;
    let copy_until = |from: usize, end: &dyn Fn(&[char], usize) -> Option<usize>| -> usize {
        let mut j = from;
        while j < chars.len() {
            if let Some(next) = end(&chars, j) {
                return next;
            }
            j += 1;
        }
        chars.len()
    };
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let end = if c == '"' {
            // A string: to the closing quote, a backslash skipping one character.
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '"' {
                j += if chars[j] == '\\' { 2 } else { 1 };
            }
            (j + 1).min(chars.len())
        } else if c == '/' && next == Some('/') || c == '#' && line_start {
            copy_until(i, &|chars, j| (chars[j] == '\n').then_some(j))
        } else if c == '/' && next == Some('*') {
            copy_until(i + 2, &|chars, j| {
                (chars[j] == '*' && chars.get(j + 1) == Some(&'/')).then_some(j + 2)
            })
        } else if c == '<' {
            // An HTML-like label: balanced angle brackets.
            let mut depth = 0usize;
            let mut j = i;
            while j < chars.len() {
                match chars[j] {
                    '<' => depth += 1,
                    '>' => {
                        depth -= 1;
                        if depth == 0 {
                            j += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            j.min(chars.len())
        } else {
            i
        };
        if end > i {
            out.extend(&chars[i..end]);
            line_start = chars[end - 1] == '\n';
            i = end;
            continue;
        }
        if c.is_ascii_alphanumeric() || c == '_' || !c.is_ascii() && !c.is_whitespace() {
            let mut j = i;
            while j < chars.len()
                && (chars[j].is_ascii_alphanumeric()
                    || chars[j] == '_'
                    || !chars[j].is_ascii() && !chars[j].is_whitespace())
            {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            if word.is_ascii() {
                out.push_str(&word);
            } else {
                out.push('"');
                out.push_str(&word.replace('"', "\\\""));
                out.push('"');
            }
            i = j;
            line_start = false;
            continue;
        }
        out.push(c);
        line_start = c == '\n' || line_start && c.is_whitespace();
        i += 1;
    }
    out
}

/// The nodes of a parsed graph, in the order its statements name them, and its edges as pairs of them (a chain
/// `a -> b -> c` is two), subgraphs included.
fn nodes_and_edges(graph: &layout::gv::parser::ast::Graph) -> (usize, Vec<(usize, usize)>) {
    use layout::gv::parser::ast::Stmt;
    fn visit(
        graph: &layout::gv::parser::ast::Graph,
        ids: &mut std::collections::HashMap<String, usize>,
        edges: &mut Vec<(usize, usize)>,
    ) {
        let id_of = |name: &str, ids: &mut std::collections::HashMap<String, usize>| {
            let next = ids.len();
            *ids.entry(name.to_string()).or_insert(next)
        };
        for stmt in &graph.list.list {
            match stmt {
                Stmt::Node(node) => {
                    id_of(&node.id.name, ids);
                }
                Stmt::Edge(edge) => {
                    let mut previous = id_of(&edge.from.name, ids);
                    for (to, _) in &edge.to {
                        let current = id_of(&to.name, ids);
                        edges.push((previous, current));
                        previous = current;
                    }
                }
                Stmt::SubGraph(inner) => visit(inner, ids, edges),
                Stmt::Attribute(_) => {}
            }
        }
    }
    let mut ids = std::collections::HashMap::new();
    let mut edges = Vec::new();
    visit(graph, &mut ids, &mut edges);
    (ids.len(), edges)
}

/// How many steps layout-rs's cycle checks will take on these edges, as a count (it saturates at `limit`).
///
/// layout-rs checks every edge it adds against a cycle with a search that tries *every path* from the edge's end
/// (it unmarks a node when it backs out), and re-checks every edge after each addition. A graph where many
/// paths run through the same nodes (a chain whose nodes each also link two steps ahead has Fibonacci-many) takes
/// time exponential in its size: 40 such nodes took two minutes in a debug build, and the work can't be
/// interrupted. This repeats layout-rs's own insertion order and counts those searches, with the path counts
/// worked out once per node, so a graph it could not finish in time is refused before it is given to it.
pub fn layout_cost(nodes: usize, edges: &[(usize, usize)], limit: f64) -> f64 {
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); nodes];
    let mut total = 0.0_f64;
    for &(from, to) in edges {
        if from == to {
            continue;
        }
        // Reverse an edge that would close a cycle (an ordinary search with a visited set: cheap).
        let (from, to) = if reaches(&successors, to, from) {
            (to, from)
        } else {
            (from, to)
        };
        let paths = path_counts(&successors);
        total += paths[to];
        successors[from].push(to);
        let paths = path_counts(&successors);
        total += successors.iter().flatten().map(|&target| paths[target]).sum::<f64>();
        if total > limit {
            return limit;
        }
    }
    total
}

/// Whether `target` can be reached from `start` along `successors`.
fn reaches(successors: &[Vec<usize>], start: usize, target: usize) -> bool {
    let mut seen = vec![false; successors.len()];
    let mut stack = vec![start];
    while let Some(node) = stack.pop() {
        if node == target {
            return true;
        }
        if std::mem::replace(&mut seen[node], true) {
            continue;
        }
        stack.extend(successors[node].iter().copied());
    }
    false
}

/// For each node of an acyclic graph, how many paths a search that backs out of every node tries from it
/// (itself, plus those of each of its successors; repeated edges count again).
fn path_counts(successors: &[Vec<usize>]) -> Vec<f64> {
    let mut counts = vec![f64::NAN; successors.len()];
    for start in 0..successors.len() {
        if !counts[start].is_nan() {
            continue;
        }
        let mut stack = vec![(start, 0usize)];
        while let Some((node, next)) = stack.pop() {
            if let Some(&child) = successors[node].get(next) {
                stack.push((node, next + 1));
                if counts[child].is_nan() {
                    stack.push((child, 0));
                }
            } else {
                counts[node] = 1.0 + successors[node].iter().map(|&child| counts[child]).sum::<f64>();
            }
        }
    }
    counts
}

/// DOT to an SVG with layout-rs, unthemed.
fn layout_dot(source: &str) -> Result<String, String> {
    let quoted = quote_unicode_ids(source);
    let mut parser = layout::gv::DotParser::new(&quoted);
    let graph = parser
        .process()
        .map_err(|error| format!("Syntax error in the DOT: {error}"))?;
    let (nodes, edges) = nodes_and_edges(&graph);
    if nodes > MAX_NODES {
        return Err(format!("It has {nodes} nodes, more than the {MAX_NODES} Glint draws"));
    }
    if edges.len() > MAX_EDGES {
        return Err(format!(
            "It has {} edges, more than the {MAX_EDGES} Glint draws",
            edges.len()
        ));
    }
    if layout_cost(nodes, &edges, MAX_LAYOUT_STEPS) >= MAX_LAYOUT_STEPS {
        return Err("Its edges make too many paths for Glint to lay the graph out in time".to_string());
    }
    let mut builder = layout::gv::GraphBuilder::new();
    builder.visit_graph(&graph);
    let mut visual = builder.get();
    let mut svg = layout::backends::svg::SVGWriter::new();
    visual.do_it(false, false, false, &mut svg);
    Ok(svg.finalize())
}

/// What layout-rs draws in is its fixed Times, serif; its page is bare. The page becomes white (so a graph's own
/// colours, which Graphviz draws on white, keep their meaning in the dark theme too, as the TypeScript's white
/// background did) and the font the app's.
pub fn theme_dot_svg(svg: &str) -> String {
    let mut out = svg.replace(
        "font-family: Times, serif;",
        &format!("font-family: \"{FONT}\", sans-serif;"),
    );
    if let Some(at) = out.find("<defs>") {
        out.insert_str(at, "<rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/>");
    }
    out
}

/// The SVG layout-rs makes of DOT `source`, on a white page. Laid out on a
/// thread of its own with the deadline of `ctl`; see the module notes.
pub fn dot_svg(source: &str, ctl: &Ctl) -> Result<String, String> {
    check_source(source)?;
    let Some(guard) = Running::acquire(ctl) else {
        return Err(if ctl.cancelled() {
            "Cancelled".to_string()
        } else {
            "Glint is busy drawing other graphs: try again in a moment".to_string()
        });
    };
    let (send, receive) = mpsc::channel();
    let owned = source.to_string();
    std::thread::Builder::new()
        .name("glint-dot-layout".into())
        .stack_size(LAYOUT_STACK)
        .spawn(move || {
            let _running = guard;
            let result = std::panic::catch_unwind(|| layout_dot(&owned))
                .unwrap_or_else(|_| Err("The layout failed on this graph".to_string()));
            send.send(result).ok();
        })
        .map_err(|error| error.to_string())?;
    // Wait in short steps so a cancelled job stops waiting.
    loop {
        match receive.recv_timeout(Duration::from_millis(50).min(ctl.remaining().max(Duration::from_millis(1)))) {
            Ok(result) => return result.map(|svg| fix_rtl_text(&theme_dot_svg(&svg))),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if ctl.cancelled() {
                    return Err("Cancelled".to_string());
                }
                if ctl.remaining().is_zero() {
                    return Err(format!("It took longer than {} seconds to lay out", BUDGET.as_secs()));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err("The layout stopped unexpectedly".to_string()),
        }
    }
}

// ---- Asking for a picture -------------------------------------------------------------------------------

/// The cache key of `source` drawn as `language` (a Graphviz graph is drawn on its own white page, so it is the same
/// in both colour sets).
pub fn key_of(language: Language, source: &str) -> svg_image::Key {
    svg_image::Key::new(language.kind(), source, 0., false)
}

/// The picture of `source` as `language`: made if it hasn't been (on a background thread; `wake` is notified when
/// it is done), else returned.
pub fn request(cx: &mut App, language: Language, source: &Arc<str>, wake: EntityId) -> Lookup {
    let source = source.clone();
    svg_image::request(cx, key_of(language, &source), wake, BUDGET, move |renderer, ctl| {
        let svg = match language {
            Language::Dot => dot_svg(&source, ctl)?,
        };
        if ctl.cancelled() {
            return Err("Cancelled".to_string());
        }
        svg_image::rasterize(renderer, &svg)
    })
}

/// The SVG of `source`, without GPUI (for tests and callers that rasterise themselves).
pub fn svg_of(language: Language, source: &str) -> Result<String, String> {
    let ctl = Ctl::new(BUDGET);
    match language {
        Language::Dot => dot_svg(source, &ctl),
    }
}

// ---- The Markdown plugin --------------------------------------------------------------------------------

/// The name of the diagram block node.
pub const BLOCK: &str = "glint-diagram";

/// A fenced diagram as the Markdown parser hands it over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fence {
    /// What the fence says it is.
    pub language: Language,
    /// What is inside it.
    pub source: Arc<str>,
}

/// ```` ```mermaid ````, ```` ```dot ````, ```` ```graphviz ```` and ```` ```gv ```` blocks, drawn (the TypeScript drew only Mermaid in
/// Markdown; DOT blocks are new).
pub struct DiagramBlock {
    /// The view to wake when a diagram's picture is ready.
    pub wake: EntityId,
}

impl gpui_kit::base::text::MarkdownPlugin for DiagramBlock {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        BLOCK
    }

    fn parse(
        &self,
        node: &gpui_kit::base::text::markdown_ast::Node,
        cx: &gpui_kit::base::text::MarkdownParseContext<'_>,
    ) -> Option<gpui_kit::base::text::MarkdownNode> {
        let gpui_kit::base::text::markdown_ast::Node::Code(code) = node else {
            return None;
        };
        let language = Language::of_fence(code.lang.as_deref()?)?;
        let written = cx.node_source(node).unwrap_or_default().to_string();
        Some(
            gpui_kit::base::text::MarkdownNode::new(
                BLOCK,
                Fence {
                    language,
                    source: Arc::from(code.value.as_str()),
                },
            )
            .text(code.value.clone())
            .markdown(written)
            .accessibility_label(language.picture_name()),
        )
    }

    fn render(&self, node: &gpui_kit::base::text::MarkdownNode, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(fence) = node.data::<Fence>() else {
            return div().into_any_element();
        };
        svg_image::release_retired(window);
        let start = node.source_range().map_or(0, |range| range.start) as u64;
        let name = fence.language.picture_name();
        let body = match request(cx, fence.language, &fence.source, self.wake) {
            Lookup::Done(outcome) => match outcome.as_ref() {
                Ok(drawn) => div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .justify_center()
                    .child(svg_image::picture(
                        ElementId::NamedInteger("diagram".into(), start),
                        drawn,
                        name,
                    ))
                    .into_any_element(),
                Err(error) => svg_image::error_banner(
                    "diagram-error",
                    start,
                    fence.language.failure_in_document(error),
                    &fence.source,
                    cx,
                )
                .into_any_element(),
            },
            // While it is made, the source shows (as the TypeScript's did).
            Lookup::Making => div()
                .id(ElementId::NamedInteger("diagram-pending".into(), start))
                .role(Role::Group)
                .aria_label(t!("Drawing the diagram…"))
                .test_support()
                .w_full()
                .min_w_0()
                .child(svg_image::source_block(
                    ElementId::NamedInteger("diagram-pending-source".into(), start),
                    &fence.source,
                    cx,
                ))
                .into_any_element(),
        };
        div().w_full().min_w_0().py(space(1.)).child(body).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::TestAppContext;

    const DOT: &str = "digraph G { rankdir=LR; a -> b [label=\"edge\"]; b -> c; c -> a; }";

    // ---- The languages --------------------------------------------------------------------------------------------

    #[test]
    fn fences_name_their_language() {
        assert_eq!(Language::of_fence("mermaid"), None);
        assert_eq!(Language::of_fence("dot"), Some(Language::Dot));
        assert_eq!(Language::of_fence("graphviz"), Some(Language::Dot));
        assert_eq!(Language::of_fence("gv"), Some(Language::Dot));
        assert_eq!(Language::of_fence("rust"), None);
        assert_eq!(Language::of_fence(""), None);
    }

    #[test]
    fn colours_are_written_as_hex() {
        assert_eq!(hex(gpui_kit::rgb(0x336699).into()), "#336699");
        assert_eq!(hex(gpui_kit::black()), "#000000");
        assert_eq!(hex(gpui_kit::white()), "#ffffff");
    }

    #[test]
    fn a_diagnostic_is_one_short_line() {
        assert_eq!(trimmed("a\n  b\tc"), "a b c");
        let long = "x".repeat(1000);
        assert!(trimmed(&long).chars().count() <= 401);
    }

    // ---- Right-to-left text, as resvg wants it ---------------------------------------------------------------------

    #[test]
    fn a_dy_on_an_arabic_line_becomes_an_absolute_y() {
        // As Mermaid writes a wrapped label: y and dy on each line, in em.
        let svg = r#"<svg><style>.a{font-size:16px;}</style><g><text y="-10"><tspan class="row" x="0" y="-0.1em" dy="1.1em"><tspan>مرحبا</tspan></tspan><tspan class="row" x="0" y="1em" dy="1.1em"><tspan>بالعالم</tspan></tspan></text></g></svg>"#;
        let fixed = fix_rtl_text(svg);
        assert!(!fixed.contains("dy="), "{fixed}");
        // -0.1em + 1.1em = 1em = 16, and 1em + 1.1em = 2.1em = 33.6.
        assert!(
            fixed.contains(r#"x="0" y="16""#) && fixed.contains(r#"x="0" y="33.6""#),
            "{fixed}"
        );
        // The text is untouched.
        assert!(fixed.contains("<tspan>مرحبا</tspan>") && fixed.contains("<tspan>بالعالم</tspan>"));
    }

    #[test]
    fn lines_without_a_y_follow_the_one_before_as_layout_rs_writes_them() {
        let svg = r#"<svg><text dominant-baseline="middle" x="50" y="20" class="a14"><tspan x = "50" dy="1.0em">أول</tspan><tspan x = "50" dy="1.0em">ثاني</tspan></text></svg>"#;
        let fixed = fix_rtl_text(svg);
        assert!(!fixed.contains("dy="), "{fixed}");
        // The font size is the stylesheet's default of 16 here: 20 + 16, then + 16 again.
        assert!(fixed.contains(r#"y="36""#) && fixed.contains(r#"y="52""#), "{fixed}");
    }

    #[test]
    fn a_dy_on_the_text_itself_moves_its_y() {
        let svg =
            r#"<svg><text x="178" y="135" class="messageText" dy="1em" style="font-size:20px">مرحبا</text></svg>"#;
        let fixed = fix_rtl_text(svg);
        assert!(!fixed.contains("dy="), "{fixed}");
        assert!(fixed.contains(r#"y="155""#), "{fixed}");
    }

    #[test]
    fn latin_text_is_left_exactly_as_it_was() {
        let svg = r#"<svg><text y="-10"><tspan x="0" y="-0.1em" dy="1.1em">Hello</tspan></text></svg>"#;
        assert_eq!(fix_rtl_text(svg), svg);
        // And an Arabic word elsewhere in the picture doesn't change a Latin text.
        let mixed = r#"<svg><text y="5"><tspan dy="1em">Hello</tspan></text><text y="5"><tspan dy="1em">مرحبا</tspan></text></svg>"#;
        let fixed = fix_rtl_text(mixed);
        assert!(fixed.contains(r#"<tspan dy="1em">Hello</tspan>"#), "{fixed}");
        assert!(fixed.contains(r#"y="21""#), "{fixed}");
    }

    // ---- DOT ------------------------------------------------------------------------------------------------------

    #[test]
    fn unicode_identifiers_are_quoted_outside_strings_and_comments() {
        assert_eq!(
            quote_unicode_ids("digraph { مرحبا -> world }"),
            "digraph { \"مرحبا\" -> world }"
        );
        assert_eq!(quote_unicode_ids("a -> b"), "a -> b");
        // Strings, comments and HTML-like labels are left alone.
        assert_eq!(quote_unicode_ids("a [label=\"مرحبا\"]"), "a [label=\"مرحبا\"]");
        assert_eq!(quote_unicode_ids("a -> b // مرحبا\nc"), "a -> b // مرحبا\nc");
        assert_eq!(quote_unicode_ids("a /* مرحبا */ b"), "a /* مرحبا */ b");
        assert_eq!(quote_unicode_ids("# مرحبا\nx"), "# مرحبا\nx");
        assert_eq!(
            quote_unicode_ids("a [label=<<b>مرحبا</b>>]"),
            "a [label=<<b>مرحبا</b>>]"
        );
        // A mixed identifier is quoted whole.
        assert_eq!(quote_unicode_ids("café -> b"), "\"café\" -> b");
        // An escaped quote doesn't end a string early.
        assert_eq!(quote_unicode_ids(r#"a [label="x\"مرحبا"]"#), r#"a [label="x\"مرحبا"]"#);
    }

    #[test]
    fn a_dot_svg_gets_a_white_page_and_the_apps_font() {
        let raw = "<svg><defs></defs><style>
.a14 { font-size: 14px; font-family: Times, serif; }
</style><rect stroke=\"#000000ff\"/></svg>";
        let themed = theme_dot_svg(raw);
        assert!(
            themed.contains("font-family: \"Segoe UI\", sans-serif;") && !themed.contains("Times"),
            "{themed}"
        );
        // The page is drawn first, under everything.
        assert!(
            themed.starts_with("<svg><rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/><defs>"),
            "{themed}"
        );
        // An SVG without defs is only given the font.
        assert_eq!(theme_dot_svg("<svg/>"), "<svg/>");
    }

    #[test]
    fn dot_is_laid_out_on_a_white_page() {
        let drawn = svg_of(Language::Dot, DOT).expect("it draws");
        assert!(drawn.contains("<svg") && drawn.contains("edge"), "{drawn}");
        assert!(
            drawn.contains("fill=\"#ffffff\"") && drawn.contains("Segoe UI"),
            "{drawn}"
        );
    }

    #[test]
    fn broken_dot_says_where_it_went_wrong() {
        for source in ["digraph { a -> }", "graph { a -- b", "not dot"] {
            let reason = svg_of(Language::Dot, source).expect_err(source);
            assert!(reason.starts_with("Syntax error in the DOT: "), "{reason}");
        }
        assert!(svg_of(Language::Dot, "").is_err());
        assert!(svg_of(Language::Dot, "   \n").is_err());
    }

    #[test]
    fn a_dot_graph_with_too_many_nodes_is_refused() {
        let mut source = String::from("digraph {\n");
        for n in 0..=MAX_NODES {
            source.push_str(&format!("  n{n};\n"));
        }
        source.push('}');
        let reason = svg_of(Language::Dot, &source).expect_err("too many");
        assert!(
            reason.contains("nodes") && reason.contains(&MAX_NODES.to_string()),
            "{reason}"
        );
    }

    #[test]
    fn a_dot_graph_with_too_many_edges_is_refused() {
        let mut source = String::from(
            "digraph {
",
        );
        for n in 0..=MAX_EDGES {
            source.push_str(&format!(
                "  a -> b{};
",
                n % 7
            ));
        }
        source.push('}');
        let reason = svg_of(Language::Dot, &source).expect_err("too many");
        assert!(
            reason.contains("edges") && reason.contains(&MAX_EDGES.to_string()),
            "{reason}"
        );
    }

    /// `n` nodes, each with an edge to the next and to the one after: Fibonacci-many paths.
    fn skipping_chain(n: usize) -> String {
        let mut source = String::from(
            "digraph {
",
        );
        for k in 0..n {
            source.push_str(&format!(
                "  n{k} -> n{};
  n{k} -> n{};
",
                k + 1,
                k + 2
            ));
        }
        source.push('}');
        source
    }

    #[test]
    fn the_cost_of_a_layout_grows_with_the_paths_not_the_edges() {
        let chain = |n: usize| -> (usize, Vec<(usize, usize)>) { (n + 1, (0..n).map(|k| (k, k + 1)).collect()) };
        let skipping = |n: usize| -> (usize, Vec<(usize, usize)>) {
            (n + 3, (0..n).flat_map(|k| [(k, k + 1), (k, k + 2)]).collect())
        };
        // A long plain chain is cheap, however long.
        let (nodes, edges) = chain(300);
        assert!(layout_cost(nodes, &edges, 1e12) < 1e9);
        // With a skip edge on every node the cost multiplies by about 1.6 a node.
        let cost = |n: usize| {
            let (nodes, edges) = skipping(n);
            layout_cost(nodes, &edges, 1e18)
        };
        assert!(cost(20) > 1e4 && cost(20) < MAX_LAYOUT_STEPS, "{}", cost(20));
        assert!(cost(30) > 30. * cost(20), "{} vs {}", cost(30), cost(20));
        assert!(cost(45) > MAX_LAYOUT_STEPS);
        // A cost that passes the limit stops being counted.
        let (nodes, edges) = skipping(60);
        assert_eq!(layout_cost(nodes, &edges, 1e6), 1e6);
        // A cycle is a back edge to reverse, not a crash or a loop.
        assert!(layout_cost(3, &[(0, 1), (1, 2), (2, 0), (1, 1)], 1e9) < 1e3);
        // Repeated edges count again.
        assert!(layout_cost(2, &[(0, 1), (0, 1), (0, 1)], 1e9) > layout_cost(2, &[(0, 1)], 1e9));
    }

    #[test]
    fn a_graph_layout_rs_could_not_finish_in_time_is_refused_up_front() {
        let started = std::time::Instant::now();
        let reason = svg_of(Language::Dot, &skipping_chain(60)).expect_err("refused");
        assert!(reason.contains("too many paths"), "{reason}");
        // At once: not after the budget.
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[test]
    fn nodes_and_edges_are_counted_through_chains_and_subgraphs() {
        let source = "digraph { a -> b -> c; subgraph s { d; e -> a } f [shape=box]; a -> a }";
        let mut parser = layout::gv::DotParser::new(source);
        let graph = parser.process().expect("it parses");
        let (nodes, edges) = nodes_and_edges(&graph);
        assert_eq!(nodes, 6);
        assert_eq!(edges.len(), 4);
    }

    #[test]
    fn a_source_over_the_cap_is_refused() {
        let big = format!("digraph {{ a -> b; /* {} */ }}", "x".repeat(MAX_SOURCE));
        assert!(svg_of(Language::Dot, &big).expect_err("big").contains("too big"));
    }

    #[test]
    fn a_layout_that_outlives_its_budget_is_abandoned_and_counted() {
        // Takes far longer than a millisecond to lay out in a debug build, but is within the cost layout-rs is
        // trusted with, so it ends within a second or so.
        let source = skipping_chain(28);
        let ctl = Ctl::new(Duration::from_millis(1));
        let reason = dot_svg(&source, &ctl).expect_err("out of time");
        assert!(reason.contains("longer than"), "{reason}");
        // A cancelled job stops waiting at once.
        let ctl = Ctl::new(Duration::from_secs(60));
        ctl.cancel();
        assert_eq!(dot_svg(&source, &ctl).expect_err("cancelled"), "Cancelled");
        // The abandoned layouts finish in the end, and the count comes back down.
        for _ in 0..3000 {
            if LAYOUTS.load(Ordering::SeqCst) == 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("layout threads are still running: {}", LAYOUTS.load(Ordering::SeqCst));
    }

    // ---- Through the cache ------------------------------------------------------------------------------------------

    struct Probe;

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui_kit::test]
    fn a_diagram_is_requested_drawn_and_kept(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.update(testing::init);
        svg_image::clear();
        let (_visual, view) = testing::open_window(cx, |_, cx| cx.new(|_| Probe));
        let id = view.entity_id();
        let source: Arc<str> = Arc::from(DOT);
        let ask = |cx: &mut TestAppContext| cx.update(|cx| request(cx, Language::Dot, &source, id));
        assert!(matches!(ask(cx), Lookup::Making));
        let key = key_of(Language::Dot, &source);
        for _ in 0..1200 {
            cx.run_until_parked();
            if svg_image::peek(&key).is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let Lookup::Done(outcome) = ask(cx) else {
            panic!("not kept")
        };
        let drawn = outcome.as_ref().as_ref().expect("it was drawn");
        assert!(drawn.width > 100.);
        svg_image::clear();
    }
}
