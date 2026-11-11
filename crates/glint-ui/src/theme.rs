//! Glint's look: the colors and sizes of src/index.css as typed tokens, light and dark, with Windows' accent
//! color and Mica, mapped onto gpui-component's `Theme` so every kit control wears them.
//!
//! **Views read [`tokens`], never literals and never gpui-component's `Theme` directly.** A hard-coded color
//! or pixel size in a view is a bug: add a token here instead.
//!
//! ```ignore
//! let t = theme::tokens(cx);
//! div().bg(t.card).text_color(t.muted_foreground).border_color(t.border)
//! ```
//!
//! **Where the numbers come from.** The token names are index.css's (`--background`, `--muted-foreground`…);
//! the values are Glint's native palette: cool neutrals (hue 240, a few percent of saturation, Tailwind's zinc
//! family) so grays read as designed rather than as plain gray, in three depths (the chrome behind, the sheet the
//! files are on, and what floats over it), with the logo's pink as the accent. Sizes follow Tailwind's 4 px unit
//! at Glint's base size ([`crate::kit::space`]), and the type scale is Tailwind's: `xs` 12, `sm` 14, `base` 16 px
//! (see [`size`]). Fonts are Windows 11's UI font and Cascadia Mono, with fallbacks for a PC that lacks them.
//!
//! **Depth.** The window has three levels. *Chrome* (title bar, toolbar, sidebar) is the darkest and lets Mica
//! through. The *sheet* (`background`) holds the files and sits on it with a rounded corner and a hairline
//! ([`Tokens::stroke`]). *Floating* things (menus, the palette, dialogs, notices) are lighter still and cast a
//! shadow ([`elevation`]). Hover, pressed and selected fills are see-through ([`Tokens::accent`],
//! [`Tokens::pressed`], [`Tokens::selected`]), so they read the same on every level.
//!
//! **Motion.** [`motion`] holds the durations and curves everything moves with, and turns on what the vendored
//! GPUI adds for Glint: hover styles that fade, and a wheel that glides. Windows' own "Animation effects" switch
//! turns all of it off ([`motion::sync_with_windows`]).
//!
//! **The accent.** With the setting "Windows' accent color", `primary` and `ring` follow the color chosen
//! in Windows (appearance.rs read it with `UISettings`; [`read_windows_accent`] is that port). Windows 11
//! fills with the darker first shade in light mode and the lighter second one in dark mode, with text
//! chosen for contrast (`glint_model::color::text_on`), exactly as useTheme.ts did.
//!
//! **Mica.** [`window_background`] is the material behind the window (Mica Alt from Windows 11 22H2, Mica
//! on the first Windows 11, nothing before). While it's on, chrome (title bar, sidebar) is transparent so
//! the material shows through; files, previews, dialogs and menus keep their own fill ([`chrome_fill`]).
//!
//! **When things change:** [`apply`] recomputes everything. It runs at start, when the settings change
//! ([`crate::state::update`]), when Windows switches between light and dark ([`on_appearance_changed`]) and
//! when Windows' accent color changes (a watcher thread posts the new color to [`init`]'s task).

use glint_model::store::{Accent, Backdrop, FontSize, Theme as ThemeSetting};
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    hsla, px, rgb, App, AppContext as _, Global, Hsla, Pixels, SharedString, WindowAppearance,
    WindowBackgroundAppearance,
};

/// The type scale (Tailwind's, in rems so the text-size setting scales it, as it scaled `<html>`'s font size
/// in the web UI: 14, 16 or 18 px per rem) and the radii (fixed pixels).
pub mod size {
    use gpui_kit::{px, rems, Pixels, Rems};

    /// `text-[0.6875rem]`: the smallest text (badges). 11 px at the usual size.
    pub const XXS: Rems = rems(0.6875);
    /// `text-xs`: 12 px at the usual size.
    pub const XS: Rems = rems(0.75);
    /// `text-sm`: 14 px.
    pub const SM: Rems = rems(0.875);
    /// `text-base`: 16 px.
    pub const BASE: Rems = rems(1.);
    /// `rounded-sm`.
    pub const RADIUS_SM: Pixels = px(4.);
    /// `rounded-md`: buttons and fields.
    pub const RADIUS: Pixels = px(6.);
    /// `rounded-lg`: cards and banners.
    pub const RADIUS_LG: Pixels = px(8.);
    /// `rounded-xl`: dialogs.
    pub const RADIUS_XL: Pixels = px(12.);
    /// The sheet the files are on: its corner against the chrome.
    pub const RADIUS_SHEET: Pixels = px(14.);
    /// A row's or a tile's highlight (hover, selection): a pill inside its line.
    pub const RADIUS_ROW: Pixels = px(8.);
}

/// How things move: the durations and curves, and the switches for what the vendored GPUI adds for Glint.
///
/// Rules of thumb: something the pointer causes directly (hover, press) answers within a frame and settles in
/// about a tenth of a second; something that appears (a menu, the palette, a dialog) arrives in under a fifth of a
/// second on a curve that starts fast ([`enter`]); something that leaves goes quicker than it came. Nothing waits
/// for an animation: every one starts from what is already on screen and can be interrupted.
pub mod motion {
    use gpui_kit::App;
    use std::time::Duration;

    /// Hover fills fading in: almost at once, so the pointer never feels behind.
    pub const HOVER_IN: Duration = Duration::from_millis(70);
    /// Hover fills fading out: a short trail behind the pointer.
    pub const HOVER_OUT: Duration = Duration::from_millis(160);
    /// Small things changing state: a selection's fill, a toggle, a chevron.
    pub const FAST: Duration = Duration::from_millis(110);
    /// Things arriving: menus, the palette, panes, a folder's files.
    pub const BASE: Duration = Duration::from_millis(170);
    /// Big surfaces: dialogs, sheets sliding in.
    pub const SLOW: Duration = Duration::from_millis(240);

    /// The curve things arrive on: fast out of the gate, a long soft landing (`cubic-bezier(0.16, 1, 0.3, 1)`).
    pub fn enter(t: f32) -> f32 {
        bezier(0.16, 1.0, 0.3, 1.0, t)
    }

    /// The curve things move between two places on (`cubic-bezier(0.2, 0, 0, 1)`).
    pub fn glide(t: f32) -> f32 {
        bezier(0.2, 0.0, 0.0, 1.0, t)
    }

    /// A CSS cubic-bezier timing curve at `t` (0 to 1): solved for the curve's parameter with a few Newton steps.
    fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
        let t = t.clamp(0., 1.);
        let at = |a: f32, b: f32, u: f32| {
            let inv = 1. - u;
            3. * inv * inv * u * a + 3. * inv * u * u * b + u * u * u
        };
        let slope = |a: f32, b: f32, u: f32| {
            let inv = 1. - u;
            3. * inv * inv * a + 6. * inv * u * (b - a) + 3. * u * u * (1. - b)
        };
        let mut u = t;
        for _ in 0..6 {
            let dx = at(x1, x2, u) - t;
            let d = slope(x1, x2, u);
            if dx.abs() < 1e-4 || d.abs() < 1e-5 {
                break;
            }
            u = (u - dx / d).clamp(0., 1.);
        }
        at(y1, y2, u)
    }

    /// Whether Windows' "Animation effects" setting (Settings → Accessibility → Visual effects) is on. On when
    /// Windows can't say.
    pub fn windows_animations_on() -> bool {
        use windows::Win32::UI::WindowsAndMessaging::{
            SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
        };
        let mut on = windows::core::BOOL(1);
        // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL through the pointer, which points at `on`.
        let read = unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                Some(std::ptr::from_mut(&mut on).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        };
        read.is_err() || on.as_bool()
    }

    /// Follows Windows' "Animation effects" switch: with it off, nothing in Glint animates (GPUI's reduced
    /// motion: fades, glides and transitions all land at once). Run at start and whenever Windows' appearance
    /// changes. Tests keep motion off themselves, so they are left alone.
    pub fn sync_with_windows(cx: &mut App) {
        if cfg!(test) {
            return;
        }
        cx.set_reduce_motion(!windows_animations_on());
    }

    /// Turns on what the vendored GPUI adds for Glint: hover styles that fade in and out. (The wheel's glide
    /// needs no switch: it follows reduced motion by itself.)
    pub fn init(cx: &mut App) {
        gpui_kit::set_hover_fade(Some((HOVER_IN, HOVER_OUT)));
        sync_with_windows(cx);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_curves_start_at_nothing_end_at_everything_and_front_load() {
            for curve in [enter as fn(f32) -> f32, glide] {
                assert!(curve(0.).abs() < 1e-3 && (curve(1.) - 1.).abs() < 1e-3);
                // Never backwards.
                let mut last = 0.;
                for step in 1..=20 {
                    let now = curve(step as f32 / 20.);
                    assert!(now + 1e-3 >= last, "{now} after {last}");
                    last = now;
                }
            }
            // Arriving: most of the way there in the first third of the time.
            assert!(enter(0.33) > 0.8, "{}", enter(0.33));
            // CSS's values for the same curves.
            assert!((enter(0.5) - 0.9718).abs() < 0.005, "{}", enter(0.5));
            assert!((glide(0.5) - 0.8778).abs() < 0.005, "{}", glide(0.5));
        }

        /// The wheel's glide (vendored GPUI, `wheel_glide.rs`), stepped as a 60 Hz screen steps it: what is left of
        /// `distance` after each frame.
        fn wheel_steps(distance: f32, scale: f32) -> Vec<f32> {
            use gpui_kit::{px, wheel_glide_advance};
            let frame = Duration::from_micros(16_667);
            let (mut left, mut elapsed, mut lefts) = (px(distance), None, Vec::new());
            while left != px(0.) {
                let by = wheel_glide_advance(left, elapsed, scale);
                // Every step moves toward the target and never past it; the last one takes what is left, however
                // little (what steps on the pixel grid leave behind in floating point).
                let last = f32::from(left - by) == 0.;
                assert!(
                    (last || f32::from(by) * distance > 0.) && f32::from(by).abs() <= f32::from(left).abs() + 1e-3,
                    "{by:?} of {left:?}"
                );
                left -= by;
                lefts.push(f32::from(left));
                elapsed = Some(frame);
                assert!(lefts.len() < 100, "the glide never ends: {lefts:?}");
            }
            lefts
        }

        #[test]
        fn a_wheel_notch_glides_there_on_whole_pixels_within_a_quarter_of_a_second() {
            for distance in [100., -100.] {
                let lefts = wheel_steps(distance, 1.);
                // On whole pixels all the way, so hairlines and text stay sharp while they move.
                assert!(lefts.iter().all(|left| left.fract() == 0.), "{lefts:?}");
                // Answered in the event's own frame: a fifth of the notch or more at once.
                assert!(lefts[0].abs() <= 80., "{lefts:?}");
                // Most of it within a tenth of a second, all of it within a quarter.
                assert!(lefts[5].abs() <= 20., "{lefts:?}");
                assert!(lefts.len() <= 15, "{} frames: {lefts:?}", lefts.len());
            }
            // At 125% a device pixel is 0.8 px: steps land on multiples of it, and it still ends.
            let lefts = wheel_steps(100., 1.25);
            assert!(
                lefts
                    .iter()
                    .all(|left| ((left * 1.25).round() - left * 1.25).abs() < 1e-3),
                "{lefts:?}"
            );
            // Three notches at once (a fast spin) take no longer to start, and end.
            assert!(wheel_steps(300., 1.)[0] <= 240.);
        }

        #[test]
        fn leaving_is_quicker_than_arriving() {
            assert!(HOVER_IN < HOVER_OUT && FAST < BASE && BASE < SLOW);
        }
    }
}

/// Shadows for what sits above the sheet. GPUI draws them outside the element, so a parent that clips cuts them.
pub mod elevation {
    use super::tokens;
    use gpui_kit::{hsla, point, px, App, BoxShadow};

    fn shadow(y: f32, blur: f32, spread: f32, alpha: f32) -> BoxShadow {
        BoxShadow {
            color: hsla(240. / 360., 0.3, 0.02, alpha),
            offset: point(px(0.), px(y)),
            blur_radius: px(blur),
            spread_radius: px(spread),
            inset: false,
        }
    }

    /// Something resting on the sheet: a card, a thumbnail, a chip being dragged.
    pub fn raised(cx: &App) -> Vec<BoxShadow> {
        let dark = tokens(cx).dark;
        vec![
            shadow(1., 2., 0., if dark { 0.35 } else { 0.08 }),
            shadow(2., 6., -1., if dark { 0.25 } else { 0.06 }),
        ]
    }

    /// Something floating over the window: a menu, the palette, a notice.
    pub fn floating(cx: &App) -> Vec<BoxShadow> {
        let dark = tokens(cx).dark;
        vec![
            shadow(2., 6., 0., if dark { 0.30 } else { 0.06 }),
            shadow(12., 32., -4., if dark { 0.55 } else { 0.16 }),
        ]
    }

    /// A dialog.
    pub fn modal(cx: &App) -> Vec<BoxShadow> {
        let dark = tokens(cx).dark;
        vec![
            shadow(4., 12., 0., if dark { 0.35 } else { 0.08 }),
            shadow(28., 72., -8., if dark { 0.65 } else { 0.22 }),
        ]
    }
}

/// Windows 11's UI font; Segoe UI before it.
const SANS: &str = "Segoe UI Variable Text";
const SANS_FALLBACK: &str = "Segoe UI";
const MONO: &str = "Cascadia Mono";
const MONO_FALLBACK: &str = "Consolas";

/// `hsl(h s% l%)` as index.css writes it: hue in degrees, saturation and lightness in percent.
pub fn hsl(h: f32, s: f32, l: f32) -> Hsla {
    hsla(h / 360., s / 100., l / 100., 1.)
}

/// `#rrggbb` given as `0xrrggbb`.
fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

/// An sRGB color (0–255 channels) as a GPUI color.
fn from_rgb([r, g, b]: [u8; 3]) -> Hsla {
    gpui_kit::Rgba {
        r: f32::from(r) / 255.,
        g: f32::from(g) / 255.,
        b: f32::from(b) / 255.,
        a: 1.,
    }
    .into()
}

/// Glint's colors, named as the CSS variables are (`--muted-foreground` is `muted_foreground`).
#[derive(Clone, Debug)]
pub struct Tokens {
    /// Whether these are the dark set.
    pub dark: bool,
    pub background: Hsla,
    pub foreground: Hsla,
    /// Windows 11's gray chrome (title bar, toolbars) over the white content.
    pub chrome: Hsla,
    /// The sheet the files are on: `background`, see-through enough for the window's light to tint it (glass).
    /// What must hide what is under it (a header rows scroll beneath, a menu) uses `background` or `popover`.
    pub sheet: Hsla,
    /// A pane of glass lying on the chrome, with the window's light behind it: the address field, the tab
    /// and the place you are in.
    pub glass: Hsla,
    pub card: Hsla,
    pub popover: Hsla,
    pub popover_foreground: Hsla,
    /// The accent: Glint's pink, or Windows' accent color.
    pub primary: Hsla,
    pub primary_foreground: Hsla,
    pub muted: Hsla,
    pub muted_foreground: Hsla,
    /// Hover and highlight fill. See-through in both modes (white at 7% in the dark, near-black at 5.5% in the
    /// light), as Windows 11's are, so it shows on every surface.
    pub accent: Hsla,
    /// The fill of something held down: the hover fill, stronger.
    pub pressed: Hsla,
    /// The fill of what is selected (rows, tiles, places), in the pane the keyboard is in.
    pub selected: Hsla,
    /// The same with the pointer over it.
    pub selected_hover: Hsla,
    /// The same in a pane the keyboard isn't in.
    pub selected_inactive: Hsla,
    /// A hairline round raised and floating things (white at 8% in the dark, black at 8% in the light): it
    /// reads on any surface, where `border` is a fixed gray.
    pub stroke: Hsla,
    /// The glint: the logo's sparkle as a two-color sweep (pink to fuchsia, or Windows' accent and its darker
    /// shade), for the few things that should catch the eye: what is loading, what is current.
    pub glint_from: Hsla,
    pub glint_to: Hsla,
    pub destructive: Hsla,
    pub destructive_foreground: Hsla,
    pub border: Hsla,
    pub input: Hsla,
    pub ring: Hsla,
    /// Text fields: their fill, the fill while typing, and the darker bottom edge Fluent gives them.
    pub field: Hsla,
    pub field_focus: Hsla,
    pub field_edge: Hsla,
    /// Good news (emerald-600 / emerald-400 in dark): "Healthy".
    pub success: Hsla,
    /// Attention (amber-600 / amber-400 in dark): "Needs a look".
    pub warning: Hsla,
    /// Preview syntax colors (GitHub's light and dark).
    pub tk_comment: Hsla,
    pub tk_keyword: Hsla,
    pub tk_string: Hsla,
    pub tk_number: Hsla,
    pub tk_function: Hsla,
    pub tk_tag: Hsla,
    pub tk_attr: Hsla,
}

impl Tokens {
    /// The light set (`:root` in index.css).
    pub fn light() -> Self {
        let primary = hsl(333., 71., 51.);
        Self {
            dark: false,
            background: hsl(0., 0., 100.),
            foreground: hsl(240., 10., 10.),
            chrome: hsl(240., 6., 95.6),
            sheet: hsla(0., 0., 1., 0.74),
            glass: hsla(0., 0., 1., 0.62),
            card: hsl(0., 0., 100.),
            popover: hsl(0., 0., 100.),
            popover_foreground: hsl(240., 10., 10.),
            // Pink-600 here, pink-500 in the dark: at least 4.5:1 as text and under white text.
            primary,
            primary_foreground: hsl(0., 0., 100.),
            muted: hsl(240., 6., 95.),
            muted_foreground: hsl(240., 4., 40.),
            accent: hsla(240. / 360., 0.1, 0.1, 0.055),
            pressed: hsla(240. / 360., 0.1, 0.1, 0.10),
            selected: primary.opacity(0.11),
            selected_hover: primary.opacity(0.16),
            selected_inactive: hsla(240. / 360., 0.1, 0.1, 0.07),
            stroke: hsla(240. / 360., 0.1, 0.1, 0.09),
            glint_from: hex(0xf472b6),
            glint_to: hex(0xc026d3),
            destructive: hsl(0., 72., 51.),
            destructive_foreground: hsl(0., 0., 100.),
            border: hsl(240., 6., 90.),
            input: hsl(240., 5., 82.),
            ring: primary,
            field: hsl(0., 0., 100.),
            field_focus: hsl(0., 0., 100.),
            field_edge: hsl(240., 4., 58.),
            success: hex(0x059669),
            warning: hex(0xd97706),
            tk_comment: hex(0x6a737d),
            tk_keyword: hex(0xd73a49),
            tk_string: hex(0x032f62),
            tk_number: hex(0x005cc5),
            tk_function: hex(0x6f42c1),
            tk_tag: hex(0x22863a),
            tk_attr: hex(0xe36209),
        }
    }

    /// The dark set (`.dark` in index.css).
    pub fn dark() -> Self {
        let primary = hsl(330., 81., 60.);
        Self {
            dark: true,
            // Dark text on the accent, as Windows 11 has in the dark.
            primary,
            primary_foreground: hsl(240., 10., 6.),
            ring: primary,
            // Three depths: the chrome behind (7.5%), the sheet the files are on (11%), what floats (15%).
            background: hsl(240., 6., 11.),
            foreground: hsl(240., 9., 95.5),
            chrome: hsl(240., 7., 7.5),
            sheet: hsla(240. / 360., 0.09, 0.105, 0.70),
            glass: hsla(240. / 360., 0.2, 1., 0.085),
            card: hsl(240., 6., 13.5),
            popover: hsl(240., 6., 15.),
            popover_foreground: hsl(240., 9., 95.5),
            muted: hsl(240., 5., 16.5),
            muted_foreground: hsl(240., 5., 64.5),
            accent: hsla(240. / 360., 0.2, 1., 0.07),
            pressed: hsla(240. / 360., 0.2, 1., 0.11),
            selected: primary.opacity(0.17),
            selected_hover: primary.opacity(0.23),
            selected_inactive: hsla(240. / 360., 0.2, 1., 0.09),
            stroke: hsla(240. / 360., 0.2, 1., 0.08),
            glint_from: hex(0xff8fd0),
            glint_to: hex(0xd946ef),
            destructive: hsl(0., 72., 60.),
            destructive_foreground: hsl(240., 10., 6.),
            border: hsl(240., 5., 18.),
            input: hsl(240., 5., 26.),
            field: hsl(240., 6., 14.5),
            field_focus: hsl(240., 7., 8.5),
            field_edge: hsl(240., 4., 44.),
            success: hex(0x34d399),
            warning: hex(0xfbbf24),
            tk_comment: hex(0x8b949e),
            tk_keyword: hex(0xff7b72),
            tk_string: hex(0xa5d6ff),
            tk_number: hex(0x79c0ff),
            tk_function: hex(0xd2a8ff),
            tk_tag: hex(0x7ee787),
            tk_attr: hex(0xffa657),
        }
    }

    /// The set for a mode, with Windows' accent color in place of Glint's pink when one is given.
    pub fn new(dark: bool, accent: Option<&WindowsAccent>) -> Self {
        let mut tokens = if dark { Self::dark() } else { Self::light() };
        if let Some(accent) = accent {
            // Windows 11 fills with its darker first shade in light mode and its lighter second one in dark.
            let fill = if dark { accent.light[1] } else { accent.dark[0] };
            tokens.primary = from_rgb(fill);
            tokens.ring = tokens.primary;
            tokens.primary_foreground = text_on(fill);
            // What is tinted with the accent follows it: the selection, and the glint (the accent to a shade
            // of itself, lighter in the dark and darker in the light).
            let (light_tint, dark_tint) = (tokens.selected.a, tokens.selected_hover.a);
            tokens.selected = tokens.primary.opacity(light_tint);
            tokens.selected_hover = tokens.primary.opacity(dark_tint);
            tokens.glint_from = from_rgb(if dark { accent.light[2] } else { accent.light[0] });
            tokens.glint_to = from_rgb(if dark { accent.accent } else { accent.dark[1] });
        }
        tokens
    }
}

/// White or near-black text on a fill, whichever reads better: glint-model's WCAG choice, as index.css's
/// `--windows-accent-*-text` were.
fn text_on([r, g, b]: [u8; 3]) -> Hsla {
    let white = glint_model::color::text_on(&format!("#{r:02x}{g:02x}{b:02x}")).starts_with("0 0% 100%");
    if white {
        hsl(0., 0., 100.)
    } else {
        hsl(0., 0., 7.)
    }
}

/// Windows' accent color and its shades, as sRGB: the base, then lighter (1–3) and darker (1–3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowsAccent {
    pub accent: [u8; 3],
    pub light: [[u8; 3]; 3],
    pub dark: [[u8; 3]; 3],
}

/// Reads Windows' accent color (Settings → Personalization → Colors). Blocking and COM-initialising: call it
/// from a background thread, as [`init`] does. Ported from appearance.rs's `imp::accent`.
pub fn read_windows_accent() -> Result<WindowsAccent, String> {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    use windows::UI::ViewManagement::{UIColorType, UISettings};
    // WinRT on a thread of the pool: in the process's multithreaded apartment. It may already be initialised
    // in another mode on this thread; either way WinRT works, so the result is ignored.
    // SAFETY: plain COM initialisation of the calling thread, balanced by the thread ending.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let settings = UISettings::new().map_err(|e| e.to_string())?;
    let color = |kind: UIColorType| {
        settings
            .GetColorValue(kind)
            .map(|c| [c.R, c.G, c.B])
            .map_err(|e| e.to_string())
    };
    Ok(WindowsAccent {
        accent: color(UIColorType::Accent)?,
        light: [
            color(UIColorType::AccentLight1)?,
            color(UIColorType::AccentLight2)?,
            color(UIColorType::AccentLight3)?,
        ],
        dark: [
            color(UIColorType::AccentDark1)?,
            color(UIColorType::AccentDark2)?,
            color(UIColorType::AccentDark3)?,
        ],
    })
}

/// Tells `changed` whenever the user picks another accent color in Windows. Runs a thread that registers
/// for `UISettings.ColorValuesChanged` and keeps the registration alive for the life of the process.
fn watch_windows_accent(changed: async_channel::Sender<WindowsAccent>) {
    std::thread::spawn(move || {
        use windows::Foundation::TypedEventHandler;
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
        use windows::UI::ViewManagement::UISettings;
        // SAFETY: plain COM initialisation of this thread.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let Ok(settings) = UISettings::new() else { return };
        let _ = settings.ColorValuesChanged(&TypedEventHandler::<UISettings, windows::core::IInspectable>::new(
            move |_, _| {
                if let Ok(accent) = read_windows_accent() {
                    let _ = changed.try_send(accent);
                }
                Ok(())
            },
        ));
        // Keep `settings` alive for the lifetime of the process so WinRT delivers change events.
        std::mem::forget(settings);
    });
}

/// What the theme is now: the tokens in use, and the facts they were made from.
pub struct GlintTheme {
    tokens: Tokens,
    backdrop: Option<WindowBackgroundAppearance>,
    windows_accent: Option<WindowsAccent>,
    /// Pixels in a rem: the text-size setting (14, 16 or 18).
    rem: Pixels,
}

impl Global for GlintTheme {}

/// Glint's tokens now. Cheap: a global lookup.
pub fn tokens(cx: &App) -> &Tokens {
    &cx.global::<GlintTheme>().tokens
}

/// Pixels in a rem now (the text-size setting). Windows set it with `window.set_rem_size` so rem-based sizes
/// (the type scale, [`crate::kit::space`]) scale with the setting.
pub fn rem_size(cx: &App) -> Pixels {
    cx.try_global::<GlintTheme>().map_or(px(16.), |theme| theme.rem)
}

/// The material behind the window now: Mica Alt or Mica while the backdrop setting is on and Windows 11 can
/// draw it, else opaque.
pub fn window_background(cx: &App) -> WindowBackgroundAppearance {
    cx.try_global::<GlintTheme>()
        .and_then(|theme| theme.backdrop)
        .unwrap_or(WindowBackgroundAppearance::Opaque)
}

/// Whether the window's material shows through its chrome.
pub fn backdrop_on(cx: &App) -> bool {
    window_background(cx) != WindowBackgroundAppearance::Opaque
}

/// The fill for chrome (title bar, toolbar, sidebar): see-through while Mica is on so it shows, else the
/// chrome gray. The files, previews, dialogs and menus use `background`, `card` and `popover` instead.
pub fn chrome_fill(cx: &App) -> Hsla {
    if backdrop_on(cx) {
        hsla(0., 0., 0., 0.)
    } else {
        tokens(cx).chrome
    }
}

/// What the window itself is filled with, under everything: the chrome color, or over Mica a veil that takes the
/// material a step back in the dark (Mica's dark gray is as light as the sheet; veiled, the sheet stands off it)
/// and nothing in the light (Mica's light gray already sits under the white sheet).
pub fn window_fill(cx: &App) -> Hsla {
    let t = tokens(cx);
    match (backdrop_on(cx), t.dark) {
        (false, _) => t.chrome,
        (true, true) => hsla(240. / 360., 0.2, 0.03, 0.42),
        (true, false) => hsla(0., 0., 0., 0.),
    }
}

/// The glint as a fill: [`Tokens::glint_from`] to [`Tokens::glint_to`], sweeping along the reading direction.
pub fn glint_fill(cx: &App) -> gpui_kit::Background {
    let t = tokens(cx);
    let (from, to) = if crate::i18n::is_rtl() {
        (t.glint_to, t.glint_from)
    } else {
        (t.glint_from, t.glint_to)
    };
    gpui_kit::linear_gradient(
        90.,
        gpui_kit::linear_color_stop(from, 0.),
        gpui_kit::linear_color_stop(to, 1.),
    )
}

/// The fill of what is selected where the keyboard is: the accent, fading along the reading direction (the
/// selection's own light), stronger under the pointer.
pub fn selected_fill(cx: &App, hovered: bool) -> gpui_kit::Background {
    let t = tokens(cx);
    let strong = if hovered { t.selected_hover.a } else { t.selected.a } * 1.35;
    let (from, to) = (t.primary.opacity(strong), t.glint_to.opacity(strong * 0.38));
    let (from, to) = if crate::i18n::is_rtl() { (to, from) } else { (from, to) };
    gpui_kit::linear_gradient(
        90.,
        gpui_kit::linear_color_stop(from, 0.),
        gpui_kit::linear_color_stop(to, 1.),
    )
}

/// The material Windows can draw: Mica Alt (as File Explorer has) from Windows 11 22H2, Mica on the first
/// Windows 11, nothing before. Ported from appearance.rs's `backdrop`.
fn supported_material() -> Option<WindowBackgroundAppearance> {
    use windows::Wdk::System::SystemServices::RtlGetVersion;
    use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is a valid OSVERSIONINFOW with its size field set, as RtlGetVersion requires.
    if unsafe { RtlGetVersion(&mut info) }.is_err() {
        return None;
    }
    match info.dwBuildNumber {
        build if build >= 22523 => Some(WindowBackgroundAppearance::MicaAltBackdrop),
        build if build >= 22000 => Some(WindowBackgroundAppearance::MicaBackdrop),
        _ => None,
    }
}

fn pixels_of(size: FontSize) -> Pixels {
    // The same 14/16/18 px useTheme.ts gave `<html>`.
    px(match size {
        FontSize::Small => 14.,
        FontSize::Medium => 16.,
        FontSize::Large => 18.,
    })
}

/// A font family that exists on this PC: the wanted one, else the fallback.
fn installed_family(cx: &App, wanted: &'static str, fallback: &'static str) -> SharedString {
    let names = cx.text_system().all_font_names();
    if names.iter().any(|name| name == wanted) {
        wanted.into()
    } else {
        fallback.into()
    }
}

/// Start-up step 2: builds the theme, then reads Windows' accent color (off the UI thread) and watches for
/// changes to it and to light/dark. Needs `gpui_kit::init` and `state::init` to have run.
pub fn init(cx: &mut App) {
    motion::init(cx);
    apply(cx);
    // Windows' accent: read now, and again whenever Windows says it changed.
    let (changed, changes) = async_channel::unbounded::<WindowsAccent>();
    watch_windows_accent(changed);
    cx.spawn(async move |cx| {
        let first = cx.background_spawn(async { read_windows_accent().ok() }).await;
        if let Some(accent) = first {
            cx.update(|cx| set_windows_accent(cx, accent));
        }
        while let Ok(accent) = changes.recv().await {
            cx.update(|cx| set_windows_accent(cx, accent));
        }
    })
    .detach();
}

fn set_windows_accent(cx: &mut App, accent: WindowsAccent) {
    if cx.has_global::<GlintTheme>() {
        let theme = cx.global_mut::<GlintTheme>();
        if theme.windows_accent.as_ref() == Some(&accent) {
            return;
        }
        theme.windows_accent = Some(accent);
    }
    apply(cx);
}

/// Windows changed between light and dark: follows it when the theme setting is "System".
pub fn on_appearance_changed(cx: &mut App) {
    // Windows tells of its "Animation effects" switch the same way.
    motion::sync_with_windows(cx);
    if crate::state::settings(cx).theme == ThemeSetting::System {
        apply(cx);
    }
}

/// Recomputes the tokens from the settings and Windows' state, and maps them onto gpui-component's theme.
/// Call after anything the theme depends on changed ([`crate::state::update`] does).
pub fn apply(cx: &mut App) {
    let (choice, accent_choice, backdrop_choice, font_size) = {
        let s = crate::state::settings(cx);
        (s.theme, s.accent, s.backdrop, s.font_size)
    };
    let dark = match choice {
        ThemeSetting::Dark => true,
        ThemeSetting::Light => false,
        ThemeSetting::System => matches!(
            cx.window_appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
    };
    let windows_accent = cx
        .try_global::<GlintTheme>()
        .and_then(|theme| theme.windows_accent.clone());
    let accent = (accent_choice == Accent::Windows)
        .then_some(windows_accent.as_ref())
        .flatten();
    let tokens = Tokens::new(dark, accent);
    let backdrop = if backdrop_choice == Backdrop::Mica {
        supported_material()
    } else {
        None
    };
    let sans = installed_family(cx, SANS, SANS_FALLBACK);
    let mono = installed_family(cx, MONO, MONO_FALLBACK);
    let font_size = pixels_of(font_size);
    let chrome = if backdrop.is_some() {
        hsla(0., 0., 0., 0.)
    } else {
        tokens.chrome
    };
    cx.set_global(GlintTheme {
        tokens: tokens.clone(),
        backdrop,
        windows_accent,
        rem: font_size,
    });

    // gpui-component loads its registered theme for the mode, and only then are colors edited (its docs:
    // "edit colors in a second update after switching mode").
    Theme::change(if dark { ThemeMode::Dark } else { ThemeMode::Light }, None, cx);
    // Notices at the bottom end, as the web app's Toaster had them (`position="bottom-right"`), clear of the toolbar.
    let notices_at = if crate::i18n::is_rtl() {
        gpui_kit::Anchor::BottomLeft
    } else {
        gpui_kit::Anchor::BottomRight
    };
    Theme::update(cx, |theme| {
        theme.notification.placement = notices_at;
        // Clear of the status bar at the foot of the sheet, which says how a copy or a search went.
        theme.notification.margins.bottom = px(72.);
        theme.font_family = sans;
        theme.mono_font_family = mono;
        theme.font_size = font_size;
        theme.mono_font_size = font_size * 0.875;
        theme.radius = size::RADIUS;
        theme.radius_lg = size::RADIUS_XL;
        theme.shadow = true;
        // The kit's own controls move on Glint's clock (dialogs, switches, tabs, progress).
        theme.motion.duration_fast = motion::FAST;
        theme.motion.duration_normal = motion::BASE;
        theme.motion.duration_slow = motion::SLOW;
        map_colors(theme, &tokens, chrome);
    });
}

/// Glint's tokens onto the colors gpui-component's controls read. Hover and pressed states are the
/// translucent `accent` fill laid over the surface, as the CSS did with `hover:bg-accent`.
fn map_colors(theme: &mut Theme, t: &Tokens, chrome: Hsla) {
    let hover = t.accent;
    let pressed = t.pressed;
    let c = &mut theme.colors;
    c.background = t.background;
    c.foreground = t.foreground;
    c.border = t.border;
    c.input = t.input;
    c.muted = t.muted;
    c.muted_foreground = t.muted_foreground;
    c.accent = hover;
    c.accent_foreground = t.foreground;
    c.popover = t.popover;
    c.popover_foreground = t.popover_foreground;
    c.primary = t.primary;
    c.primary_foreground = t.primary_foreground;
    c.primary_hover = t.primary.opacity(0.9);
    c.primary_active = t.primary.opacity(0.8);
    c.secondary = t.muted;
    c.secondary_foreground = t.foreground;
    c.secondary_hover = hover;
    c.secondary_active = pressed;
    c.danger = t.destructive;
    c.danger_foreground = t.destructive_foreground;
    c.danger_hover = t.destructive.opacity(0.9);
    c.danger_active = t.destructive.opacity(0.8);
    c.success = t.success;
    c.warning = t.warning;
    c.ring = t.ring;
    c.caret = t.foreground;
    c.selection = t.primary.opacity(0.3);
    c.link = t.primary;
    c.link_hover = t.primary.opacity(0.85);
    c.link_active = t.primary.opacity(0.7);
    // What a dialog dims the window with: enough to set the dialog off, not so much the files vanish.
    c.overlay = hsla(240. / 360., 0.2, 0.03, if t.dark { 0.55 } else { 0.28 });
    c.list = t.background;
    c.list_even = t.background;
    c.list_head = t.background;
    c.list_hover = hover;
    c.list_active = t.selected;
    c.list_active_border = t.primary;
    c.table = t.background;
    c.table_even = t.background;
    c.table_head = t.background;
    c.table_head_foreground = t.muted_foreground;
    c.table_hover = hover;
    c.table_active = t.selected;
    c.table_active_border = t.primary;
    c.table_row_border = t.border;
    c.title_bar = chrome;
    c.title_bar_border = t.border;
    c.status_bar = chrome;
    c.status_bar_border = t.border;
    c.sidebar = chrome;
    c.sidebar_foreground = t.foreground;
    c.sidebar_border = t.border;
    c.sidebar_accent = hover;
    c.sidebar_accent_foreground = t.foreground;
    c.sidebar_primary = t.primary;
    c.sidebar_primary_foreground = t.primary_foreground;
    c.tab_bar = chrome;
    c.tab = hsla(0., 0., 0., 0.);
    c.tab_foreground = t.muted_foreground;
    c.tab_active = t.background;
    c.tab_active_foreground = t.foreground;
    c.scrollbar = hsla(0., 0., 0., 0.);
    c.scrollbar_thumb = t.muted_foreground.opacity(0.32);
    c.scrollbar_thumb_hover = t.muted_foreground.opacity(0.6);
    c.drag_border = t.primary;
    c.drop_target = t.primary.opacity(0.2);
}

// ---- Kinds of files: their icons' colors -------------------------------------------------------------------

/// A kind of file's icon colors: the stroke, and the fill some get (the folder's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KindTone {
    pub color: Hsla,
    pub fill: Option<Hsla>,
}

/// Tailwind's colors by name ("sky-500"), the ones the web UI used outside its tokens (the kinds' tones, cloud
/// marks): sRGB, v3's values, which v4's OKLCH ones match closely on an sRGB screen. None for a name not here.
pub fn tailwind(name: &str) -> Option<Hsla> {
    let hex: u32 = match name {
        "sky-400" => 0x38bdf8,
        "sky-500" => 0x0ea5e9,
        "sky-600" => 0x0284c7,
        "violet-500" => 0x8b5cf6,
        "pink-500" => 0xec4899,
        "red-500" => 0xef4444,
        "rose-500" => 0xf43f5e,
        "orange-500" => 0xf97316,
        "indigo-500" => 0x6366f1,
        "emerald-400" => 0x34d399,
        "emerald-500" => 0x10b981,
        "emerald-600" => 0x059669,
        "amber-400" => 0xfbbf24,
        "amber-500" => 0xf59e0b,
        "amber-600" => 0xd97706,
        "amber-700" => 0xb45309,
        "blue-400" => 0x60a5fa,
        "blue-600" => 0x2563eb,
        "green-500" => 0x22c55e,
        "green-600" => 0x16a34a,
        "teal-400" => 0x2dd4bf,
        "teal-600" => 0x0d9488,
        "slate-400" => 0x94a3b8,
        "slate-500" => 0x64748b,
        "yellow-500" => 0xeab308,
        "yellow-600" => 0xca8a04,
        "yellow-700" => 0xa16207,
        "cyan-400" => 0x22d3ee,
        "cyan-600" => 0x0891b2,
        _ => return None,
    };
    Some(gpui_kit::rgb(hex).into())
}

/// One class's color: `muted-foreground` or a Tailwind color, with an optional `/opacity` in percent.
fn tone_color(spec: &str, t: &Tokens) -> Option<Hsla> {
    let (name, opacity) = match spec.split_once('/') {
        Some((name, percent)) => (name, percent.parse::<f32>().ok()? / 100.),
        None => (spec, 1.),
    };
    let color = if name == "muted-foreground" {
        t.muted_foreground
    } else {
        tailwind(name)?
    };
    Some(color.opacity(opacity))
}

/// A kind of file's icon colors in the theme in use, from the Tailwind classes the TypeScript gave it:
/// `text-…` is the stroke, `fill-…` the fill, and a `dark:` class wins in the dark theme.
pub fn kind_tone(kind: &glint_model::file_types::Kind, cx: &App) -> KindTone {
    let t = tokens(cx);
    let mut tone = KindTone {
        color: t.muted_foreground,
        fill: None,
    };
    // The plain classes first, then the dark theme's over them.
    for pass_dark in [false, true] {
        if pass_dark && !t.dark {
            break;
        }
        for class in kind.tone.split_whitespace() {
            let (is_dark, class) = match class.strip_prefix("dark:") {
                Some(rest) => (true, rest),
                None => (false, class),
            };
            if is_dark != pass_dark {
                continue;
            }
            if let Some(spec) = class.strip_prefix("text-") {
                if let Some(color) = tone_color(spec, t) {
                    tone.color = color;
                }
            } else if let Some(spec) = class.strip_prefix("fill-") {
                tone.fill = tone_color(spec, t);
            }
        }
    }
    tone
}

// ---- Places: their own colors ------------------------------------------------------------------------------

/// A place Glint knows by more than its path: each has a color of its own, the same wherever it is drawn (its
/// chip in the rail, its card on This PC, its folder in a list) and in the light behind the window while you
/// are in it, so where you are can be told from the corner of the eye.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Recent,
    Gallery,
    Known(crate::explorer::KnownId),
    Drive,
    Wsl,
    RecycleBin,
    Remote,
}

/// A place's color. Home is the accent (it is the place the others are in); the rest are fixed.
pub fn place_tone(place: Place, cx: &App) -> Hsla {
    use crate::explorer::KnownId as K;
    let hex: u32 = match place {
        Place::Known(K::Home) => return tokens(cx).primary,
        Place::Recent => 0x8b5cf6,
        Place::Gallery => 0xd946ef,
        Place::Known(K::Desktop) => 0x0ea5e9,
        Place::Known(K::Documents) => 0x3b82f6,
        Place::Known(K::Downloads) => 0x10b981,
        Place::Known(K::Pictures) => 0xf43f5e,
        Place::Known(K::Music) => 0xf97316,
        Place::Known(K::Videos) => 0x6366f1,
        Place::Drive | Place::RecycleBin => 0x64748b,
        Place::Wsl => 0xea580c,
        Place::Remote => 0x14b8a6,
    };
    rgb(hex).into()
}

/// The folder's color (amber), as rows, tiles and the tree draw it.
pub fn folder_tone(cx: &App) -> Hsla {
    kind_tone(glint_model::file_types::folder_kind(), cx).color
}

/// The monospace family for paths, code and hex (index.css's `--font-mono`: Cascadia Mono), as the kit's theme
/// holds it.
pub fn mono_font(cx: &App) -> SharedString {
    gpui_kit::component::Theme::global(cx).mono_font_family.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Hsla, b: Hsla) -> bool {
        (a.h - b.h).abs() < 1e-3 && (a.s - b.s).abs() < 1e-3 && (a.l - b.l).abs() < 1e-3 && (a.a - b.a).abs() < 1e-3
    }

    #[test]
    fn the_palette_keeps_its_accent_and_its_three_depths() {
        let (light, dark) = (Tokens::light(), Tokens::dark());
        // The logo's pink, as index.css had it.
        assert!(close(light.primary, hsl(333., 71., 51.)));
        assert!(close(dark.primary, hsl(330., 81., 60.)));
        assert!(close(light.background, hsl(0., 0., 100.)));
        // Dark: the chrome is behind the sheet, and what floats is in front of it. Light: the sheet is the white.
        assert!(dark.chrome.l < dark.background.l && dark.background.l < dark.card.l && dark.card.l < dark.popover.l);
        assert!(light.chrome.l < light.background.l);
        // Hover, pressed and selected fills are see-through, so they read on every depth; pressed is the stronger.
        for t in [&light, &dark] {
            assert!(t.accent.a < 0.2 && t.pressed.a > t.accent.a, "{:?}", t.accent);
            assert!(t.selected.a < t.selected_hover.a && t.stroke.a < 0.2);
        }
        assert!(dark.dark && !light.dark);
    }

    /// WCAG's contrast ratio of two opaque colors.
    fn contrast(a: Hsla, b: Hsla) -> f32 {
        fn luminance(color: Hsla) -> f32 {
            let rgb = gpui_kit::Rgba::from(color);
            let channel = |v: f32| {
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(rgb.r) + 0.7152 * channel(rgb.g) + 0.0722 * channel(rgb.b)
        }
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn text_reads_on_every_depth() {
        for t in [Tokens::light(), Tokens::dark()] {
            for surface in [t.background, t.chrome, t.card, t.popover] {
                // Body text well past AA; the quiet text at AA (4.5:1) even on the chrome.
                assert!(contrast(t.foreground, surface) >= 12., "{:?}", surface);
                assert!(
                    contrast(t.muted_foreground, surface) >= 4.5,
                    "{} on {:?}",
                    contrast(t.muted_foreground, surface),
                    surface
                );
            }
            // The accent as text and under its own text.
            assert!(
                contrast(t.primary, t.background) >= 4.4,
                "{}",
                contrast(t.primary, t.background)
            );
            assert!(contrast(t.primary_foreground, t.primary) >= 4.4);
        }
    }

    #[test]
    fn windows_accent_replaces_pink_with_its_shade_for_the_mode() {
        let accent = WindowsAccent {
            accent: [0, 120, 212],
            light: [[40, 140, 220], [60, 160, 235], [100, 190, 245]],
            dark: [[0, 100, 180], [0, 80, 150], [0, 60, 120]],
        };
        // Light mode: the darker first shade; dark mode: the lighter second one.
        assert!(close(
            Tokens::new(false, Some(&accent)).primary,
            from_rgb([0, 100, 180])
        ));
        assert!(close(
            Tokens::new(true, Some(&accent)).primary,
            from_rgb([60, 160, 235])
        ));
        assert!(close(
            Tokens::new(false, Some(&accent)).ring,
            Tokens::new(false, Some(&accent)).primary
        ));
        // Without one, Glint's pink stays.
        assert!(close(Tokens::new(false, None).primary, hsl(333., 71., 51.)));
        // Text on a dark blue is white, on a light yellow it is near-black.
        assert!(close(text_on([0, 100, 180]), hsl(0., 0., 100.)));
        assert!(close(text_on([255, 230, 100]), hsl(0., 0., 7.)));
    }

    #[test]
    fn rgb_channels_convert_to_the_color_they_name() {
        assert!(close(from_rgb([255, 255, 255]), hsl(0., 0., 100.)));
        assert!(close(from_rgb([0, 0, 0]), hsl(0., 0., 0.)));
        let red = from_rgb([255, 0, 0]);
        assert!(red.h.abs() < 1e-3 && (red.s - 1.).abs() < 1e-3 && (red.l - 0.5).abs() < 1e-3);
    }

    #[test]
    fn windows_reports_an_accent_color_with_ordered_shades() {
        // Needs WinRT, which every Windows 10+ PC has; skipped where it can't answer (a service session).
        let Ok(accent) = read_windows_accent() else { return };
        let sum = |c: [u8; 3]| c.iter().map(|&v| u32::from(v)).sum::<u32>();
        assert!(
            sum(accent.light[2]) >= sum(accent.accent) && sum(accent.dark[2]) <= sum(accent.accent),
            "{accent:?}"
        );
    }
}
