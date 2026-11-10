//! What the map's colors mean: the three ways to color it (file type, age, growth), the bands of the last two,
//! and the palette the tiles are drawn in. Ported from the top of src/components/explorer/DiskUsage.tsx.
//!
//! The hues of the file types come from `glint_model::file_types` (`TYPE_GROUPS`); the bands of age and growth
//! are the map's own, as they were in the TypeScript. The frames, text and outlines come from the theme's tokens,
//! mixed the way the TypeScript's two hand-made palettes were (a gray step per depth).

use crate::theme::{hsl, Tokens};
use glint_model::entries::FileEntry;
use glint_model::file_types::{TypeGroup, TYPE_GROUPS};
use glint_model::usage::{UsageMap, WAS_NOT_THERE, WRITTEN_SINCE};
use gpui_kit::{hsla, Hsla};

/// A day, in milliseconds.
pub const DAY_MS: f64 = 86_400_000.0;

/// What the tiles are colored by (`ColorMode`). The T key goes round them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorMode {
    /// By kind of file: pictures, videos, code…
    #[default]
    Type,
    /// By when the file last changed, from today ("hot") to over a year ago ("frozen").
    Age,
    /// By what grew since the index's daily mark.
    Growth,
}

impl ColorMode {
    /// The mode T switches to (`NEXT_MODE`): type, age, growth, type…
    pub fn next(self) -> Self {
        match self {
            Self::Type => Self::Age,
            Self::Age => Self::Growth,
            Self::Growth => Self::Type,
        }
    }
}

/// New since the mark.
pub const GROWTH_NEW: TypeGroup = TypeGroup {
    name: "New",
    hue: 286,
    sat: 58,
};
/// At least doubled (and by a megabyte or more).
pub const GROWTH_DOUBLED: TypeGroup = TypeGroup {
    name: "Doubled or more",
    hue: 0,
    sat: 74,
};
/// Bigger than at the mark.
pub const GROWTH_GREW: TypeGroup = TypeGroup {
    name: "Grew",
    hue: 24,
    sat: 82,
};
/// A file written to since the mark (what it held isn't kept).
pub const GROWTH_WRITTEN: TypeGroup = TypeGroup {
    name: "Written to",
    hue: 46,
    sat: 80,
};
/// As big as at the mark.
pub const GROWTH_SAME: TypeGroup = TypeGroup {
    name: "Same",
    hue: 220,
    sat: 7,
};
/// Smaller than at the mark.
pub const GROWTH_SHRANK: TypeGroup = TypeGroup {
    name: "Shrank",
    hue: 198,
    sat: 62,
};

/// The growth bands in the order the legend lists them (`GROWTH_ORDER`).
pub const GROWTH_ORDER: [TypeGroup; 6] = [
    GROWTH_NEW,
    GROWTH_DOUBLED,
    GROWTH_GREW,
    GROWTH_WRITTEN,
    GROWTH_SAME,
    GROWTH_SHRANK,
];

/// What happened to node `i` since the index's mark: from new to shrank (`growthBand`). Only for a map that
/// has `growth`; without it every item reads as the same.
pub fn growth_band(map: &UsageMap, i: usize) -> TypeGroup {
    let then = map
        .growth
        .as_ref()
        .and_then(|g| g.previous.get(i).copied())
        .unwrap_or(f64::NAN);
    let now = map.size.get(i).copied().unwrap_or(0.0);
    if then == WAS_NOT_THERE {
        return GROWTH_NEW;
    }
    if then == WRITTEN_SINCE {
        return GROWTH_WRITTEN;
    }
    // Doubling counts from a megabyte up: a few bytes more in a tiny folder isn't news.
    if now > then {
        if now >= 2.0 * then && now - then >= f64::from(1u32 << 20) {
            GROWTH_DOUBLED
        } else {
            GROWTH_GREW
        }
    } else if now < then {
        GROWTH_SHRANK
    } else {
        GROWTH_SAME
    }
}

/// A band of the thermal age spectrum.
#[derive(Clone, Copy, Debug)]
pub struct AgeBand {
    pub group: TypeGroup,
    /// Files up to this old (ms) are in the band.
    pub max_age_ms: f64,
}

/// From hot (changed in the last 24 hours) to frozen (untouched for over a year) (`AGE_BANDS`).
pub const AGE_BANDS: [AgeBand; 6] = [
    AgeBand {
        group: TypeGroup {
            name: "Hot (<24h)",
            hue: 8,
            sat: 84,
        },
        max_age_ms: DAY_MS,
    },
    AgeBand {
        group: TypeGroup {
            name: "Recent (≤7d)",
            hue: 26,
            sat: 82,
        },
        max_age_ms: 7.0 * DAY_MS,
    },
    AgeBand {
        group: TypeGroup {
            name: "Warm (≤30d)",
            hue: 44,
            sat: 76,
        },
        max_age_ms: 30.0 * DAY_MS,
    },
    AgeBand {
        group: TypeGroup {
            name: "Cool (≤6m)",
            hue: 172,
            sat: 54,
        },
        max_age_ms: 180.0 * DAY_MS,
    },
    AgeBand {
        group: TypeGroup {
            name: "Cold (≤1y)",
            hue: 204,
            sat: 58,
        },
        max_age_ms: 365.0 * DAY_MS,
    },
    AgeBand {
        group: TypeGroup {
            name: "Frozen (>1y)",
            hue: 226,
            sat: 42,
        },
        max_age_ms: f64::INFINITY,
    },
];

/// The band a file modified at `modified` (Unix ms; 0 for unknown) is in at time `now` (`ageBand`). An unknown
/// date is as old as it gets.
pub fn age_band(modified: i64, now: f64) -> TypeGroup {
    let frozen = AGE_BANDS[AGE_BANDS.len() - 1].group;
    if modified <= 0 {
        return frozen;
    }
    // A date in the future counts as just now.
    let age = (now - modified as f64).max(0.0);
    AGE_BANDS
        .iter()
        .find(|band| age <= band.max_age_ms)
        .map_or(frozen, |band| band.group)
}

/// Files of a type nobody listed still get a color of their own, muted, from their extension (`extensionHue`).
/// The hash runs over UTF-16 units as the TypeScript's did.
pub fn extension_hue(ext: &str) -> u32 {
    let mut hash: u32 = 0;
    for unit in ext.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(u32::from(unit));
    }
    hash % 360
}

/// One color halfway between `a` and `b` by `amount` (0 is `a`, 1 is `b`). The tokens used with it are grays, so
/// mixing hue and saturation along with lightness only gives them the faint tint the TypeScript's frames had.
fn mix(a: Hsla, b: Hsla, amount: f32) -> Hsla {
    hsla(
        a.h + (b.h - a.h) * amount,
        a.s + (b.s - a.s) * amount,
        a.l + (b.l - a.l) * amount,
        a.a + (b.a - a.a) * amount,
    )
}

/// The colors the map is drawn in, from the theme's tokens (`THEMES`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapColors {
    pub dark: bool,
    /// Lightness of file tiles, 0 to 100.
    pub lightness: f32,
    pub background: Hsla,
    /// Frame fill by depth (1 is right inside the folder mapped).
    pub frames: [Hsla; 4],
    pub text: Hsla,
    pub muted: Hsla,
    /// Folders drawn whole (too deep or too small to show inside).
    pub folder: Hsla,
    /// Text on tiles.
    pub label: Hsla,
    pub label_muted: Hsla,
    /// The outline of the item under the pointer.
    pub hover: Hsla,
    pub hover_fill: Hsla,
    /// The outline of selected items.
    pub selected: Hsla,
    /// "Smaller items": stripes over this.
    pub rest: Hsla,
    pub stripe: Hsla,
    /// The light and the shadow of each tile's cushion.
    pub shine: Hsla,
    pub shade: Hsla,
    /// What covers the map while a legend entry is spotlit.
    pub dim: Hsla,
}

impl MapColors {
    /// The map's palette for these tokens. The tile lightness, the folder color and the label opacities are the
    /// TypeScript's own numbers for each theme; the grays derive from the background and foreground.
    pub fn new(t: &Tokens) -> Self {
        let dark = t.dark;
        let steps = if dark {
            [0.06, 0.10, 0.14, 0.18]
        } else {
            [0.10, 0.15, 0.20, 0.25]
        };
        let gray = |amount: f32| mix(t.background, t.foreground, amount);
        Self {
            dark,
            lightness: if dark { 43. } else { 63. },
            background: t.background,
            frames: steps.map(gray),
            text: t.foreground,
            muted: t.muted_foreground,
            folder: if dark { hsl(38., 16., 33.) } else { hsl(38., 24., 70.) },
            label: if dark {
                hsla(0., 0., 1., 0.94)
            } else {
                hsla(0., 0., 0., 0.86)
            },
            label_muted: if dark {
                hsla(0., 0., 1., 0.7)
            } else {
                hsla(0., 0., 0., 0.6)
            },
            hover: if dark {
                hsla(0., 0., 1., 0.95)
            } else {
                hsla(0., 0., 0., 0.85)
            },
            hover_fill: hsla(0., 0., 1., if dark { 0.14 } else { 0.3 }),
            selected: t.primary,
            rest: gray(0.15),
            stripe: gray(0.23),
            shine: hsla(0., 0., 1., if dark { 0.2 } else { 0.35 }),
            shade: hsla(0., 0., 0., if dark { 0.28 } else { 0.16 }),
            dim: t.background.opacity(if dark { 0.64 } else { 0.66 }),
        }
    }

    /// The fill of a band or group at this theme's lightness (a legend swatch, an age or growth tile).
    pub fn band(&self, group: &TypeGroup) -> Hsla {
        hsl(group.hue as f32, group.sat as f32, self.lightness)
    }

    /// A file's tile color (`tileColor`): its group's, or for a file of a type nobody listed, a muted color of
    /// its own from its extension.
    pub fn tile(&self, entry: &FileEntry, group: &TypeGroup) -> Hsla {
        if *group == TYPE_GROUPS.other && !entry.ext.is_empty() {
            hsl(extension_hue(&entry.ext) as f32, 22., self.lightness)
        } else {
            self.band(group)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_model::usage::UsageGrowth;

    fn map_with(previous: Vec<f64>, size: Vec<f64>) -> UsageMap {
        UsageMap {
            path: "C:\\x".into(),
            engine: "index",
            elapsed: 1.0,
            limited: false,
            service_too_old: false,
            growth_too_old: false,
            growth: Some(UsageGrowth {
                since: 0.0,
                oldest: 0.0,
                drive_then: 0.0,
                drive_now: 0.0,
                previous,
            }),
            count: size.len(),
            size,
            modified: vec![],
            attrs: vec![],
            parent: vec![],
            names: vec![],
            paths: vec![],
            first: vec![],
            end: vec![],
        }
    }

    #[test]
    fn t_goes_round_the_three_modes() {
        assert_eq!(ColorMode::Type.next(), ColorMode::Age);
        assert_eq!(ColorMode::Age.next(), ColorMode::Growth);
        assert_eq!(ColorMode::Growth.next(), ColorMode::Type);
    }

    #[test]
    fn age_bands_run_from_hot_to_frozen_and_an_unknown_date_is_frozen() {
        let now = 1_000.0 * DAY_MS;
        let ago = |days: f64| (now - days * DAY_MS) as i64;
        assert_eq!(age_band(ago(0.5), now).name, "Hot (<24h)");
        assert_eq!(age_band(ago(3.0), now).name, "Recent (≤7d)");
        assert_eq!(age_band(ago(20.0), now).name, "Warm (≤30d)");
        assert_eq!(age_band(ago(100.0), now).name, "Cool (≤6m)");
        assert_eq!(age_band(ago(300.0), now).name, "Cold (≤1y)");
        assert_eq!(age_band(ago(900.0), now).name, "Frozen (>1y)");
        assert_eq!(age_band(0, now).name, "Frozen (>1y)");
        assert_eq!(age_band(-5, now).name, "Frozen (>1y)");
        // A date ahead of the clock is "just now".
        assert_eq!(age_band((now + DAY_MS) as i64, now).name, "Hot (<24h)");
    }

    #[test]
    fn growth_bands_follow_the_typescript() {
        let mb = f64::from(1u32 << 20);
        let map = map_with(
            vec![
                0.0,
                WAS_NOT_THERE,
                WRITTEN_SINCE,
                mb,
                10.0,
                4.0 * mb,
                3.0 * mb,
                5.0 * mb,
            ],
            vec![0.0, 5.0, 5.0, 3.0 * mb, 100.0, 4.0 * mb, 5.0 * mb, 2.0 * mb],
        );
        assert_eq!(growth_band(&map, 1).name, "New");
        assert_eq!(growth_band(&map, 2).name, "Written to");
        // Tripled, and by more than a megabyte.
        assert_eq!(growth_band(&map, 3).name, "Doubled or more");
        // Tenfold, but a few bytes: not news.
        assert_eq!(growth_band(&map, 4).name, "Grew");
        assert_eq!(growth_band(&map, 5).name, "Same");
        assert_eq!(growth_band(&map, 6).name, "Grew");
        assert_eq!(growth_band(&map, 7).name, "Shrank");
    }

    #[test]
    fn extension_hues_are_stable_and_in_range() {
        assert_eq!(extension_hue(""), 0);
        // 'a' is 97; 97 * 31 + 98 = 3105 for "ab"; % 360 = 225.
        assert_eq!(extension_hue("a"), 97);
        assert_eq!(extension_hue("ab"), 225);
        for ext in ["zzzzzzzzzzzzzzzzzzzz", "تقرير", "日本語"] {
            assert!(extension_hue(ext) < 360);
        }
    }

    #[test]
    fn the_light_and_dark_palettes_differ_where_the_typescripts_did() {
        let (light, dark) = (MapColors::new(&Tokens::light()), MapColors::new(&Tokens::dark()));
        assert_eq!((light.lightness, dark.lightness), (63., 43.));
        // Frames get further from the background with depth in both themes.
        assert!((dark.frames[3].l - dark.background.l).abs() > (dark.frames[0].l - dark.background.l).abs());
        assert!((light.frames[3].l - light.background.l).abs() > (light.frames[0].l - light.background.l).abs());
        assert!(dark.frames[0].l > dark.background.l && light.frames[0].l < light.background.l);
    }

    #[test]
    fn an_unlisted_type_gets_a_color_of_its_own_and_listed_ones_their_groups() {
        let colors = MapColors::new(&Tokens::dark());
        let file = |name: &str| crate::explorer::fake::file("C:\\x", name, 1, 1);
        let unlisted = file("data.qqq");
        let other = colors.tile(&unlisted, &TYPE_GROUPS.other);
        assert_eq!(other, hsl(extension_hue("qqq") as f32, 22., 43.));
        let plain = file("README");
        assert_eq!(colors.tile(&plain, &TYPE_GROUPS.other), colors.band(&TYPE_GROUPS.other));
        let picture = file("a.png");
        assert_eq!(
            colors.tile(&picture, &TYPE_GROUPS.images),
            colors.band(&TYPE_GROUPS.images)
        );
    }
}
