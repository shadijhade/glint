//! Math in Markdown, LaTeX syntax (MarkdownView.tsx's `MathView`; docs/gpui/previews-native.md section 7).
//!
//! | TypeScript                                     | Here                                                         |
//! |------------------------------------------------|--------------------------------------------------------------|
//! | `temml.renderToString(tex, {displayMode, throwOnError: false, trust: false})` then MathML | RaTeX ([`make`]): parse, lay out, SVG with the glyphs as outlines (the KaTeX fonts are inside the crate) |
//! | `<MathView>` shows `<code>{tex}</code>` until Temml has loaded, and when it doesn't parse | [`fallback`]: the LaTeX in a tinted monospace span |
//! | ```` ```math ```` block, `$$…$$` on its own      | [`MathBlock`]: a display formula, centred                     |
//! | `$…$` and a `$$…$$` inside a paragraph          | [`MathInline`]: flows with the text on its baseline           |
//!
//! # How it is drawn
//!
//! [`request`] asks [`super::svg_image`] for the formula's picture (a job on a background thread: RaTeX parses,
//! lays out and writes the SVG, resvg rasterises it) and the view is woken when it is ready. Until then, and if
//! the formula doesn't parse, the LaTeX shows as a monospace span. The picture is cached by formula, size and
//! theme ([`svg_image::Key`]).
//!
//! **Baseline.** RaTeX's display list says how far the formula rises above its baseline and how far it falls below
//! (in em). The SVG is written at the surrounding font size with 1 px of padding, so the baseline is `padding +
//! height × font size` from the top: [`Drawn::baseline`] carries it, and the text view sits the picture on the
//! text's baseline with it (`InlineElement::with_baseline`).
//!
//! **Size.** RaTeX writes the SVG's `width` and `height` in points; they are rewritten as px so one SVG unit is one
//! logical pixel at the font size asked for.
//!
//! **Colour.** RaTeX draws in black; the SVG is written with `rgb()` colours and the black becomes the text
//! colour of the app, light or dark. A formula's own `\color{…}` keeps its colour.
//!
//! **What counts as a formula.** `$…$` is a formula when it is the way a person writes one, as GitHub reads it: no
//! space after the opening or before the closing `$`, and no digit straight after the closing one ("$5 and $10"
//! is two prices, not a formula).
//!
//! # Not ported / differences
//!
//! - Temml's MathML became RaTeX's KaTeX-compatible layout: the commands are KaTeX's (mhchem's `\ce`, `\pu` too),
//!   the look is KaTeX's, not the browser's MathML.
//! - **Arabic and Hebrew letters inside a formula** (`\text{مرحبا}`) are not drawn: RaTeX has no shaping or
//!   right-to-left layout and would show the letters apart and in reverse order, so such a formula shows as its
//!   LaTeX instead (which the text view draws correctly) with the reason in its description. Arabic *around* a
//!   formula is ordinary text and is right.
//! - `trust: false` (no `\href`, `\url`, `\includegraphics`): RaTeX draws a picture, so there is nothing to follow.
//! - Selecting and copying a formula gives its LaTeX.

use super::diagram::{hex, is_rtl_char};
use super::svg_image::{self, Drawn, Lookup};
use crate::kit::prelude::*;
use crate::theme;
use gpui_kit::base::text::{
    markdown_ast, InlineElement, InlineRenderContext, MarkdownNode, MarkdownParseContext, MarkdownPlugin,
};
use gpui_kit::{img, EntityId, ImageSource, ObjectFit, StyledImage as _};
use ratex_layout::{layout, to_display_list, validate_layout, LayoutOptions};
use ratex_svg::{render_to_svg_with_color_syntax, SvgColorSyntax, SvgOptions};
use std::sync::Arc;
use std::time::Duration;

/// The longest a formula may take to draw.
pub const BUDGET: Duration = Duration::from_secs(5);
/// The longest LaTeX source drawn, in characters.
pub const MAX_TEX: usize = 20_000;
/// The padding round a formula, in px at the font size it is written for.
const PADDING: f64 = 1.;
/// How much bigger a display formula is than the body text (KaTeX's own 1.21 is more than a page of text wants).
const DISPLAY_SCALE: f32 = 1.15;
/// The longest a monospace span of LaTeX gets before it stops growing.
const SPAN_MAX_WIDTH: f32 = 520.;

/// The name of the inline node, and of the block node.
pub const INLINE: &str = "glint-math-inline";
pub const BLOCK: &str = "glint-math-block";

/// A formula as the Markdown parser hands it over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tex {
    /// The LaTeX between the dollars.
    pub tex: Arc<str>,
    /// A display formula (own line, big operators), not an inline one.
    pub display: bool,
}

/// A formula made: its SVG, and where its baseline is.
#[derive(Clone, Debug)]
pub struct Made {
    /// The SVG, in user units that are px at the font size it was made for.
    pub svg: String,
    /// The distance from the top to the baseline, in the same units.
    pub baseline: f64,
    /// The size.
    pub width: f64,
    /// The size.
    pub height: f64,
}

/// Why `tex` can't be drawn without looking at it, if it can't.
pub fn refusal(tex: &str) -> Option<String> {
    if tex.trim().is_empty() {
        return Some("There is nothing in it".to_string());
    }
    if tex.chars().count() > MAX_TEX {
        return Some(format!("It is too long to draw here (over {MAX_TEX} characters)"));
    }
    if tex.chars().any(is_rtl_char) {
        return Some("Arabic and Hebrew letters in a formula can't be drawn yet".to_string());
    }
    None
}

/// The SVG of `tex` at `font_px`, in `color` (`#rrggbb`) where it is black. `display` is the display style
/// (large operators, limits above and below), else the text style of a formula inside a line.
pub fn make(tex: &str, display: bool, font_px: f64, color: &str) -> Result<Made, String> {
    if let Some(reason) = refusal(tex) {
        return Err(reason);
    }
    // The style of a formula inside a line is KaTeX's `\textstyle`; the layout's default is the display style.
    let source = if display {
        tex.to_string()
    } else {
        format!("\\textstyle {tex}")
    };
    let ast = ratex_parser::parse(&source).map_err(|error| error.to_string())?;
    let boxes = layout(&ast, &LayoutOptions::default());
    validate_layout(&boxes)?;
    let list = to_display_list(&boxes);
    if !(list.width.is_finite() && list.height.is_finite() && list.depth.is_finite()) {
        return Err("The formula has no size".to_string());
    }
    let svg = render_to_svg_with_color_syntax(
        &list,
        &SvgOptions {
            font_size: font_px,
            padding: PADDING,
            stroke_width: (font_px / 24.).max(0.5),
            embed_glyphs: true,
            font_dir: String::new(),
        },
        SvgColorSyntax::Rgb,
    );
    // RaTeX sizes the SVG in points (`width="64pt"`, a third bigger than the viewBox's px): make them px, so the
    // picture is as big as the baseline arithmetic below says.
    let header_end = svg.find('>').map_or(0, |end| end + 1);
    let svg = format!("{}{}", svg[..header_end].replace("pt\"", "\""), &svg[header_end..]);
    let svg = svg.replace("rgb(0,0,0)", &rgb_of(color));
    Ok(Made {
        svg,
        baseline: PADDING + list.height * font_px,
        width: list.width * font_px + 2. * PADDING,
        height: (list.height + list.depth) * font_px + 2. * PADDING,
    })
}

/// `#rrggbb` as `rgb(r,g,b)` (RaTeX's spelling); anything else as it is.
fn rgb_of(color: &str) -> String {
    let digits = color.strip_prefix('#').filter(|d| d.len() == 6);
    match digits.and_then(|d| u32::from_str_radix(d, 16).ok()) {
        Some(n) => format!("rgb({},{},{})", (n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff),
        None => color.to_string(),
    }
}

/// The font size a formula is cached and drawn at: to half a pixel, so a pointer's worth of difference in the
/// text size doesn't make a new picture.
pub fn quantized(font_px: f32) -> f32 {
    ((font_px.clamp(6., 96.)) * 2.).round() / 2.
}

/// The cache key of a formula.
pub fn key_of(tex: &str, display: bool, font_px: f32, dark: bool) -> svg_image::Key {
    svg_image::Key::new(
        if display { "math-display" } else { "math" },
        tex,
        quantized(font_px),
        dark,
    )
}

/// The picture of a formula: made if it hasn't been (on a background thread; `wake` is notified when it is done),
/// else returned.
pub fn request(cx: &mut App, tex: &Arc<str>, display: bool, font_px: f32, wake: EntityId) -> Lookup {
    let (dark, color) = {
        let tokens = theme::tokens(cx);
        (tokens.dark, hex(tokens.foreground))
    };
    let font_px = quantized(font_px);
    let source = tex.clone();
    svg_image::request(
        cx,
        key_of(&source, display, font_px, dark),
        wake,
        BUDGET,
        move |renderer, ctl| {
            let made = make(&source, display, f64::from(font_px), &color)?;
            if ctl.cancelled() {
                return Err("Cancelled".to_string());
            }
            let mut drawn = svg_image::rasterize(renderer, &made.svg)?;
            drawn.baseline = Some(made.baseline as f32 * drawn.scale);
            Ok(drawn)
        },
    )
}

// ---- What shows ---------------------------------------------------------------------------------------------

/// The LaTeX in a tinted monospace span: what shows while the picture is made, and what shows instead of a
/// formula that can't be drawn (`failed`: tinted as a warning; `why` is its description).
pub fn fallback(id: ElementId, tex: &str, failed: bool, why: Option<&str>, cx: &App) -> AnyElement {
    let t = theme::tokens(cx);
    let tint = if failed { t.warning.opacity(0.16) } else { t.muted };
    let mut span = div()
        .id(id)
        .role(Role::Math)
        .aria_label(SharedString::from(tex.to_string()));
    if let Some(reason) = why {
        span = span.aria_description(SharedString::from(reason.to_string()));
    }
    span.test_support()
        .px(px(3.))
        .rounded(theme::size::RADIUS_SM)
        .bg(tint)
        .font_family(theme::mono_font(cx))
        .text_color(t.foreground)
        .max_w(px(SPAN_MAX_WIDTH))
        .child(SharedString::from(tex.to_string()))
        .into_any_element()
}

/// The element id of the formula at `start` in the document.
fn id_of(prefix: &'static str, node: &MarkdownNode) -> ElementId {
    ElementId::NamedInteger(prefix.into(), node.source_range().map_or(0, |range| range.start) as u64)
}

fn drawn_picture(drawn: &Drawn) -> gpui_kit::Img {
    img(ImageSource::Render(drawn.image.clone()))
        .w(px(drawn.width))
        .h(px(drawn.height))
        .object_fit(ObjectFit::Fill)
}

// ---- The Markdown plugins -------------------------------------------------------------------------------

/// Whether the `$…$` at `source` (with `after`, what follows it in the document) is a formula as GitHub reads it.
pub fn looks_like_math(inner: &str, after: Option<char>) -> bool {
    let (Some(first), Some(last)) = (inner.chars().next(), inner.chars().next_back()) else {
        return false;
    };
    if first.is_whitespace() || last.is_whitespace() || after.is_some_and(|c| c.is_ascii_digit()) {
        return false;
    }
    // Markup between two dollar amounts ("<strong>$1.56</strong> and <strong>$1.50</strong>") is prose.
    !(inner.contains("</") || inner.contains("<br"))
}

/// What follows the node in the fragment being parsed.
fn char_after(node: &markdown_ast::Node, cx: &MarkdownParseContext<'_>) -> Option<char> {
    let end = node.position()?.end.offset;
    cx.source().get(end..)?.chars().next()
}

/// Inline formulas: `$…$`, and a `$$…$$` inside a paragraph (the display style, in the line).
pub struct MathInline {
    /// The view to wake when a formula's picture is ready.
    pub wake: EntityId,
}

impl MarkdownPlugin for MathInline {
    fn name(&self) -> &str {
        INLINE
    }

    fn parse(&self, node: &markdown_ast::Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let markdown_ast::Node::InlineMath(math) = node else {
            return None;
        };
        if !looks_like_math(&math.value, char_after(node, cx)) {
            return None;
        }
        let written = cx.node_source(node).unwrap_or_default();
        let data = Tex {
            tex: Arc::from(math.value.as_str()),
            display: written.starts_with("$$"),
        };
        Some(
            MarkdownNode::new(INLINE, data)
                .text(math.value.clone())
                .markdown(written.to_string())
                .accessibility_label(math.value.clone()),
        )
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<InlineElement> {
        let data = node.data::<Tex>()?;
        svg_image::release_retired(window);
        let id = id_of("math-inline", node);
        let font_px = f32::from(context.font_size());
        let placed = match request(cx, &data.tex, data.display, font_px, self.wake) {
            Lookup::Done(outcome) => match outcome.as_ref() {
                Ok(drawn) => {
                    let element = InlineElement::new(
                        div()
                            .id(id)
                            .role(Role::Math)
                            .aria_label(SharedString::from(data.tex.to_string()))
                            .test_support()
                            .child(drawn_picture(drawn)),
                    );
                    return Some(match drawn.baseline {
                        Some(baseline) => element.with_baseline(px(baseline)),
                        None => element,
                    });
                }
                Err(reason) => (true, Some(reason.clone())),
            },
            Lookup::Making => (false, None),
        };
        Some(InlineElement::new(fallback(
            id,
            &data.tex,
            placed.0,
            placed.1.as_deref(),
            cx,
        )))
    }
}

/// Display formulas: a `$$…$$` block, and a ```` ```math ```` fence. Centred, on their own.
pub struct MathBlock {
    /// The view to wake when a formula's picture is ready.
    pub wake: EntityId,
}

impl MarkdownPlugin for MathBlock {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        BLOCK
    }

    fn parse(&self, node: &markdown_ast::Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let tex = match node {
            markdown_ast::Node::Math(math) => math.value.clone(),
            markdown_ast::Node::Code(code)
                if code
                    .lang
                    .as_deref()
                    .is_some_and(|l| l.trim().eq_ignore_ascii_case("math")) =>
            {
                code.value.clone()
            }
            _ => return None,
        };
        let written = cx.node_source(node).unwrap_or_default().to_string();
        Some(
            MarkdownNode::new(
                BLOCK,
                Tex {
                    tex: Arc::from(tex.as_str()),
                    display: true,
                },
            )
            .text(tex.clone())
            .markdown(written)
            .accessibility_label(tex),
        )
    }

    fn render(&self, node: &MarkdownNode, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(data) = node.data::<Tex>() else {
            return div().into_any_element();
        };
        svg_image::release_retired(window);
        let id = id_of("math-block", node);
        let font_px = f32::from(window.rem_size()) * DISPLAY_SCALE;
        let shown = match request(cx, &data.tex, true, font_px, self.wake) {
            Lookup::Done(outcome) => match outcome.as_ref() {
                Ok(drawn) => div()
                    .id(id)
                    .role(Role::Math)
                    .aria_label(SharedString::from(data.tex.to_string()))
                    .test_support()
                    .overflow_x_scroll()
                    .child(drawn_picture(drawn))
                    .into_any_element(),
                Err(reason) => fallback(id, &data.tex, true, Some(reason), cx),
            },
            Lookup::Making => fallback(id, &data.tex, false, None, cx),
        };
        div()
            .w_full()
            .min_w_0()
            .flex()
            .justify_center()
            .py(space(1.))
            .child(shown)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;

    #[test]
    fn a_formula_is_made_with_its_baseline() {
        let inline = make("x^2 + y_i", false, 16., "#202020").expect("it parses");
        assert!(inline.width > 10. && inline.height > 8., "{inline:?}");
        // The baseline is inside the picture, below the top and above the bottom.
        assert!(inline.baseline > 1. && inline.baseline < inline.height, "{inline:?}");
        // The colour is the text colour, not black.
        assert!(
            inline.svg.contains("rgb(32,32,32)") && !inline.svg.contains("rgb(0,0,0)"),
            "{}",
            &inline.svg[..300.min(inline.svg.len())]
        );
        // A display formula is taller than the same one in a line (limits above and below).
        let sum = r"\sum_{i=1}^{n} i";
        let (a, b) = (
            make(sum, false, 16., "#000000").unwrap(),
            make(sum, true, 16., "#000000").unwrap(),
        );
        assert!(b.height > a.height, "{} vs {}", a.height, b.height);
    }

    #[test]
    fn a_formula_that_does_not_parse_says_why() {
        let reason = make(r"\frac{1}{", false, 16., "#000000").expect_err("it doesn't parse");
        assert!(!reason.is_empty());
        assert!(make("", true, 16., "#000").is_err());
        assert!(make("   ", true, 16., "#000").is_err());
        assert!(make(&"x".repeat(MAX_TEX + 1), true, 16., "#000").is_err());
        // Unknown commands are errors, not crashes.
        assert!(make(r"\notacommand{x}", false, 16., "#000000").is_err());
    }

    #[test]
    fn arabic_letters_inside_a_formula_are_refused_not_garbled() {
        let reason = make(r"\text{مرحبا} x^2", false, 16., "#000000").expect_err("refused");
        assert!(reason.contains("Arabic"), "{reason}");
        assert!(make(r"\text{שלום}", true, 16., "#000000").is_err());
        // Arabic-Indic digits are not letters of a script RaTeX can't set: they are in the Arabic block too.
        assert!(refusal("x = ٣").is_some());
    }

    #[test]
    fn dollars_are_a_formula_only_when_written_as_one() {
        assert!(looks_like_math("x^2", None));
        assert!(looks_like_math("x", Some(' ')));
        // GitHub's rules: no space inside the dollars, no digit after the closing one.
        assert!(!looks_like_math(" x ", None));
        assert!(!looks_like_math("5 and ", None));
        assert!(!looks_like_math("x", Some('5')));
        assert!(!looks_like_math("1.56</strong> beat the <strong>", None));
        assert!(!looks_like_math("", None));
    }

    #[test]
    fn sizes_are_cached_to_half_a_pixel() {
        assert_eq!(quantized(16.), 16.);
        assert_eq!(quantized(16.2), 16.);
        assert_eq!(quantized(16.3), 16.5);
        assert_eq!(quantized(1.), 6.);
        assert_eq!(quantized(500.), 96.);
        let (a, b) = (key_of("x", false, 16.1, false), key_of("x", false, 16.2, false));
        assert_eq!(a, b);
        assert_ne!(a, key_of("x", true, 16., false));
        assert_ne!(a, key_of("x", false, 16., true));
    }

    #[test]
    fn rgb_spelling_follows_ratex() {
        assert_eq!(rgb_of("#ff8000"), "rgb(255,128,0)");
        assert_eq!(rgb_of("red"), "red");
    }

    struct Probe;

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui_kit::test]
    fn a_requested_formula_is_made_once_and_carries_its_baseline(cx: &mut TestAppContext) {
        let _guard = crate::testing::lock();
        cx.update(crate::testing::init);
        svg_image::clear();
        let (_visual, view) = crate::testing::open_window(cx, |_, cx| cx.new(|_| Probe));
        let tex: Arc<str> = Arc::from(r"\frac{a}{b} + x^2");
        let (light, dark) = (key_of(&tex, false, 16., false), key_of(&tex, false, 16., true));
        assert_ne!(light, dark);
        assert!(matches!(
            cx.update(|cx| request(cx, &tex, false, 16.1, view.entity_id())),
            Lookup::Making
        ));
        for _ in 0..400 {
            cx.run_until_parked();
            if svg_image::peek(&light).is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        // Asked at 16.1 px, kept at 16: the same picture for 16 px.
        let Lookup::Done(outcome) = cx.update(|cx| request(cx, &tex, false, 16., view.entity_id())) else {
            panic!("not kept");
        };
        let drawn = outcome.as_ref().as_ref().expect("it was drawn");
        let baseline = drawn.baseline.expect("a formula has a baseline");
        // The baseline is inside the picture: above its bottom (the depth is the fraction's lower part).
        assert!(
            baseline > 4. && baseline < drawn.height,
            "{baseline} in {}",
            drawn.height
        );
        // A formula that doesn't draw is kept as a failure, with its reason.
        let broken: Arc<str> = Arc::from(r"\frac{1}{");
        cx.update(|cx| request(cx, &broken, false, 16., view.entity_id()));
        for _ in 0..400 {
            cx.run_until_parked();
            if svg_image::peek(&key_of(&broken, false, 16., false)).is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let failed = svg_image::peek(&key_of(&broken, false, 16., false)).expect("kept");
        assert!(failed.as_ref().is_err());
        svg_image::clear();
    }

    #[gpui_kit::test]
    fn a_made_formula_rasterises(cx: &mut TestAppContext) {
        let made = make(r"\int_0^1 e^{-t^2}\,dt", false, 16., "#101010").expect("it parses");
        let renderer = cx.update(|cx| cx.svg_renderer());
        let drawn = svg_image::rasterize(&renderer, &made.svg).expect("it draws");
        assert!(
            (drawn.width - made.width as f32).abs() < 2.,
            "{} vs {}",
            drawn.width,
            made.width
        );
        assert!(
            (drawn.height - made.height as f32).abs() < 2.,
            "{} vs {}",
            drawn.height,
            made.height
        );
    }
}
