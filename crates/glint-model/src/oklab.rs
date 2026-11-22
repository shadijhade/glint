//! Perceptual OKLab color math, deterministic dominant-palette extraction, and color filtering.
//!
//! Uses Björn Ottosson's OKLab color space (D65 sRGB -> linear RGB -> LMS -> cube root -> Lab) so
//! Euclidean distance matches human color perception closely without ML models or GPU shaders.

use serde::{Deserialize, Serialize};

/// Perceptual OKLab color (`L` in `[0, 1]`, `a`/`b` roughly in `[-0.4, 0.4]`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Oklab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

/// Top three dominant perceptual colors extracted from an image, sorted by weight descending.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Palette3 {
    pub colors: [Oklab; 3],
    pub weights: [f32; 3],
}

/// Live OKLab color-wheel filter state (`target` color + maximum Euclidean OKLab radius).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorFilter {
    pub target: Oklab,
    pub max_dist: f32,
}

impl ColorFilter {
    pub const DEFAULT_TOLERANCE: f32 = 0.22;

    pub fn new(target: Oklab) -> Self {
        Self {
            target,
            max_dist: Self::DEFAULT_TOLERANCE,
        }
    }
}

#[inline]
fn srgb_to_linear(u: u8) -> f32 {
    let s = (u as f32) / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
fn linear_to_srgb_byte(c: f32) -> u8 {
    let clamped = c.clamp(0.0, 1.0);
    let s = if clamped <= 0.003_130_8 {
        12.92 * clamped
    } else {
        1.055 * clamped.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

/// Converts 8-bit sRGB channels to OKLab.
#[inline]
pub fn srgb_to_oklab(r: u8, g: u8, b: u8) -> Oklab {
    let lr = srgb_to_linear(r);
    let lg = srgb_to_linear(g);
    let lb = srgb_to_linear(b);

    let l = 0.412_221_46 * lr + 0.536_332_55 * lg + 0.051_445_995 * lb;
    let m = 0.211_903_5 * lr + 0.680_699_5 * lg + 0.107_396_96 * lb;
    let s = 0.088_302_46 * lr + 0.281_718_85 * lg + 0.629_978_7 * lb;

    let l_ = l.max(0.0).cbrt();
    let m_ = m.max(0.0).cbrt();
    let s_ = s.max(0.0).cbrt();

    Oklab {
        l: 0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_,
        a: 1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_,
        b: 0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_,
    }
}

/// Converts an OKLab color back to clamped 8-bit sRGB channels.
#[inline]
pub fn oklab_to_srgb(c: Oklab) -> [u8; 3] {
    let l_ = c.l + 0.396_337_78 * c.a + 0.215_803_76 * c.b;
    let m_ = c.l - 0.105_561_346 * c.a - 0.063_854_17 * c.b;
    let s_ = c.l - 0.089_484_18 * c.a - 1.291_485_5 * c.b;

    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;

    let r = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
    let g = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s;
    let b = -0.004_196_086 * l - 0.703_418_6 * m + 1.707_614_7 * s;

    [linear_to_srgb_byte(r), linear_to_srgb_byte(g), linear_to_srgb_byte(b)]
}

/// Maps normalized wheel disk coordinates `(nx, ny)` in `[-1.0, 1.0]` at `lightness` to OKLab.
#[inline]
pub fn wheel_point_to_oklab(nx: f32, ny: f32, lightness: f32) -> Oklab {
    let r2 = nx * nx + ny * ny;
    let (cx, cy) = if r2 > 1.0 {
        let inv = r2.sqrt().recip();
        (nx * inv, ny * inv)
    } else {
        (nx, ny)
    };
    Oklab {
        l: lightness.clamp(0.15, 0.95),
        a: cx * WHEEL_MAX_CHROMA,
        b: -cy * WHEEL_MAX_CHROMA,
    }
}

/// The chroma that the wheel's edge stands for (`wheel_point_to_oklab`'s radius).
pub const WHEEL_MAX_CHROMA: f32 = 0.32;

/// The wheel disk at `lightness` as `side` x `side` straight-alpha BGRA pixels (what GPUI draws): each pixel is the
/// color `wheel_point_to_oklab` gives for its center, and the rim is anti-aliased by the pixel's coverage of the
/// disk. Empty when `side` is 0.
pub fn wheel_raster_bgra(side: u32, lightness: f32) -> Vec<u8> {
    let n = side as usize;
    let mut out = vec![0u8; n * n * 4];
    if n == 0 {
        return out;
    }
    let radius = n as f32 / 2.0;
    for iy in 0..n {
        let dy = iy as f32 + 0.5 - radius;
        for ix in 0..n {
            let dx = ix as f32 + 0.5 - radius;
            // Coverage of a one-pixel-wide rim: 1 inside it, 0 outside, a ramp across.
            let coverage = (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let [r, g, b] = oklab_to_srgb(wheel_point_to_oklab(dx / radius, dy / radius, lightness));
            let at = (iy * n + ix) * 4;
            out[at..at + 4].copy_from_slice(&[b, g, r, (coverage * 255.0 + 0.5) as u8]);
        }
    }
    out
}

/// Squared Euclidean OKLab distance between two colors.
#[inline]
pub fn oklab_dist_sq(x: Oklab, y: Oklab) -> f32 {
    let dl = x.l - y.l;
    let da = x.a - y.a;
    let db = x.b - y.b;
    dl * dl + da * da + db * db
}

/// Minimum weighted squared OKLab distance from `target` to a photo's extracted palette.
/// Lower is closer; dominant palette clusters receive a tiny rank preference over tiny accents.
#[inline]
pub fn palette_distance_sq(palette: &Palette3, target: Oklab) -> f32 {
    let mut best = f32::INFINITY;
    for i in 0..3 {
        let w = palette.weights[i];
        if w <= 0.0 {
            continue;
        }
        let d = oklab_dist_sq(palette.colors[i], target) + (1.0 - w).clamp(0.0, 1.0) * 0.004;
        if d < best {
            best = d;
        }
    }
    best
}

/// Deterministically extracts up to 3 dominant perceptual colors from straight/premultiplied BGRA pixels.
/// Subsamples ~256 opaque pixels and runs deterministic farthest-point initialization + 6 k-means passes.
pub fn extract_palette_bgra(width: u32, height: u32, bgra: &[u8]) -> Option<Palette3> {
    if width == 0 || height == 0 {
        return None;
    }
    let expected = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
    if bgra.len() < expected {
        return None;
    }
    let pixel_count = (width as usize) * (height as usize);
    let step = (pixel_count / 256).max(1);

    let mut samples: Vec<Oklab> = Vec::with_capacity(256);
    let mut idx = 0usize;
    while idx < pixel_count && samples.len() < 256 {
        let off = idx * 4;
        let b = bgra[off];
        let g = bgra[off + 1];
        let r = bgra[off + 2];
        let a = bgra[off + 3];
        if a >= 128 {
            samples.push(srgb_to_oklab(r, g, b));
        }
        idx += step;
    }
    if samples.is_empty() {
        // Fallback scan if strided pixels happened to hit transparent borders.
        for chunk in bgra[..expected].as_chunks::<4>().0 {
            if chunk[3] >= 128 {
                samples.push(srgb_to_oklab(chunk[2], chunk[1], chunk[0]));
                if samples.len() >= 256 {
                    break;
                }
            }
        }
    }
    if samples.is_empty() {
        return None;
    }

    // Seed 1: highest-chroma pixel (fallback center sample if all neutral gray).
    let mut seed0 = samples[samples.len() / 2];
    let mut best_chroma = -1.0f32;
    for &s in &samples {
        let chroma = s.a * s.a + s.b * s.b;
        if chroma > best_chroma {
            best_chroma = chroma;
            seed0 = s;
        }
    }

    // Seed 2: farthest point from seed0.
    let mut seed1 = seed0;
    let mut max_d1 = -1.0f32;
    for &s in &samples {
        let d = oklab_dist_sq(s, seed0);
        if d > max_d1 {
            max_d1 = d;
            seed1 = s;
        }
    }

    // Seed 3: farthest point from closest of {seed0, seed1}.
    let mut seed2 = seed1;
    let mut max_d2 = -1.0f32;
    for &s in &samples {
        let d = oklab_dist_sq(s, seed0).min(oklab_dist_sq(s, seed1));
        if d > max_d2 {
            max_d2 = d;
            seed2 = s;
        }
    }

    let mut centroids = [seed0, seed1, seed2];
    let mut counts = [0usize; 3];

    for _ in 0..6 {
        let mut sum_l = [0.0f32; 3];
        let mut sum_a = [0.0f32; 3];
        let mut sum_b = [0.0f32; 3];
        counts = [0usize; 3];

        for &s in &samples {
            let d0 = oklab_dist_sq(s, centroids[0]);
            let d1 = oklab_dist_sq(s, centroids[1]);
            let d2 = oklab_dist_sq(s, centroids[2]);
            let best = if d0 <= d1 && d0 <= d2 {
                0
            } else if d1 <= d2 {
                1
            } else {
                2
            };
            counts[best] += 1;
            sum_l[best] += s.l;
            sum_a[best] += s.a;
            sum_b[best] += s.b;
        }

        for i in 0..3 {
            if counts[i] > 0 {
                let inv = (counts[i] as f32).recip();
                centroids[i] = Oklab {
                    l: sum_l[i] * inv,
                    a: sum_a[i] * inv,
                    b: sum_b[i] * inv,
                };
            }
        }
    }

    let total = (counts[0] + counts[1] + counts[2]).max(1) as f32;
    let mut order = [0usize, 1, 2];
    order.sort_by(|&i, &j| counts[j].cmp(&counts[i]));

    Some(Palette3 {
        colors: [centroids[order[0]], centroids[order[1]], centroids[order[2]]],
        weights: [
            (counts[order[0]] as f32) / total,
            (counts[order[1]] as f32) / total,
            (counts[order[2]] as f32) / total,
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_oklab_round_trip_on_primary_and_neutral_colors() {
        for [r, g, b] in [
            [0, 0, 0],
            [255, 255, 255],
            [128, 128, 128],
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [255, 190, 40],
        ] {
            let lab = srgb_to_oklab(r, g, b);
            let back = oklab_to_srgb(lab);
            assert!(
                back[0].abs_diff(r) <= 1 && back[1].abs_diff(g) <= 1 && back[2].abs_diff(b) <= 1,
                "round-trip drift for ({r},{g},{b}): got {back:?}"
            );
        }
    }

    #[test]
    fn extract_palette_finds_dominant_and_secondary_colors() {
        // Build a 16x16 BGRA image: top 12 rows orange (255,140,20), bottom 4 rows sky blue (30,130,245).
        let (w, h) = (16u32, 16u32);
        let mut bgra = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for _x in 0..w {
                if y < 12 {
                    bgra.extend_from_slice(&[20, 140, 255, 255]);
                } else {
                    bgra.extend_from_slice(&[245, 130, 30, 255]);
                }
            }
        }
        let palette = extract_palette_bgra(w, h, &bgra).expect("palette extracted");
        assert!(palette.weights[0] > 0.65, "weights={:?}", palette.weights);

        let orange = srgb_to_oklab(255, 140, 20);
        let blue = srgb_to_oklab(30, 130, 245);
        let green = srgb_to_oklab(30, 200, 60);
        assert!(palette_distance_sq(&palette, orange) < 0.01);
        assert!(palette_distance_sq(&palette, blue) < palette_distance_sq(&palette, green));
    }

    #[test]
    fn transparent_image_returns_none_without_panicking() {
        let bgra = vec![0u8; 4 * 4 * 4];
        assert!(extract_palette_bgra(4, 4, &bgra).is_none());
        assert!(extract_palette_bgra(4, 4, &[1, 2]).is_none());
    }
}
