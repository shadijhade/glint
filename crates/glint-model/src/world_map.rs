//! Offline 120 FPS Web Mercator vector world map projection and O(N) screen-space pin clustering.
//!
//! Zero network tiles, zero AI: embedded simplified continent and major island polygons paired with
//! deterministic Web Mercator camera math and screen-space cluster bucketing.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Maximum latitude representable in standard Web Mercator (~85.0511°).
const MAX_MERCATOR_LAT: f64 = 85.051_128_78;

/// Offline simplified landmass polygon rings in `(longitude_deg, latitude_deg)`.
/// Every ring stays strictly inside `[-180.0, 180.0]` longitude so no polygon wraps across the antimeridian.
pub const LAND_POLYGONS: &[&[(f32, f32)]] = &[
    // North America & Central America
    &[
        (-168.0, 65.5),
        (-160.0, 70.5),
        (-136.0, 69.5),
        (-124.0, 70.0),
        (-102.0, 69.0),
        (-95.0, 66.0),
        (-82.0, 63.0),
        (-78.0, 53.0),
        (-92.0, 57.0),
        (-82.0, 62.0),
        (-64.0, 60.0),
        (-55.5, 52.0),
        (-60.0, 46.5),
        (-66.5, 44.5),
        (-70.0, 41.8),
        (-75.5, 35.5),
        (-81.2, 29.0),
        (-80.2, 25.2),
        (-82.5, 27.5),
        (-84.5, 30.0),
        (-90.0, 29.2),
        (-97.2, 26.0),
        (-96.2, 19.2),
        (-90.5, 21.2),
        (-87.0, 21.4),
        (-88.2, 15.8),
        (-83.5, 14.5),
        (-83.0, 9.8),
        (-77.5, 8.2),
        (-79.5, 8.8),
        (-85.5, 10.5),
        (-92.2, 14.5),
        (-105.5, 20.5),
        (-110.0, 23.0),
        (-114.5, 31.2),
        (-110.5, 24.5),
        (-117.1, 32.5),
        (-122.4, 37.8),
        (-124.5, 43.0),
        (-124.7, 48.4),
        (-130.0, 54.5),
        (-137.0, 58.5),
        (-150.0, 60.0),
        (-161.5, 55.5),
        (-166.0, 60.0),
        (-168.0, 65.5),
    ],
    // Greenland
    &[
        (-72.0, 77.0),
        (-56.0, 82.0),
        (-30.0, 83.2),
        (-18.0, 80.5),
        (-21.5, 70.5),
        (-34.0, 66.0),
        (-43.5, 59.8),
        (-52.0, 64.5),
        (-54.5, 70.5),
        (-67.0, 76.0),
        (-72.0, 77.0),
    ],
    // Caribbean (Cuba + Hispaniola simplified)
    &[
        (-84.9, 21.8),
        (-79.0, 23.0),
        (-74.1, 20.2),
        (-77.5, 19.9),
        (-82.5, 21.7),
        (-84.9, 21.8),
    ],
    &[
        (-74.5, 18.5),
        (-68.4, 19.6),
        (-68.4, 18.2),
        (-73.5, 18.0),
        (-74.5, 18.5),
    ],
    // South America
    &[
        (-77.5, 8.5),
        (-75.0, 10.8),
        (-71.5, 12.2),
        (-62.0, 10.6),
        (-52.0, 4.5),
        (-50.0, 1.0),
        (-42.0, -2.8),
        (-35.0, -6.0),
        (-38.5, -13.0),
        (-40.5, -22.5),
        (-48.5, -27.5),
        (-53.5, -34.0),
        (-58.0, -38.0),
        (-65.0, -43.0),
        (-68.0, -52.5),
        (-66.0, -55.0),
        (-73.5, -54.0),
        (-74.5, -46.0),
        (-72.0, -35.0),
        (-70.5, -18.5),
        (-77.2, -12.0),
        (-81.2, -4.8),
        (-80.0, 1.0),
        (-77.5, 8.5),
    ],
    // Great Britain
    &[
        (-5.5, 50.1),
        (-1.2, 50.8),
        (1.7, 52.5),
        (-0.2, 54.0),
        (-2.0, 56.0),
        (-3.0, 58.6),
        (-5.8, 58.2),
        (-5.0, 55.2),
        (-3.2, 53.3),
        (-5.0, 51.7),
        (-5.5, 50.1),
    ],
    // Ireland
    &[(-10.2, 51.8), (-9.8, 54.3), (-6.0, 55.2), (-6.1, 52.2), (-10.2, 51.8)],
    // Iceland
    &[
        (-24.2, 65.5),
        (-18.0, 66.4),
        (-13.6, 65.1),
        (-15.0, 63.8),
        (-22.6, 63.9),
        (-24.2, 65.5),
    ],
    // Continental Europe & Scandinavia
    &[
        (-9.3, 38.7),
        (-9.0, 43.4),
        (-1.8, 43.4),
        (-4.5, 48.4),
        (2.0, 51.0),
        (5.5, 53.5),
        (8.5, 55.0),
        (10.5, 57.7),
        (12.2, 55.8),
        (9.5, 55.0),
        (13.5, 54.2),
        (19.0, 54.5),
        (21.0, 56.0),
        (24.5, 59.5),
        (29.8, 60.0),
        (21.5, 65.5),
        (17.5, 62.0),
        (18.0, 59.3),
        (16.0, 56.2),
        (13.0, 55.5),
        (10.8, 59.8),
        (5.2, 60.4),
        (10.5, 65.0),
        (19.0, 69.5),
        (29.0, 71.0),
        (40.0, 67.5),
        (44.0, 66.0),
        (60.0, 69.5),
        (60.0, 52.0),
        (47.0, 46.5),
        (41.0, 43.0),
        (36.0, 45.2),
        (33.5, 44.5),
        (30.5, 46.0),
        (28.0, 41.2),
        (26.2, 40.2),
        (23.7, 37.9),
        (22.0, 36.8),
        (20.0, 40.0),
        (14.0, 45.5),
        (12.5, 44.0),
        (16.5, 41.2),
        (18.4, 40.1),
        (15.6, 38.0),
        (12.4, 41.9),
        (9.0, 44.4),
        (5.4, 43.3),
        (3.2, 42.4),
        (0.0, 38.8),
        (-5.6, 36.0),
        (-9.3, 38.7),
    ],
    // Africa
    &[
        (-6.0, 35.8),
        (3.0, 36.8),
        (10.5, 37.2),
        (11.5, 33.0),
        (20.0, 32.0),
        (32.5, 31.2),
        (34.8, 28.0),
        (43.2, 12.6),
        (51.3, 11.9),
        (45.3, 2.0),
        (40.5, -3.5),
        (40.5, -11.0),
        (35.0, -20.0),
        (32.8, -26.0),
        (28.0, -33.0),
        (18.4, -34.4),
        (15.0, -26.0),
        (12.0, -16.0),
        (12.0, -6.0),
        (9.2, 4.0),
        (3.0, 6.4),
        (-7.8, 4.4),
        (-13.5, 9.5),
        (-17.5, 14.7),
        (-16.5, 21.0),
        (-10.0, 29.5),
        (-6.0, 35.8),
    ],
    // Madagascar
    &[
        (49.3, -12.0),
        (50.4, -15.5),
        (47.5, -24.5),
        (44.0, -25.2),
        (43.5, -20.0),
        (47.0, -15.0),
        (49.3, -12.0),
    ],
    // Asia (from Suez/Anatolia to East Asia & Siberia)
    &[
        (26.2, 40.0),
        (36.0, 42.0),
        (41.5, 41.5),
        (47.0, 46.5),
        (60.0, 52.0),
        (60.0, 69.5),
        (73.0, 72.5),
        (104.0, 77.5),
        (128.0, 73.0),
        (160.0, 70.0),
        (179.0, 66.5),
        (170.0, 60.5),
        (162.0, 59.5),
        (156.8, 51.0),
        (161.5, 55.5),
        (155.0, 59.5),
        (141.0, 53.5),
        (135.0, 44.0),
        (129.0, 40.5),
        (129.2, 35.1),
        (126.5, 35.0),
        (124.5, 40.0),
        (119.5, 39.8),
        (122.0, 37.5),
        (120.5, 35.5),
        (121.9, 31.2),
        (120.5, 26.5),
        (114.2, 22.3),
        (108.5, 21.5),
        (109.2, 12.2),
        (104.5, 8.6),
        (100.5, 13.5),
        (103.8, 1.3),
        (100.0, 5.5),
        (98.5, 10.0),
        (96.5, 16.5),
        (92.2, 21.0),
        (89.0, 22.0),
        (85.0, 19.5),
        (80.3, 13.0),
        (77.5, 8.1),
        (73.0, 19.0),
        (69.0, 22.5),
        (67.0, 24.8),
        (56.5, 25.5),
        (49.5, 29.5),
        (56.0, 24.0),
        (59.8, 22.5),
        (54.0, 17.0),
        (43.5, 12.6),
        (39.0, 21.5),
        (34.8, 28.0),
        (32.5, 31.2),
        (36.0, 36.8),
        (27.0, 37.0),
        (26.2, 40.0),
    ],
    // Japan
    &[
        (130.5, 31.5),
        (131.0, 34.0),
        (136.0, 35.5),
        (140.8, 35.7),
        (141.5, 41.2),
        (145.5, 43.3),
        (141.8, 45.4),
        (139.8, 40.5),
        (136.5, 37.2),
        (131.0, 34.8),
        (129.8, 32.8),
        (130.5, 31.5),
    ],
    // Sri Lanka
    &[(79.8, 9.6), (81.8, 7.6), (80.5, 6.0), (79.8, 7.5), (79.8, 9.6)],
    // Taiwan
    &[
        (121.5, 25.1),
        (121.6, 22.5),
        (120.8, 21.9),
        (120.1, 23.8),
        (121.5, 25.1),
    ],
    // Sumatra
    &[
        (95.3, 5.5),
        (99.8, 3.0),
        (105.8, -5.8),
        (102.0, -4.0),
        (96.5, 2.0),
        (95.3, 5.5),
    ],
    // Java
    &[
        (105.5, -6.2),
        (114.4, -7.8),
        (114.0, -8.6),
        (106.0, -7.2),
        (105.5, -6.2),
    ],
    // Borneo
    &[
        (109.0, 1.5),
        (117.5, 6.8),
        (118.8, 4.2),
        (116.5, -3.5),
        (110.2, -3.0),
        (109.0, 1.5),
    ],
    // Philippines (Luzon + Mindanao simplified)
    &[
        (120.0, 18.4),
        (122.4, 17.5),
        (123.8, 13.0),
        (120.4, 13.8),
        (120.0, 18.4),
    ],
    &[(125.5, 9.8), (126.5, 7.0), (124.2, 5.7), (122.0, 6.9), (125.5, 9.8)],
    // New Guinea
    &[
        (131.0, -1.5),
        (141.0, -2.5),
        (150.5, -10.3),
        (143.0, -8.5),
        (136.0, -4.8),
        (131.0, -1.5),
    ],
    // Australia
    &[
        (113.5, -22.0),
        (122.2, -17.9),
        (130.8, -12.4),
        (136.8, -12.3),
        (140.8, -17.5),
        (142.5, -10.7),
        (146.0, -17.5),
        (153.5, -28.0),
        (150.0, -37.5),
        (144.8, -38.4),
        (138.5, -35.0),
        (135.5, -34.8),
        (129.0, -31.8),
        (121.9, -33.9),
        (115.1, -34.3),
        (115.8, -31.9),
        (113.5, -22.0),
    ],
    // Tasmania
    &[
        (144.7, -40.8),
        (148.3, -41.0),
        (147.8, -43.2),
        (145.2, -43.0),
        (144.7, -40.8),
    ],
    // New Zealand
    &[
        (172.7, -34.4),
        (178.5, -37.7),
        (175.3, -41.4),
        (172.5, -40.5),
        (172.7, -34.4),
    ],
    &[
        (173.8, -41.2),
        (172.5, -43.9),
        (168.4, -46.6),
        (166.5, -45.8),
        (171.2, -42.2),
        (173.8, -41.2),
    ],
    // Antarctica strip
    &[
        (-179.9, -71.0),
        (-130.0, -73.0),
        (-85.0, -73.0),
        (-60.0, -63.5),
        (-55.0, -65.0),
        (-35.0, -76.0),
        (0.0, -70.0),
        (50.0, -67.5),
        (100.0, -66.5),
        (150.0, -68.5),
        (179.9, -72.0),
        (179.9, -84.5),
        (-179.9, -84.5),
        (-179.9, -71.0),
    ],
];

/// A single photo or video carrying EXIF GPS coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeoPhoto {
    pub path: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub taken: i64,
}

/// Normalized Web Mercator viewport camera (`cx`, `cy` in `[0, 1]`, `zoom >= 1.0`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapCamera {
    pub cx: f64,
    pub cy: f64,
    pub zoom: f64,
}

impl Default for MapCamera {
    fn default() -> Self {
        Self {
            cx: 0.5,
            cy: 0.44,
            zoom: 1.0,
        }
    }
}

/// A screen-space cluster of 1 or more nearby [`GeoPhoto`] items.
#[derive(Clone, Debug, PartialEq)]
pub struct PinCluster {
    pub sx: f32,
    pub sy: f32,
    pub lat: f64,
    pub lon: f64,
    pub indices: Vec<usize>,
}

/// Converts `(lat_deg, lon_deg)` to normalized Web Mercator world coordinates `(wx, wy)` in `[0, 1]`.
#[inline]
pub fn project(lat_deg: f64, lon_deg: f64) -> (f64, f64) {
    let lon = lon_deg.clamp(-180.0, 180.0);
    let lat = lat_deg.clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT);
    let wx = (lon + 180.0) / 360.0;
    let sin_lat = lat.to_radians().sin();
    let wy = 0.5 - ((1.0 + sin_lat) / (1.0 - sin_lat)).ln() / (4.0 * std::f64::consts::PI);
    (wx.clamp(0.0, 1.0), wy.clamp(0.0, 1.0))
}

impl MapCamera {
    #[inline]
    fn scale(&self, width: f32) -> f64 {
        self.zoom * f64::from(width.max(1.0))
    }

    #[inline]
    fn clamp(&mut self) {
        self.zoom = self.zoom.clamp(1.0, 4096.0);
        self.cx = self.cx.clamp(0.0, 1.0);
        self.cy = self.cy.clamp(0.05, 0.95);
    }

    /// Maps normalized world `(wx, wy)` to viewport-local `(sx, sy)` pixels.
    #[inline]
    pub fn to_screen(&self, wx: f64, wy: f64, width: f32, height: f32) -> (f32, f32) {
        let s = self.scale(width);
        let sx = ((wx - self.cx) * s) as f32 + width * 0.5;
        let sy = ((wy - self.cy) * s) as f32 + height * 0.5;
        (sx, sy)
    }

    /// Maps viewport-local `(sx, sy)` pixels back to normalized world `(wx, wy)`.
    #[inline]
    pub fn screen_to_world(&self, sx: f32, sy: f32, width: f32, height: f32) -> (f64, f64) {
        let s = self.scale(width);
        let wx = self.cx + f64::from(sx - width * 0.5) / s;
        let wy = self.cy + f64::from(sy - height * 0.5) / s;
        (wx, wy)
    }

    /// Translates the camera when dragging the viewport by `(dx_px, dy_px)`.
    pub fn pan_by(&mut self, dx_px: f32, dy_px: f32, width: f32) {
        let s = self.scale(width);
        self.cx -= f64::from(dx_px) / s;
        self.cy -= f64::from(dy_px) / s;
        self.clamp();
    }

    /// Zooms by `factor` while keeping the world point under cursor `(sx, sy)` anchored.
    pub fn zoom_at(&mut self, factor: f64, sx: f32, sy: f32, width: f32, height: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let (wx, wy) = self.screen_to_world(sx, sy, width, height);
        self.zoom = (self.zoom * factor).clamp(1.0, 4096.0);
        let s = self.scale(width);
        self.cx = wx - f64::from(sx - width * 0.5) / s;
        self.cy = wy - f64::from(sy - height * 0.5) / s;
        self.clamp();
    }

    /// Computes a camera framed neatly around `(lat, lon)` photo locations.
    pub fn fit_points(points: &[(f64, f64)], aspect_wh: f64) -> Self {
        if points.is_empty() {
            return Self::default();
        }
        let mut min_x = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for &(lat, lon) in points {
            let (wx, wy) = project(lat, lon);
            min_x = min_x.min(wx);
            max_x = max_x.max(wx);
            min_y = min_y.min(wy);
            max_y = max_y.max(wy);
        }
        let cx = (min_x + max_x) * 0.5;
        let cy = (min_y + max_y) * 0.5;
        let span_x = (max_x - min_x).max(0.002);
        let span_y = (max_y - min_y).max(0.002);
        let aspect = aspect_wh.clamp(0.3, 4.0);
        let zoom_x = 0.72 / span_x;
        let zoom_y = 0.72 / (span_y * aspect);
        let mut cam = Self {
            cx,
            cy,
            zoom: zoom_x.min(zoom_y).clamp(1.0, 256.0),
        };
        cam.clamp();
        cam
    }
}

/// Groups geotagged photos visible within `[0..width, 0..height]` into `cell_px` grid clusters in O(N).
pub fn cluster_pins(photos: &[GeoPhoto], cam: &MapCamera, width: f32, height: f32, cell_px: f32) -> Vec<PinCluster> {
    type ClusterBucket = Vec<(usize, f32, f32)>;

    if photos.is_empty() || width <= 1.0 || height <= 1.0 {
        return Vec::new();
    }
    let cell = cell_px.max(8.0);
    let pad = cell;
    let mut buckets: HashMap<(i32, i32), ClusterBucket> = HashMap::new();

    for (idx, photo) in photos.iter().enumerate() {
        let (wx, wy) = project(photo.lat, photo.lon);
        let (sx, sy) = cam.to_screen(wx, wy, width, height);
        if sx < -pad || sy < -pad || sx > width + pad || sy > height + pad {
            continue;
        }
        let gx = (sx / cell).floor() as i32;
        let gy = (sy / cell).floor() as i32;
        buckets.entry((gx, gy)).or_default().push((idx, sx, sy));
    }

    let mut cells: Vec<((i32, i32), ClusterBucket)> = buckets.into_iter().collect();
    cells.sort_by_key(|(k, _)| (k.1, k.0));

    cells
        .into_iter()
        .map(|(_, members)| {
            let n = members.len() as f64;
            let mut sum_sx = 0.0f64;
            let mut sum_sy = 0.0f64;
            let mut sum_lat = 0.0f64;
            let mut sum_lon = 0.0f64;
            let mut indices = Vec::with_capacity(members.len());
            for (idx, sx, sy) in members {
                sum_sx += f64::from(sx);
                sum_sy += f64::from(sy);
                sum_lat += photos[idx].lat;
                sum_lon += photos[idx].lon;
                indices.push(idx);
            }
            PinCluster {
                sx: (sum_sx / n) as f32,
                sy: (sum_sy / n) as f32,
                lat: sum_lat / n,
                lon: sum_lon / n,
                indices,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mercator_projection_and_screen_inverse_round_trip() {
        let (wx, wy) = project(0.0, 0.0);
        assert!((wx - 0.5).abs() < 1e-9);
        assert!((wy - 0.5).abs() < 1e-9);

        let cam = MapCamera {
            cx: 0.55,
            cy: 0.35,
            zoom: 4.5,
        };
        let (twx, twy) = project(35.6762, 139.6503);
        let (sx, sy) = cam.to_screen(twx, twy, 1200.0, 700.0);
        let (rx, ry) = cam.screen_to_world(sx, sy, 1200.0, 700.0);
        assert!((rx - twx).abs() < 1e-6);
        assert!((ry - twy).abs() < 1e-6);
    }

    #[test]
    fn zoom_at_keeps_cursor_world_coordinates_stationary() {
        let mut cam = MapCamera::default();
        let (cursor_x, cursor_y) = (780.0, 240.0);
        let (wx_before, wy_before) = cam.screen_to_world(cursor_x, cursor_y, 1000.0, 600.0);
        cam.zoom_at(2.5, cursor_x, cursor_y, 1000.0, 600.0);
        let (wx_after, wy_after) = cam.screen_to_world(cursor_x, cursor_y, 1000.0, 600.0);
        assert!((wx_before - wx_after).abs() < 1e-5);
        assert!((wy_before - wy_after).abs() < 1e-5);
    }

    #[test]
    fn cluster_pins_merges_at_low_zoom_and_splits_at_high_zoom() {
        let photos = vec![
            GeoPhoto {
                path: "a.jpg".into(),
                name: "a.jpg".into(),
                lat: 48.8566,
                lon: 2.3522,
                taken: 1,
            },
            GeoPhoto {
                path: "b.jpg".into(),
                name: "b.jpg".into(),
                lat: 48.8600,
                lon: 2.3600,
                taken: 2,
            },
            GeoPhoto {
                path: "c.jpg".into(),
                name: "c.jpg".into(),
                lat: 35.6762,
                lon: 139.6503,
                taken: 3,
            },
        ];

        let low = MapCamera::default();
        let clusters_low = cluster_pins(&photos, &low, 1000.0, 600.0, 40.0);
        assert_eq!(clusters_low.len(), 2, "Paris pair merges vs Tokyo");

        let (px, py) = project(48.858, 2.356);
        let high = MapCamera {
            cx: px,
            cy: py,
            zoom: 1024.0,
        };
        let clusters_high = cluster_pins(&photos, &high, 1000.0, 600.0, 20.0);
        assert_eq!(clusters_high.len(), 2, "Paris pair splits and Tokyo is off-screen");
    }
}
