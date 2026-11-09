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
//! **Where the numbers come from.** Colors are the HSL triplets of index.css (`--background: 0 0% 100%`…),
//! written here as `hsl(0., 0., 100.)`. Sizes follow Tailwind's 4 px unit at Glint's base size
//! ([`crate::kit::space`]), and the type scale is Tailwind's: `xs` 12, `sm` 14, `base` 16 px (see [`size`]).
//! Fonts are Windows 11's UI font and Cascadia Mono, with fallbacks for a PC that lacks them.
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
    pub card: Hsla,
    pub popover: Hsla,
    pub popover_foreground: Hsla,
    /// The accent: Glint's pink, or Windows' accent color.
    pub primary: Hsla,
    pub primary_foreground: Hsla,
    pub muted: Hsla,
    pub muted_foreground: Hsla,
    /// Hover and highlight fill. See-through in dark mode (white at 8.5%), as Windows 11's are, so it shows
    /// on every surface.
    pub accent: Hsla,
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
        Self {
            dark: false,
            background: hsl(0., 0., 100.),
            foreground: hsl(0., 0., 11.),
            chrome: hsl(0., 0., 95.3),
            card: hsl(0., 0., 100.),
            popover: hsl(0., 0., 100.),
            popover_foreground: hsl(0., 0., 11.),
            // Pink-600 here, pink-500 in the dark: at least 4.5:1 as text and under white text.
            primary: hsl(333., 71., 51.),
            primary_foreground: hsl(0., 0., 100.),
            muted: hsl(0., 0., 94.),
            muted_foreground: hsl(0., 0., 38.),
            accent: hsl(0., 0., 92.),
            destructive: hsl(0., 72., 51.),
            destructive_foreground: hsl(0., 0., 100.),
            border: hsl(0., 0., 89.),
            input: hsl(0., 0., 82.),
            ring: hsl(333., 71., 51.),
            field: hsl(0., 0., 100.),
            field_focus: hsl(0., 0., 100.),
            field_edge: hsl(0., 0., 60.),
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
        Self {
            dark: true,
            // Dark text on the accent, as Windows 11 has in the dark.
            primary: hsl(330., 81., 60.),
            primary_foreground: hsl(0., 0., 7.),
            ring: hsl(330., 81., 60.),
            background: hsl(0., 0., 11.),
            foreground: hsl(0., 0., 95.),
            chrome: hsl(0., 0., 12.5),
            card: hsl(0., 0., 14.),
            popover: hsl(0., 0., 17.),
            popover_foreground: hsl(0., 0., 95.),
            muted: hsl(0., 0., 16.),
            muted_foreground: hsl(0., 0., 64.),
            accent: hsla(0., 0., 1., 0.085),
            destructive: hsl(0., 72., 60.),
            destructive_foreground: hsl(0., 0., 7.),
            border: hsl(0., 0., 18.5),
            input: hsl(0., 0., 26.),
            field: hsl(0., 0., 17.),
            field_focus: hsl(0., 0., 11.),
            field_edge: hsl(0., 0., 45.),
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
    Theme::update(cx, |theme| {
        theme.font_family = sans;
        theme.mono_font_family = mono;
        theme.font_size = font_size;
        theme.mono_font_size = font_size * 0.875;
        theme.radius = size::RADIUS;
        theme.radius_lg = size::RADIUS_XL;
        map_colors(theme, &tokens, chrome);
    });
}

/// Glint's tokens onto the colors gpui-component's controls read. Hover and pressed states are the
/// translucent `accent` fill laid over the surface, as the CSS did with `hover:bg-accent`.
fn map_colors(theme: &mut Theme, t: &Tokens, chrome: Hsla) {
    let hover = t.accent;
    // A pressed fill is the hover fill twice, as dark as the highlight gets.
    let pressed = t.background.blend(t.accent).blend(t.accent);
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
    c.overlay = hsla(0., 0., 0., 0.5);
    c.list = t.background;
    c.list_even = t.background;
    c.list_head = t.background;
    c.list_hover = hover;
    c.list_active = t.primary.opacity(0.15);
    c.list_active_border = t.primary;
    c.table = t.background;
    c.table_even = t.background;
    c.table_head = t.background;
    c.table_head_foreground = t.muted_foreground;
    c.table_hover = hover;
    c.table_active = t.primary.opacity(0.15);
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
    c.scrollbar_thumb = t.muted_foreground.opacity(0.35);
    c.scrollbar_thumb_hover = t.muted_foreground.opacity(0.55);
    c.drag_border = t.primary;
    c.drop_target = t.primary.opacity(0.2);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Hsla, b: Hsla) -> bool {
        (a.h - b.h).abs() < 1e-3 && (a.s - b.s).abs() < 1e-3 && (a.l - b.l).abs() < 1e-3 && (a.a - b.a).abs() < 1e-3
    }

    #[test]
    fn tokens_are_index_css_values() {
        let (light, dark) = (Tokens::light(), Tokens::dark());
        // Spot checks against :root and .dark in src/index.css.
        assert!(close(light.primary, hsl(333., 71., 51.)));
        assert!(close(light.background, hsl(0., 0., 100.)));
        assert!(close(light.chrome, hsl(0., 0., 95.3)));
        assert!(close(dark.primary, hsl(330., 81., 60.)));
        assert!(close(dark.background, hsl(0., 0., 11.)));
        // The dark highlight is see-through white; the light one is opaque gray.
        assert!((dark.accent.a - 0.085).abs() < 1e-6 && (light.accent.a - 1.0).abs() < 1e-6);
        assert!(dark.dark && !light.dark);
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
