//! The pictures in the map: which photos' thumbnails are asked of the thumbnail service (the lead photo of the
//! biggest pins, and the photos of the open drawer, at one small size, only once the camera has rested) and what
//! the service has so far.

use super::drawer::{Item, TILE};
use super::frame::Frame;
use super::pins::PinPicture;
use super::*;
use crate::components::file_grid::thumbnail::{Look, Want};
use glint_model::entries::{with_details, FileEntry, RawEntry};
use std::hash::{DefaultHasher, Hash, Hasher};

/// The square the service is asked for, in CSS pixels: a pin's circle and a drawer's tile are both cut from it.
pub(super) const PICTURE_BOX: f32 = TILE;

/// How long the camera must rest before pictures are asked for: panning and flying decode nothing.
const SETTLE: Duration = Duration::from_millis(130);

/// The most photos the drawer shows.
pub(super) const DRAWER_MOST: usize = 24;

/// The most entries kept for the photos of a set: a camera that has been everywhere doesn't keep them all.
const KEPT: usize = 512;

/// A photo as the thumbnail service knows a file: made once and kept while it is shown, so asking for a
/// picture in a frame allocates nothing.
#[derive(Default)]
pub(super) struct Leads {
    set: usize,
    entries: HashMap<usize, FileEntry>,
}

impl Leads {
    /// The entry of photo `idx` of `set`.
    pub(super) fn entry(&mut self, set: &GeoSet, idx: usize) -> Option<&FileEntry> {
        if self.set != set.id() || self.entries.len() > KEPT {
            self.entries.clear();
            self.set = set.id();
        }
        let photo = set.get(idx)?;
        Some(self.entries.entry(idx).or_insert_with(|| {
            with_details(
                RawEntry {
                    name: photo.name.to_string(),
                    path: Some(photo.path.to_string()),
                    size: photo.size,
                    modified: photo.modified,
                    ..Default::default()
                },
                "",
            )
        }))
    }
}

/// The image a look has, when it has one (a picture of this size, or one of another size standing in).
fn image_of(look: Look) -> Option<(Arc<RenderImage>, f32, f32)> {
    match look {
        Look::Picture(d) | Look::StandIn(d) => Some((d.image.clone(), d.width, d.height)),
        Look::Icon => None,
    }
}

impl PhotoMapView {
    /// The pictures the pins that get one have now (the others are drawn as dots until theirs comes).
    pub(super) fn pin_pictures(&mut self, frame: &Frame, set: &GeoSet, scale: f32, cx: &App) -> Vec<PinPicture> {
        let thumbs = self.thumbs.read(cx);
        let mut out = Vec::with_capacity(frame.top.len());
        for &p in &frame.top {
            let Some(lead) = frame
                .clusters
                .pins
                .get(p as usize)
                .and_then(|pin| frame.clusters.lead_of(pin))
            else {
                continue;
            };
            let Some(entry) = self.leads.entry(set, lead) else {
                continue;
            };
            if let Some((image, width, height)) = image_of(thumbs.look(entry, PICTURE_BOX, scale)) {
                out.push(PinPicture {
                    pin: p,
                    image,
                    width,
                    height,
                });
            }
        }
        out
    }

    /// The photos of an open drawer, with what picture each has.
    pub(super) fn drawer_items(&mut self, members: &[u32], set: &GeoSet, scale: f32, cx: &App) -> Vec<Item> {
        let thumbs = self.thumbs.read(cx);
        let mut items = Vec::with_capacity(members.len().min(DRAWER_MOST));
        for &i in members.iter().take(DRAWER_MOST) {
            let Some(photo) = set.get(i as usize).cloned() else {
                continue;
            };
            let picture = self
                .leads
                .entry(set, i as usize)
                .and_then(|entry| image_of(thumbs.look(entry, PICTURE_BOX, scale)))
                .map(|(image, ..)| image);
            items.push(Item { photo, picture });
        }
        items
    }

    /// Tells the thumbnail service which pictures the map shows, once the camera has rested for a moment (a pan or a
    /// flight asks for nothing), and again only when what is shown changes.
    pub(super) fn ask_for_pictures(
        &mut self,
        frame: &Frame,
        drawer: &[u32],
        set: &GeoSet,
        scale: f32,
        cx: &mut Context<Self>,
    ) {
        if self.seen_camera != self.camera {
            self.seen_camera = self.camera;
            self.moves += 1;
            self.settled = false;
            // One timer for a whole pan (not one a frame): it looks again after each wait until the camera has not
            // moved for a whole one.
            if self.settle.is_none() {
                self.settle = Some(cx.spawn(async move |this, cx| {
                    let Ok(mut at) = this.update(cx, |this, _| this.moves) else {
                        return;
                    };
                    loop {
                        cx.background_executor().timer(SETTLE).await;
                        let rested = this.update(cx, |this, cx| {
                            if this.moves == at {
                                this.settled = true;
                                this.settle = None;
                                cx.notify();
                                None
                            } else {
                                Some(this.moves)
                            }
                        });
                        match rested {
                            Ok(Some(moves)) => at = moves,
                            _ => return,
                        }
                    }
                }));
            }
        }
        if !self.settled {
            return;
        }
        let mut wanted: Vec<usize> = frame
            .top
            .iter()
            .filter_map(|&p| {
                frame
                    .clusters
                    .pins
                    .get(p as usize)
                    .and_then(|pin| frame.clusters.lead_of(pin))
            })
            .collect();
        wanted.extend(drawer.iter().take(DRAWER_MOST).map(|&i| i as usize));
        let mut hasher = DefaultHasher::new();
        (set.id(), scale.to_bits(), &wanted).hash(&mut hasher);
        let key = hasher.finish();
        if key == self.asked {
            return;
        }
        self.asked = key;
        let wants: Vec<Want> = wanted
            .iter()
            .filter_map(|&i| {
                self.leads
                    .entry(set, i)
                    .and_then(|entry| Want::for_entry(entry, PICTURE_BOX, scale))
            })
            .collect();
        let lane = format!("photo-map-{}", self.pane);
        let id = cx.entity_id();
        self.thumbs.update(cx, |thumbs, cx| thumbs.show(id, &lane, wants, cx));
    }
}
