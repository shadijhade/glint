//! Offline 120 FPS Web Mercator vector world map projection and O(N) screen-space pin clustering.
//!
//! Zero network tiles, zero AI: embedded simplified continent and major island polygons paired with
//! deterministic Web Mercator camera math and screen-space cluster bucketing.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GeoPhoto {
    pub path: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    /// When it was taken, ms since 1970 (when it was modified, if it doesn't say).
    pub taken: i64,
}

/// The photos a map shows, and where each lies on the world map: worked out once when the set is made, so
/// clustering a camera's view never calls `ln` or `sin` again, and cloning the set costs two reference counts.
#[derive(Clone, Debug)]
pub struct GeoSet {
    photos: Arc<[GeoPhoto]>,
    world: Arc<[(f64, f64)]>,
}

impl Default for GeoSet {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl std::ops::Deref for GeoSet {
    type Target = [GeoPhoto];

    fn deref(&self) -> &[GeoPhoto] {
        &self.photos
    }
}

impl GeoSet {
    /// A set of `photos`, in the order given.
    pub fn new(photos: Vec<GeoPhoto>) -> Self {
        let world: Vec<(f64, f64)> = photos.iter().map(|p| project(p.lat, p.lon)).collect();
        Self {
            photos: photos.into(),
            world: world.into(),
        }
    }

    /// Who this set is: two clones of a set have the same id, two sets made apart never do (while both live).
    pub fn id(&self) -> usize {
        self.photos.as_ptr() as usize ^ self.photos.len().rotate_left(48)
    }

    /// The earliest and the latest time anything in the set was taken.
    pub fn time_range(&self) -> Option<(i64, i64)> {
        let first = self.photos.first()?.taken;
        Some(
            self.photos
                .iter()
                .fold((first, first), |(lo, hi), p| (lo.min(p.taken), hi.max(p.taken))),
        )
    }

    /// The photos taken from `from` to `to` (both included), in the same order, as a set of their own.
    pub fn taken_between(&self, from: i64, to: i64) -> Self {
        let mut photos = Vec::new();
        let mut world = Vec::new();
        for (p, w) in self.photos.iter().zip(self.world.iter()) {
            if p.taken >= from && p.taken <= to {
                photos.push(p.clone());
                world.push(*w);
            }
        }
        Self {
            photos: photos.into(),
            world: world.into(),
        }
    }

    /// The world coordinates `(wx, wy)` of photo `i`.
    pub fn world_of(&self, i: usize) -> Option<(f64, f64)> {
        self.world.get(i).copied()
    }
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

/// A screen-space cluster of one or more nearby photos: where it is drawn, where its photos are on the globe, and
/// which they are (a range of [`Clusters::members_of`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PinCluster {
    pub sx: f32,
    pub sy: f32,
    pub lat: f64,
    pub lon: f64,
    /// Where its photos start in [`Clusters::members`], and how many there are.
    pub first: u32,
    pub count: u32,
}

/// One camera's view of a set: its pins, and the photos in each (indices into the set). Built once per camera,
/// shared by the render and the paint closure as one `Arc`.
#[derive(Debug, Default, PartialEq)]
pub struct Clusters {
    pub pins: Vec<PinCluster>,
    members: Vec<u32>,
}

impl Clusters {
    /// The photos in `pin`: indices into the set, in the set's order (so the first is the set's first of them).
    pub fn members_of(&self, pin: &PinCluster) -> &[u32] {
        let from = (pin.first as usize).min(self.members.len());
        let to = (from + pin.count as usize).min(self.members.len());
        &self.members[from..to]
    }

    /// The photo that stands for `pin`: the first of its members.
    pub fn lead_of(&self, pin: &PinCluster) -> Option<usize> {
        self.members.get(pin.first as usize).map(|&i| i as usize)
    }
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
        let mut bounds = WorldBounds::default();
        for &(lat, lon) in points {
            let (wx, wy) = project(lat, lon);
            bounds.add(wx, wy);
        }
        Self::fit_bounds(&bounds, aspect_wh)
    }

    /// A camera framed around all of a set.
    pub fn fit_set(set: &GeoSet, aspect_wh: f64) -> Self {
        let mut bounds = WorldBounds::default();
        for &(wx, wy) in set.world.iter() {
            bounds.add(wx, wy);
        }
        Self::fit_bounds(&bounds, aspect_wh)
    }

    /// A camera framed neatly around `bounds`; the default one when nothing was added to them.
    pub fn fit_bounds(bounds: &WorldBounds, aspect_wh: f64) -> Self {
        if bounds.is_empty() {
            return Self::default();
        }
        let cx = (bounds.min_x + bounds.max_x) * 0.5;
        let cy = (bounds.min_y + bounds.max_y) * 0.5;
        let span_x = (bounds.max_x - bounds.min_x).max(0.002);
        let span_y = (bounds.max_y - bounds.min_y).max(0.002);
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

/// The corners of a box on the world map.
#[derive(Clone, Copy, Debug)]
pub struct WorldBounds {
    pub min_x: f64,
    pub max_x: f64,
    pub min_y: f64,
    pub max_y: f64,
}

impl Default for WorldBounds {
    fn default() -> Self {
        Self {
            min_x: f64::INFINITY,
            max_x: f64::NEG_INFINITY,
            min_y: f64::INFINITY,
            max_y: f64::NEG_INFINITY,
        }
    }
}

impl WorldBounds {
    /// Grows the box to hold the point.
    pub fn add(&mut self, wx: f64, wy: f64) {
        self.min_x = self.min_x.min(wx);
        self.max_x = self.max_x.max(wx);
        self.min_y = self.min_y.min(wy);
        self.max_y = self.max_y.max(wy);
    }

    /// Nothing was added.
    pub fn is_empty(&self) -> bool {
        self.min_x > self.max_x
    }
}

thread_local! {
    /// What [`cluster_set`] counts into: sized to the largest view asked for, kept between calls, so a frame
    /// allocates nothing for its working.
    static SCRATCH: std::cell::RefCell<Scratch> = std::cell::RefCell::new(Scratch::default());
}

#[derive(Default)]
struct Scratch {
    /// The cell each photo falls in (`NO_CELL` when it is off the view).
    cells: Vec<u32>,
    /// Per cell: how many photos, then (after the prefix sum) where its members start.
    counts: Vec<u32>,
    sums: Vec<[f64; 4]>,
}

const NO_CELL: u32 = u32::MAX;

/// Groups the photos visible within `[0..width, 0..height]` into `cell_px` squares of screen, in two passes over
/// the set and one over the cells (a counting sort: no comparing, no hashing). Pins come out row by row, left to
/// right; a pin's photos keep the set's order.
pub fn cluster_set(set: &GeoSet, cam: &MapCamera, width: f32, height: f32, cell_px: f32) -> Clusters {
    if set.is_empty() || width <= 1.0 || height <= 1.0 {
        return Clusters::default();
    }
    let cell = cell_px.max(8.0);
    let pad = cell;
    // Cells cover -pad..width+pad: one more than the view's cells on each side.
    let cols = (width / cell).ceil() as i32 + 3;
    let rows = (height / cell).ceil() as i32 + 3;
    let total = (cols * rows) as usize;
    let scale = cam.zoom * f64::from(width.max(1.0));
    let (half_w, half_h) = (width * 0.5, height * 0.5);

    SCRATCH.with_borrow_mut(|scratch| {
        let Scratch { cells, counts, sums } = scratch;
        cells.clear();
        cells.resize(set.len(), NO_CELL);
        counts.clear();
        counts.resize(total + 1, 0);
        sums.clear();
        sums.resize(total, [0.0; 4]);
        let mut visible = 0u32;
        for (i, &(wx, wy)) in set.world.iter().enumerate() {
            let sx = ((wx - cam.cx) * scale) as f32 + half_w;
            let sy = ((wy - cam.cy) * scale) as f32 + half_h;
            if sx < -pad || sy < -pad || sx > width + pad || sy > height + pad {
                continue;
            }
            let gx = (sx / cell).floor() as i32 + 1;
            let gy = (sy / cell).floor() as i32 + 1;
            if gx < 0 || gy < 0 || gx >= cols || gy >= rows {
                continue;
            }
            let c = (gy * cols + gx) as usize;
            cells[i] = c as u32;
            counts[c] += 1;
            let s = &mut sums[c];
            s[0] += f64::from(sx);
            s[1] += f64::from(sy);
            s[2] += set.photos[i].lat;
            s[3] += set.photos[i].lon;
            visible += 1;
        }
        // The pins, and where each cell's members will start.
        let mut pins = Vec::new();
        let mut at = 0u32;
        for c in 0..total {
            let n = counts[c];
            counts[c] = at;
            if n > 0 {
                let s = &sums[c];
                let nf = f64::from(n);
                pins.push(PinCluster {
                    sx: (s[0] / nf) as f32,
                    sy: (s[1] / nf) as f32,
                    lat: s[2] / nf,
                    lon: s[3] / nf,
                    first: at,
                    count: n,
                });
            }
            at += n;
        }
        // Members, in the set's order within each cell (`counts[c]` walks from the cell's start to its end).
        let mut members = vec![0u32; visible as usize];
        for (i, &c) in cells.iter().enumerate() {
            if c != NO_CELL {
                let slot = &mut counts[c as usize];
                members[*slot as usize] = i as u32;
                *slot += 1;
            }
        }
        Clusters { pins, members }
    })
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

    fn photo(path: &str, lat: f64, lon: f64, taken: i64) -> GeoPhoto {
        GeoPhoto {
            path: path.into(),
            name: path.into(),
            lat,
            lon,
            taken,
        }
    }

    #[test]
    fn cluster_set_merges_at_low_zoom_and_splits_at_high_zoom() {
        let set = GeoSet::new(vec![
            photo("a.jpg", 48.8566, 2.3522, 1),
            photo("b.jpg", 48.8600, 2.3600, 2),
            photo("c.jpg", 35.6762, 139.6503, 3),
        ]);

        let low = MapCamera::default();
        let clusters_low = cluster_set(&set, &low, 1000.0, 600.0, 40.0);
        assert_eq!(clusters_low.pins.len(), 2, "Paris pair merges vs Tokyo");
        let paris = clusters_low.pins.iter().find(|p| p.count == 2).expect("the Paris pin");
        assert_eq!(
            clusters_low.members_of(paris),
            &[0, 1],
            "a pin's photos keep the set's order"
        );
        assert_eq!(clusters_low.lead_of(paris), Some(0));
        assert!((paris.lat - 48.8583).abs() < 1e-3 && (paris.lon - 2.3561).abs() < 1e-3);

        let (px, py) = project(48.858, 2.356);
        let high = MapCamera {
            cx: px,
            cy: py,
            zoom: 1024.0,
        };
        let clusters_high = cluster_set(&set, &high, 1000.0, 600.0, 20.0);
        assert_eq!(clusters_high.pins.len(), 2, "Paris pair splits and Tokyo is off-screen");
        assert!(clusters_high.pins.iter().all(|p| p.count == 1));
    }

    #[test]
    fn cluster_set_agrees_with_a_plain_grouping_of_every_photo() {
        // 3,000 photos on a spiral; every photo on screen is in exactly one pin, off-screen ones in none.
        let photos: Vec<GeoPhoto> = (0..3_000)
            .map(|i| {
                let a = f64::from(i) * 0.37;
                photo(
                    "x",
                    40.0 + a.sin() * 30.0,
                    10.0 + a.cos() * 60.0 + f64::from(i) * 0.01,
                    i64::from(i),
                )
            })
            .collect();
        let set = GeoSet::new(photos);
        let cam = MapCamera {
            cx: 0.55,
            cy: 0.35,
            zoom: 3.0,
        };
        let (w, h, cell) = (900.0f32, 500.0f32, 36.0f32);
        let clusters = cluster_set(&set, &cam, w, h, cell);
        let mut seen = vec![0u32; set.len()];
        for pin in &clusters.pins {
            assert_eq!(clusters.members_of(pin).len(), pin.count as usize);
            let mut last = None;
            for &i in clusters.members_of(pin) {
                seen[i as usize] += 1;
                assert!(last.is_none_or(|l| l < i), "in the set's order");
                last = Some(i);
            }
        }
        for (i, &(wx, wy)) in set.world.iter().enumerate() {
            let (sx, sy) = cam.to_screen(wx, wy, w, h);
            let on = !(sx < -cell || sy < -cell || sx > w + cell || sy > h + cell);
            assert_eq!(seen[i], u32::from(on), "photo {i} at {sx},{sy}");
        }
        // Pins come row by row.
        let rows: Vec<i32> = clusters.pins.iter().map(|p| (p.sy / cell).floor() as i32).collect();
        assert!(rows.windows(2).all(|r| r[0] <= r[1] + 1));
        // Nothing to show: nothing, whatever the box.
        assert!(cluster_set(&GeoSet::default(), &cam, w, h, cell).pins.is_empty());
        assert!(cluster_set(&set, &cam, 1.0, 1.0, cell).pins.is_empty());
    }

    #[test]
    fn a_set_is_the_same_photos_when_cloned_and_others_when_made_again() {
        let a = GeoSet::new(vec![photo("a", 1.0, 2.0, 5), photo("b", 3.0, 4.0, 9)]);
        assert_eq!(a.id(), a.clone().id());
        assert_ne!(
            a.id(),
            GeoSet::new(vec![photo("a", 1.0, 2.0, 5), photo("b", 3.0, 4.0, 9)]).id()
        );
        assert_eq!(a.time_range(), Some((5, 9)));
        assert_eq!(GeoSet::default().time_range(), None);
        let only_b = a.taken_between(6, 10);
        assert_eq!((only_b.len(), only_b[0].path.as_str()), (1, "b"));
        assert_eq!(only_b.world_of(0), a.world_of(1));
    }

    #[test]
    fn fitting_a_set_frames_all_of_it() {
        let set = GeoSet::new(vec![photo("a", 48.8, 2.3, 1), photo("b", 35.7, 139.6, 2)]);
        let cam = MapCamera::fit_set(&set, 1.6);
        for i in 0..set.len() {
            let (wx, wy) = set.world_of(i).expect("a photo");
            let (sx, sy) = cam.to_screen(wx, wy, 800.0, 500.0);
            assert!((0.0..=800.0).contains(&sx) && (0.0..=500.0).contains(&sy), "{sx},{sy}");
        }
        assert_eq!(MapCamera::fit_set(&GeoSet::default(), 1.6), MapCamera::default());
        assert_eq!(MapCamera::fit_points(&[(48.8, 2.3), (35.7, 139.6)], 1.6), cam);
    }
}
