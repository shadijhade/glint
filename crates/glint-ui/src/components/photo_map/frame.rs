//! What one camera's view of the photos costs, paid once: the clusters, kept until the camera, the size of the
//! map or the photos change, and shared with the paint closure as one `Arc`.

use glint_model::world_map::{cluster_set, Clusters, GeoSet, MapCamera};
use std::sync::Arc;

/// How big a cluster's square of screen is, in pixels.
pub(super) const CELL_PX: f32 = 36.0;

/// What a set of clusters was made from.
#[derive(Clone, Copy, PartialEq)]
struct Key {
    set: usize,
    camera: [u64; 3],
    size: [u32; 2],
}

/// The clusters of the last camera asked about.
#[derive(Default)]
pub(super) struct ClusterCache {
    last: Option<(Key, Arc<Clusters>)>,
}

impl ClusterCache {
    /// The clusters of `set` seen by `camera` in a `width` x `height` box: the last ones when nothing changed
    /// (a clone of an `Arc`), else worked out now.
    pub(super) fn get(&mut self, set: &GeoSet, camera: &MapCamera, width: f32, height: f32) -> Arc<Clusters> {
        let key = Key {
            set: set.id(),
            camera: [camera.cx.to_bits(), camera.cy.to_bits(), camera.zoom.to_bits()],
            size: [width.to_bits(), height.to_bits()],
        };
        if let Some((last, clusters)) = &self.last {
            if *last == key {
                return clusters.clone();
            }
        }
        let clusters = Arc::new(cluster_set(set, camera, width, height, CELL_PX));
        self.last = Some((key, clusters.clone()));
        clusters
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_model::world_map::GeoPhoto;
    use std::time::Instant;

    /// `n` photos around sixty places, in the shape of a library: long paths and names.
    pub(in crate::components::photo_map) fn library(n: usize) -> Vec<GeoPhoto> {
        let mut seed = 12345u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as f64) / ((1u64 << 31) as f64)
        };
        let hubs: Vec<(f64, f64)> = (0..60)
            .map(|_| (next() * 120.0 - 50.0, next() * 340.0 - 170.0))
            .collect();
        (0..n)
            .map(|i| {
                let (la, lo) = hubs[i % hubs.len()];
                GeoPhoto {
                    path: format!("C:\\Users\\someone\\Pictures\\Trips\\2024\\IMG_{i:06}.jpg"),
                    name: format!("IMG_{i:06}.jpg"),
                    lat: la + (next() - 0.5) * 2.0,
                    lon: lo + (next() - 0.5) * 2.0,
                    taken: 1_600_000_000_000 + i as i64 * 3_000_000,
                    ..Default::default()
                }
            })
            .collect()
    }

    #[test]
    fn a_camera_that_did_not_move_costs_a_clone_and_one_that_did_costs_one_pass() {
        let set = GeoSet::new(library(2_000));
        let mut cache = ClusterCache::default();
        let cam = MapCamera::default();
        let first = cache.get(&set, &cam, 1200.0, 700.0);
        let again = cache.get(&set, &cam, 1200.0, 700.0);
        assert!(Arc::ptr_eq(&first, &again), "the same camera is not worked out again");
        let moved = cache.get(&set, &MapCamera { cx: 0.51, ..cam }, 1200.0, 700.0);
        assert!(!Arc::ptr_eq(&first, &moved));
        let resized = cache.get(&set, &MapCamera { cx: 0.51, ..cam }, 1000.0, 700.0);
        assert!(!Arc::ptr_eq(&moved, &resized));
        // A clone of the set is the same photos; another set is not.
        let again = cache.get(&set.clone(), &MapCamera { cx: 0.51, ..cam }, 1000.0, 700.0);
        assert!(Arc::ptr_eq(&resized, &again));
        let other = GeoSet::new(library(2_000));
        assert!(!Arc::ptr_eq(
            &resized,
            &cache.get(&other, &MapCamera { cx: 0.51, ..cam }, 1000.0, 700.0)
        ));
    }

    /// A frame's work, printed with `--nocapture`. In the debug test profile, before this change (the view cloned
    /// the photos, strings and all, three times a frame and clustered twice): 3.57 ms a frame for 5,000 photos and
    /// 43.6 ms for 50,000. After: 0.12 ms and 1.13 ms while panning and zooming, nothing while the camera rests.
    #[test]
    fn a_frame_costs_little() {
        for n in [5_000usize, 50_000] {
            let set = GeoSet::new(library(n));
            let mut cache = ClusterCache::default();
            let mut cam = MapCamera::default();
            let frames = 60;
            // Panning and zooming: the camera changes every frame, so each frame clusters once.
            let t0 = Instant::now();
            let mut sink = 0usize;
            for f in 0..frames {
                cam.pan_by(3.0, 1.0, 1200.0);
                if f % 7 == 0 {
                    cam.zoom_at(1.05, 600.0, 300.0, 1200.0, 700.0);
                }
                let clusters = cache.get(&set, &cam, 1200.0, 700.0);
                sink += clusters.pins.len();
            }
            let panning = t0.elapsed().as_secs_f64() * 1000.0 / f64::from(frames);
            // Everything else that redraws the map (a hover, a notice): the same camera.
            let t0 = Instant::now();
            for _ in 0..frames {
                sink += cache.get(&set, &cam, 1200.0, 700.0).pins.len();
            }
            let resting = t0.elapsed().as_secs_f64() * 1000.0 / f64::from(frames);
            println!("AFTER n={n}: panning {panning:.3} ms a frame, resting {resting:.4} ms a frame (sink {sink})");
            assert!(sink > 0);
        }
    }
}
