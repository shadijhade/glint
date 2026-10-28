//! Windows 11's look: Mica Alt behind the title bar and the sidebar (Settings → Appearance →
//! Backdrop), and the accent color Windows uses (Settings → Appearance → Accent color).

use tauri::window::{Effect, EffectsBuilder};
use tauri::{Theme, WebviewWindow};

/// The material Windows draws behind the window: Mica Alt (as File Explorer has) from Windows 11 22H2,
/// Mica on the first Windows 11, nothing before.
fn backdrop() -> Option<Effect> {
    #[cfg(windows)]
    match windows_version::OsVersion::current().build {
        build if build >= 22523 => Some(Effect::Tabbed),
        build if build >= 22000 => Some(Effect::Mica),
        _ => None,
    }
    #[cfg(not(windows))]
    None
}

/// The window's theme, which its frame and the backdrop's tint follow (Glint's own Light, Dark or
/// System, not only Windows'), and the backdrop, on or off. Whether a backdrop is on: the page then
/// lets it show through its title bar and sidebar.
#[tauri::command]
pub fn appearance(window: WebviewWindow, theme: String, backdrop: bool) -> bool {
    let _ = window.set_theme(match theme.as_str() {
        "light" => Some(Theme::Light),
        "dark" => Some(Theme::Dark),
        _ => None,
    });
    match self::backdrop().filter(|_| backdrop) {
        Some(effect) => window.set_effects(EffectsBuilder::new().effect(effect).build()).is_ok(),
        None => {
            let _ = window.set_effects(None::<tauri::utils::config::WindowEffectsConfig>);
            false
        }
    }
}

/// Windows' accent color and its shades, as #rrggbb: the base, then lighter (1–3) and darker (1–3).
/// Windows 11 fills with the darker first shade in light mode, the lighter second one in dark mode.
#[derive(serde::Serialize, Debug, PartialEq)]
pub struct Accent {
    pub accent: String,
    pub light: [String; 3],
    pub dark: [String; 3],
}

#[tauri::command]
pub async fn accent_colors() -> Result<Accent, String> {
    crate::fsx::blocking(imp::accent).await
}

#[cfg(windows)]
mod imp {
    use super::Accent;
    use windows::UI::ViewManagement::{UIColorType, UISettings};
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    pub fn accent() -> Result<Accent, String> {
        // WinRT on a thread of the pool: in the process's multithreaded apartment.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let settings = UISettings::new().map_err(|e| e.to_string())?;
        let color = |kind: UIColorType| settings.GetColorValue(kind).map(|c| format!("#{:02x}{:02x}{:02x}", c.R, c.G, c.B)).map_err(|e| e.to_string());
        Ok(Accent {
            accent: color(UIColorType::Accent)?,
            light: [color(UIColorType::AccentLight1)?, color(UIColorType::AccentLight2)?, color(UIColorType::AccentLight3)?],
            dark: [color(UIColorType::AccentDark1)?, color(UIColorType::AccentDark2)?, color(UIColorType::AccentDark3)?],
        })
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn accent() -> Result<super::Accent, String> {
        Err("Only Windows has an accent color".into())
    }
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn reads_windows_accent_color() {
        let accent = super::imp::accent().unwrap();
        let hex = |s: &str| s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit());
        assert!(hex(&accent.accent) && accent.light.iter().chain(&accent.dark).all(|s| hex(s)), "{accent:?}");
        // The shades go lighter and darker.
        let sum = |s: &str| (1..7).step_by(2).map(|i| u32::from_str_radix(&s[i..i + 2], 16).unwrap()).sum::<u32>();
        assert!(sum(&accent.light[2]) >= sum(&accent.accent) && sum(&accent.dark[2]) <= sum(&accent.accent), "{accent:?}");
    }
}
