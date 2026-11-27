// todo("windows"): remove
#![cfg_attr(windows, allow(dead_code))]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AtlasTextureId, AtlasTile, Background, Bounds, ContentMask, Corners, Edges, Hsla, Pixels,
    Point, Radians, ScaledPixels, Size, bounds_tree::BoundsTree, point,
};
use std::{
    fmt::Debug,
    iter::Peekable,
    ops::{Add, Range, Sub},
    slice,
};

#[allow(non_camel_case_types, unused)]
#[expect(missing_docs)]
pub type PathVertex_ScaledPixels = PathVertex<ScaledPixels>;

#[expect(missing_docs)]
pub type DrawOrder = u32;

/// A boolean stored as a `u32` so that GPU-facing structs contain no
/// compiler-inserted padding bytes, which would be undefined behavior to
/// reinterpret as `&[u8]` when writing instance buffers. Guaranteed to be
/// `0` or `1` by construction; shaders read it as a `u32`/`uint`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct PaddedBool32(u32);

impl From<bool> for PaddedBool32 {
    fn from(value: bool) -> Self {
        PaddedBool32(value as u32)
    }
}

#[derive(Default)]
#[expect(missing_docs)]
pub struct Scene {
    pub(crate) paint_operations: Vec<PaintOperation>,
    primitive_bounds: BoundsTree<ScaledPixels>,
    layer_stack: Vec<DrawOrder>,
    /// Glint: the paint transforms in force, innermost last, each composed with those around it.
    transforms: Vec<SceneTransform>,
    pub shadows: Vec<Shadow>,
    pub quads: Vec<Quad>,
    pub paths: Vec<Path<ScaledPixels>>,
    pub underlines: Vec<Underline>,
    pub monochrome_sprites: Vec<MonochromeSprite>,
    pub subpixel_sprites: Vec<SubpixelSprite>,
    pub polychrome_sprites: Vec<PolychromeSprite>,
    pub surfaces: Vec<PaintSurface>,
}

#[expect(missing_docs)]
impl Scene {
    pub fn clear(&mut self) {
        self.paint_operations.clear();
        self.primitive_bounds.clear();
        self.layer_stack.clear();
        self.transforms.clear();
        self.paths.clear();
        self.shadows.clear();
        self.quads.clear();
        self.underlines.clear();
        self.monochrome_sprites.clear();
        self.subpixel_sprites.clear();
        self.polychrome_sprites.clear();
        self.surfaces.clear();
    }

    pub fn len(&self) -> usize {
        self.paint_operations.len()
    }

    pub fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        let bounds = match self.transforms.last() {
            Some(transform) => transform.bounds(bounds).intersect(&transform.clip),
            None => bounds,
        };
        self.push_layer_as_is(bounds);
    }

    /// A layer whose bounds are already where they are drawn (a replay of what an earlier frame painted).
    fn push_layer_as_is(&mut self, bounds: Bounds<ScaledPixels>) {
        let order = self.primitive_bounds.insert(bounds);
        self.layer_stack.push(order);
        self.paint_operations
            .push(PaintOperation::StartLayer(bounds));
    }

    /// Glint: everything inserted until the matching [`Scene::pop_transform`] is drawn `scale` times larger and
    /// moved by `translate` (device pixels: a point `p` lands on `p * scale + translate`), inside any transform
    /// already in force. `clip` is the content mask where the transform begins, in the coordinates of what is
    /// around it: nothing transformed is drawn outside it, however far the transform moves it.
    pub fn push_transform(
        &mut self,
        scale: f32,
        translate: Point<ScaledPixels>,
        clip: Bounds<ScaledPixels>,
    ) {
        let mut transform = SceneTransform {
            scale,
            translate,
            clip,
        };
        if let Some(outer) = self.transforms.last() {
            transform = SceneTransform {
                scale: outer.scale * scale,
                translate: outer.point(translate),
                clip: outer.bounds(clip).intersect(&outer.clip),
            };
        }
        self.transforms.push(transform);
    }

    /// Ends the transform begun by the last [`Scene::push_transform`].
    pub fn pop_transform(&mut self) {
        self.transforms.pop();
    }

    /// Whether a transform is in force.
    pub fn is_transformed(&self) -> bool {
        !self.transforms.is_empty()
    }

    pub fn pop_layer(&mut self) {
        self.layer_stack.pop();
        self.paint_operations.push(PaintOperation::EndLayer);
    }

    pub fn insert_primitive(&mut self, primitive: impl Into<Primitive>) {
        let mut primitive = primitive.into();
        if let Some(transform) = self.transforms.last() {
            primitive.transform(transform);
        }
        self.insert_primitive_as_is(primitive);
    }

    /// A primitive that is already where it is drawn (a replay of what an earlier frame painted).
    fn insert_primitive_as_is(&mut self, mut primitive: Primitive) {
        let clipped_bounds = primitive
            .bounds()
            .intersect(&primitive.content_mask().bounds);

        if clipped_bounds.is_empty() {
            return;
        }

        let order = self
            .layer_stack
            .last()
            .copied()
            .unwrap_or_else(|| self.primitive_bounds.insert(clipped_bounds));
        match &mut primitive {
            Primitive::Shadow(shadow) => {
                shadow.order = order;
                self.shadows.push(*shadow);
            }
            Primitive::Quad(quad) => {
                quad.order = order;
                self.quads.push(*quad);
            }
            Primitive::Path(path) => {
                path.order = order;
                path.id = PathId(self.paths.len());
                self.paths.push(path.clone());
            }
            Primitive::Underline(underline) => {
                underline.order = order;
                self.underlines.push(*underline);
            }
            Primitive::MonochromeSprite(sprite) => {
                sprite.order = order;
                self.monochrome_sprites.push(*sprite);
            }
            Primitive::SubpixelSprite(sprite) => {
                sprite.order = order;
                self.subpixel_sprites.push(*sprite);
            }
            Primitive::PolychromeSprite(sprite) => {
                sprite.order = order;
                self.polychrome_sprites.push(*sprite);
            }
            Primitive::Surface(surface) => {
                surface.order = order;
                self.surfaces.push(surface.clone());
            }
        }
        self.paint_operations
            .push(PaintOperation::Primitive(primitive));
    }

    pub fn replay(&mut self, range: Range<usize>, prev_scene: &Scene) {
        for operation in &prev_scene.paint_operations[range] {
            match operation {
                // What a cached view painted is kept as it was drawn, transform and all: a view under a
                // transform that changed is painted again instead (its cache key holds the transform).
                PaintOperation::Primitive(primitive) => {
                    self.insert_primitive_as_is(primitive.clone())
                }
                PaintOperation::StartLayer(bounds) => self.push_layer_as_is(*bounds),
                PaintOperation::EndLayer => self.pop_layer(),
            }
        }
    }

    /// Glint: a copy of what the operations in `range` painted, to paint again later with
    /// [`Scene::paint_snapshot`]. None when the range is not in this scene.
    pub fn snapshot(&self, range: Range<usize>) -> Option<PaintSnapshot> {
        let operations = self.paint_operations.get(range)?;
        Some(PaintSnapshot {
            operations: operations.into(),
        })
    }

    /// Glint: paints a snapshot again, under the transform in force and `opacity` times as opaque.
    pub fn paint_snapshot(&mut self, snapshot: &PaintSnapshot, opacity: f32) {
        // Layers opened by the snapshot are closed by it, whatever it holds.
        let mut open = 0usize;
        for operation in snapshot.operations.iter() {
            match operation {
                PaintOperation::Primitive(primitive) => {
                    let mut primitive = primitive.clone();
                    if opacity < 1. {
                        primitive.fade(opacity);
                    }
                    self.insert_primitive(primitive);
                }
                PaintOperation::StartLayer(bounds) => {
                    self.push_layer(*bounds);
                    open += 1;
                }
                PaintOperation::EndLayer => {
                    if open > 0 {
                        self.pop_layer();
                        open -= 1;
                    }
                }
            }
        }
        for _ in 0..open {
            self.pop_layer();
        }
    }

    pub fn finish(&mut self) {
        if !self.shadows.is_sorted_by_key(|shadow| shadow.order) {
            self.shadows.sort_by_key(|shadow| shadow.order);
        }
        if !self.quads.is_sorted_by_key(|quad| quad.order) {
            self.quads.sort_by_key(|quad| quad.order);
        }
        if !self.paths.is_sorted_by_key(|path| path.order) {
            self.paths.sort_by_key(|path| path.order);
        }
        if !self.underlines.is_sorted_by_key(|underline| underline.order) {
            self.underlines.sort_by_key(|underline| underline.order);
        }
        if !self
            .monochrome_sprites
            .is_sorted_by_key(|sprite| (sprite.order, sprite.tile.texture_id.index))
        {
            self.monochrome_sprites
                .sort_by_key(|sprite| (sprite.order, sprite.tile.texture_id.index));
        }
        if !self
            .subpixel_sprites
            .is_sorted_by_key(|sprite| (sprite.order, sprite.tile.texture_id.index))
        {
            self.subpixel_sprites
                .sort_by_key(|sprite| (sprite.order, sprite.tile.texture_id.index));
        }
        if !self
            .polychrome_sprites
            .is_sorted_by_key(|sprite| (sprite.order, sprite.tile.texture_id.index))
        {
            self.polychrome_sprites
                .sort_by_key(|sprite| (sprite.order, sprite.tile.texture_id.index));
        }
        if !self.surfaces.is_sorted_by_key(|surface| surface.order) {
            self.surfaces.sort_by_key(|surface| surface.order);
        }
    }

    #[cfg_attr(
        all(
            any(target_os = "linux", target_os = "freebsd"),
            not(any(feature = "x11", feature = "wayland"))
        ),
        allow(dead_code)
    )]
    pub fn batches(&self) -> impl Iterator<Item = PrimitiveBatch> + '_ {
        BatchIterator {
            shadows_start: 0,
            shadows_iter: self.shadows.iter().peekable(),
            quads_start: 0,
            quads_iter: self.quads.iter().peekable(),
            paths_start: 0,
            paths_iter: self.paths.iter().peekable(),
            underlines_start: 0,
            underlines_iter: self.underlines.iter().peekable(),
            monochrome_sprites_start: 0,
            monochrome_sprites_iter: self.monochrome_sprites.iter().peekable(),
            subpixel_sprites_start: 0,
            subpixel_sprites_iter: self.subpixel_sprites.iter().peekable(),
            polychrome_sprites_start: 0,
            polychrome_sprites_iter: self.polychrome_sprites.iter().peekable(),
            surfaces_start: 0,
            surfaces_iter: self.surfaces.iter().peekable(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Default)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
pub(crate) enum PrimitiveKind {
    Shadow,
    #[default]
    Quad,
    Path,
    Underline,
    MonochromeSprite,
    SubpixelSprite,
    PolychromeSprite,
    Surface,
}

#[derive(Clone)]
pub(crate) enum PaintOperation {
    Primitive(Primitive),
    StartLayer(Bounds<ScaledPixels>),
    EndLayer,
}

/// Glint: a uniform scale and a move for everything painted while it is in force
/// ([`crate::Window::with_paint_transform`]): a point `p` is drawn at `p * scale + translate`. The GPU draws
/// the scaled quads, shadows, paths and sprites: nothing is laid out or rasterized again, so text is drawn from
/// the glyphs it already has (soft while it is scaled, sharp again at a scale of one).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintTransform {
    /// How many times larger things are drawn.
    pub scale: f32,
    /// Where the window's origin lands.
    pub translate: Point<Pixels>,
}

impl Default for PaintTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl PaintTransform {
    /// Everything where it is.
    pub const IDENTITY: Self = Self {
        scale: 1.,
        translate: Point {
            x: Pixels(0.),
            y: Pixels(0.),
        },
    };

    /// `scale` times larger, `origin` staying where it is.
    pub fn scale_about(scale: f32, origin: Point<Pixels>) -> Self {
        Self {
            scale,
            translate: Point {
                x: Pixels(origin.x.0 * (1. - scale)),
                y: Pixels(origin.y.0 * (1. - scale)),
            },
        }
    }

    /// Moved by `by`.
    pub fn moved(by: Point<Pixels>) -> Self {
        Self {
            scale: 1.,
            translate: by,
        }
    }

    /// This transform, then `after`.
    pub fn then(self, after: Self) -> Self {
        Self {
            scale: self.scale * after.scale,
            translate: after.apply(self.translate),
        }
    }

    /// The transform that undoes this one; None for a scale of nothing.
    pub fn inverse(self) -> Option<Self> {
        (self.scale.is_finite() && self.scale.abs() > 1e-6).then(|| Self {
            scale: 1. / self.scale,
            translate: Point {
                x: Pixels(-self.translate.x.0 / self.scale),
                y: Pixels(-self.translate.y.0 / self.scale),
            },
        })
    }

    /// Where `point` is drawn.
    pub fn apply(&self, point: Point<Pixels>) -> Point<Pixels> {
        Point {
            x: Pixels(point.x.0 * self.scale + self.translate.x.0),
            y: Pixels(point.y.0 * self.scale + self.translate.y.0),
        }
    }

    /// Where `bounds` are drawn.
    pub fn apply_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        Bounds {
            origin: self.apply(bounds.origin),
            size: Size {
                width: Pixels(bounds.size.width.0 * self.scale),
                height: Pixels(bounds.size.height.0 * self.scale),
            },
        }
    }

    /// Whether it changes nothing that could be seen.
    pub fn is_identity(&self) -> bool {
        (self.scale - 1.).abs() < 1e-4
            && self.translate.x.0.abs() < 1e-3
            && self.translate.y.0.abs() < 1e-3
    }

    /// Whether it can be drawn: a finite scale above nothing and a finite move.
    pub fn is_drawable(&self) -> bool {
        self.scale.is_finite()
            && self.scale > 1e-4
            && self.translate.x.0.is_finite()
            && self.translate.y.0.is_finite()
    }

    /// Its numbers bit for bit, for telling whether two are the very same.
    pub(crate) fn bits(&self) -> [u32; 3] {
        [
            self.scale.to_bits(),
            self.translate.x.0.to_bits(),
            self.translate.y.0.to_bits(),
        ]
    }
}

/// Glint: where painting has come to in a frame ([`crate::Window::paint_position`]): two of them name what was
/// painted between them, for [`crate::Window::snapshot_painted`] once that frame is the one on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaintPosition {
    pub(crate) frame: u64,
    pub(crate) index: usize,
}

/// Glint: what a stretch of a frame painted, kept to be painted again (see [`Scene::snapshot`]). Its sprites
/// name tiles of the atlas as they were: one whose image was dropped since draws nothing, or whatever took
/// its place, so a snapshot is for the next moments, not for keeping.
#[derive(Clone)]
pub struct PaintSnapshot {
    operations: std::rc::Rc<[PaintOperation]>,
}

impl PaintSnapshot {
    /// Whether it holds nothing to paint.
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }
}

/// Glint: a uniform scale and a move, in device pixels, with the clip of where it began (in the coordinates
/// things are drawn in).
#[derive(Clone, Copy, Debug, PartialEq)]
struct SceneTransform {
    scale: f32,
    translate: Point<ScaledPixels>,
    clip: Bounds<ScaledPixels>,
}

impl SceneTransform {
    fn point(&self, point: Point<ScaledPixels>) -> Point<ScaledPixels> {
        Point {
            x: ScaledPixels(point.x.0 * self.scale + self.translate.x.0),
            y: ScaledPixels(point.y.0 * self.scale + self.translate.y.0),
        }
    }

    fn length(&self, length: ScaledPixels) -> ScaledPixels {
        ScaledPixels(length.0 * self.scale)
    }

    fn bounds(&self, bounds: Bounds<ScaledPixels>) -> Bounds<ScaledPixels> {
        Bounds {
            origin: self.point(bounds.origin),
            size: Size {
                width: self.length(bounds.size.width),
                height: self.length(bounds.size.height),
            },
        }
    }

    fn mask(&self, mask: ContentMask<ScaledPixels>) -> ContentMask<ScaledPixels> {
        ContentMask {
            bounds: self.bounds(mask.bounds).intersect(&self.clip),
        }
    }

    fn corners(&self, corners: Corners<ScaledPixels>) -> Corners<ScaledPixels> {
        Corners {
            top_left: self.length(corners.top_left),
            top_right: self.length(corners.top_right),
            bottom_right: self.length(corners.bottom_right),
            bottom_left: self.length(corners.bottom_left),
        }
    }

    fn edges(&self, edges: Edges<ScaledPixels>) -> Edges<ScaledPixels> {
        Edges {
            top: self.length(edges.top),
            right: self.length(edges.right),
            bottom: self.length(edges.bottom),
            left: self.length(edges.left),
        }
    }

    /// A sprite's own matrix (a turning icon's) is applied by the shader to the sprite's bounds, which are
    /// transformed here: the matrix keeps its turn and takes the move that makes the two agree.
    fn sprite(&self, matrix: TransformationMatrix) -> TransformationMatrix {
        if matrix == TransformationMatrix::unit() {
            return matrix;
        }
        let [[a, b], [c, d]] = matrix.rotation_scale;
        let (x, y) = (self.translate.x.0, self.translate.y.0);
        TransformationMatrix {
            rotation_scale: matrix.rotation_scale,
            translation: [
                self.scale * matrix.translation[0] + x - (a * x + b * y),
                self.scale * matrix.translation[1] + y - (c * x + d * y),
            ],
        }
    }
}

#[derive(Clone)]
#[expect(missing_docs)]
pub enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Path(Path<ScaledPixels>),
    Underline(Underline),
    MonochromeSprite(MonochromeSprite),
    SubpixelSprite(SubpixelSprite),
    PolychromeSprite(PolychromeSprite),
    Surface(PaintSurface),
}

#[expect(missing_docs)]
impl Primitive {
    /// Glint: the primitive where `transform` draws it.
    fn transform(&mut self, transform: &SceneTransform) {
        match self {
            Primitive::Shadow(shadow) => {
                shadow.bounds = transform.bounds(shadow.bounds);
                shadow.element_bounds = transform.bounds(shadow.element_bounds);
                shadow.content_mask = transform.mask(shadow.content_mask);
                shadow.corner_radii = transform.corners(shadow.corner_radii);
                shadow.element_corner_radii = transform.corners(shadow.element_corner_radii);
                shadow.blur_radius = transform.length(shadow.blur_radius);
            }
            Primitive::Quad(quad) => {
                quad.bounds = transform.bounds(quad.bounds);
                quad.content_mask = transform.mask(quad.content_mask);
                quad.corner_radii = transform.corners(quad.corner_radii);
                quad.border_widths = transform.edges(quad.border_widths);
            }
            Primitive::Path(path) => {
                path.bounds = transform.bounds(path.bounds);
                path.content_mask = transform.mask(path.content_mask.clone());
                for vertex in &mut path.vertices {
                    vertex.xy_position = transform.point(vertex.xy_position);
                    vertex.content_mask = transform.mask(vertex.content_mask.clone());
                }
            }
            Primitive::Underline(underline) => {
                underline.bounds = transform.bounds(underline.bounds);
                underline.content_mask = transform.mask(underline.content_mask);
                underline.thickness = transform.length(underline.thickness);
            }
            Primitive::MonochromeSprite(sprite) => {
                sprite.bounds = transform.bounds(sprite.bounds);
                sprite.content_mask = transform.mask(sprite.content_mask);
                sprite.transformation = transform.sprite(sprite.transformation);
            }
            Primitive::SubpixelSprite(sprite) => {
                sprite.bounds = transform.bounds(sprite.bounds);
                sprite.content_mask = transform.mask(sprite.content_mask);
                sprite.transformation = transform.sprite(sprite.transformation);
            }
            Primitive::PolychromeSprite(sprite) => {
                sprite.bounds = transform.bounds(sprite.bounds);
                sprite.content_mask = transform.mask(sprite.content_mask);
                sprite.corner_radii = transform.corners(sprite.corner_radii);
            }
            Primitive::Surface(surface) => {
                surface.bounds = transform.bounds(surface.bounds);
                surface.content_mask = transform.mask(surface.content_mask.clone());
            }
        }
    }

    /// Glint: the primitive `opacity` times as opaque.
    fn fade(&mut self, opacity: f32) {
        match self {
            Primitive::Shadow(shadow) => shadow.color = shadow.color.opacity(opacity),
            Primitive::Quad(quad) => {
                quad.background = quad.background.opacity(opacity);
                quad.border_color = quad.border_color.opacity(opacity);
            }
            Primitive::Path(path) => path.color = path.color.opacity(opacity),
            Primitive::Underline(underline) => underline.color = underline.color.opacity(opacity),
            Primitive::MonochromeSprite(sprite) => sprite.color = sprite.color.opacity(opacity),
            Primitive::SubpixelSprite(sprite) => sprite.color = sprite.color.opacity(opacity),
            Primitive::PolychromeSprite(sprite) => sprite.opacity *= opacity,
            Primitive::Surface(_) => {}
        }
    }

    pub fn bounds(&self) -> &Bounds<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.bounds,
            Primitive::Quad(quad) => &quad.bounds,
            Primitive::Path(path) => &path.bounds,
            Primitive::Underline(underline) => &underline.bounds,
            Primitive::MonochromeSprite(sprite) => &sprite.bounds,
            Primitive::SubpixelSprite(sprite) => &sprite.bounds,
            Primitive::PolychromeSprite(sprite) => &sprite.bounds,
            Primitive::Surface(surface) => &surface.bounds,
        }
    }

    pub fn content_mask(&self) -> &ContentMask<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.content_mask,
            Primitive::Quad(quad) => &quad.content_mask,
            Primitive::Path(path) => &path.content_mask,
            Primitive::Underline(underline) => &underline.content_mask,
            Primitive::MonochromeSprite(sprite) => &sprite.content_mask,
            Primitive::SubpixelSprite(sprite) => &sprite.content_mask,
            Primitive::PolychromeSprite(sprite) => &sprite.content_mask,
            Primitive::Surface(surface) => &surface.content_mask,
        }
    }
}

#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
struct BatchIterator<'a> {
    shadows_start: usize,
    shadows_iter: Peekable<slice::Iter<'a, Shadow>>,
    quads_start: usize,
    quads_iter: Peekable<slice::Iter<'a, Quad>>,
    paths_start: usize,
    paths_iter: Peekable<slice::Iter<'a, Path<ScaledPixels>>>,
    underlines_start: usize,
    underlines_iter: Peekable<slice::Iter<'a, Underline>>,
    monochrome_sprites_start: usize,
    monochrome_sprites_iter: Peekable<slice::Iter<'a, MonochromeSprite>>,
    subpixel_sprites_start: usize,
    subpixel_sprites_iter: Peekable<slice::Iter<'a, SubpixelSprite>>,
    polychrome_sprites_start: usize,
    polychrome_sprites_iter: Peekable<slice::Iter<'a, PolychromeSprite>>,
    surfaces_start: usize,
    surfaces_iter: Peekable<slice::Iter<'a, PaintSurface>>,
}

impl<'a> Iterator for BatchIterator<'a> {
    type Item = PrimitiveBatch;

    fn next(&mut self) -> Option<Self::Item> {
        let mut orders_and_kinds = [
            (
                self.shadows_iter.peek().map(|s| s.order),
                PrimitiveKind::Shadow,
            ),
            (self.quads_iter.peek().map(|q| q.order), PrimitiveKind::Quad),
            (self.paths_iter.peek().map(|q| q.order), PrimitiveKind::Path),
            (
                self.underlines_iter.peek().map(|u| u.order),
                PrimitiveKind::Underline,
            ),
            (
                self.monochrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::MonochromeSprite,
            ),
            (
                self.subpixel_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::SubpixelSprite,
            ),
            (
                self.polychrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::PolychromeSprite,
            ),
            (
                self.surfaces_iter.peek().map(|s| s.order),
                PrimitiveKind::Surface,
            ),
        ];
        orders_and_kinds.sort_by_key(|(order, kind)| (order.unwrap_or(u32::MAX), *kind));

        let first = orders_and_kinds[0];
        let second = orders_and_kinds[1];
        let (batch_kind, max_order_and_kind) = if first.0.is_some() {
            (first.1, (second.0.unwrap_or(u32::MAX), second.1))
        } else {
            return None;
        };

        match batch_kind {
            PrimitiveKind::Shadow => {
                let shadows_start = self.shadows_start;
                let mut shadows_end = shadows_start + 1;
                self.shadows_iter.next();
                while self
                    .shadows_iter
                    .next_if(|shadow| (shadow.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    shadows_end += 1;
                }
                self.shadows_start = shadows_end;
                Some(PrimitiveBatch::Shadows(shadows_start..shadows_end))
            }
            PrimitiveKind::Quad => {
                let quads_start = self.quads_start;
                let mut quads_end = quads_start + 1;
                self.quads_iter.next();
                while self
                    .quads_iter
                    .next_if(|quad| (quad.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    quads_end += 1;
                }
                self.quads_start = quads_end;
                Some(PrimitiveBatch::Quads(quads_start..quads_end))
            }
            PrimitiveKind::Path => {
                let paths_start = self.paths_start;
                let mut paths_end = paths_start + 1;
                self.paths_iter.next();
                while self
                    .paths_iter
                    .next_if(|path| (path.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    paths_end += 1;
                }
                self.paths_start = paths_end;
                Some(PrimitiveBatch::Paths(paths_start..paths_end))
            }
            PrimitiveKind::Underline => {
                let underlines_start = self.underlines_start;
                let mut underlines_end = underlines_start + 1;
                self.underlines_iter.next();
                while self
                    .underlines_iter
                    .next_if(|underline| (underline.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    underlines_end += 1;
                }
                self.underlines_start = underlines_end;
                Some(PrimitiveBatch::Underlines(underlines_start..underlines_end))
            }
            PrimitiveKind::MonochromeSprite => {
                let texture_id = self.monochrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.monochrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.monochrome_sprites_iter.next();
                while self
                    .monochrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.monochrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::SubpixelSprite => {
                let texture_id = self.subpixel_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.subpixel_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.subpixel_sprites_iter.next();
                while self
                    .subpixel_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.subpixel_sprites_start = sprites_end;
                Some(PrimitiveBatch::SubpixelSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::PolychromeSprite => {
                let texture_id = self.polychrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.polychrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.polychrome_sprites_iter.next();
                while self
                    .polychrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.polychrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::Surface => {
                let surfaces_start = self.surfaces_start;
                let mut surfaces_end = surfaces_start + 1;
                self.surfaces_iter.next();
                while self
                    .surfaces_iter
                    .next_if(|surface| (surface.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    surfaces_end += 1;
                }
                self.surfaces_start = surfaces_end;
                Some(PrimitiveBatch::Surfaces(surfaces_start..surfaces_end))
            }
        }
    }
}

#[derive(Debug)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
#[allow(missing_docs)]
pub enum PrimitiveBatch {
    Shadows(Range<usize>),
    Quads(Range<usize>),
    Paths(Range<usize>),
    Underlines(Range<usize>),
    MonochromeSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    SubpixelSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    PolychromeSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    Surfaces(Range<usize>),
}

impl PrimitiveBatch {
    #[expect(missing_docs)]
    pub fn label(&self) -> String {
        match self {
            Self::Shadows(range) => format!("shadows ({})", range.len()),
            Self::Quads(range) => format!("quads ({})", range.len()),
            Self::Paths(range) => format!("paths ({})", range.len()),
            Self::Underlines(range) => format!("underlines ({})", range.len()),
            Self::MonochromeSprites { texture_id, range } => {
                format!(
                    "monochrome sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::SubpixelSprites { texture_id, range } => {
                format!(
                    "subpixel sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::PolychromeSprites { texture_id, range } => {
                format!(
                    "polychrome sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::Surfaces(range) => format!("surfaces ({})", range.len()),
        }
    }
}

#[derive(Default, Debug, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Quad {
    pub order: DrawOrder,
    pub border_style: BorderStyle,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub background: Background,
    pub border_color: Hsla,
    pub corner_radii: Corners<ScaledPixels>,
    pub border_widths: Edges<ScaledPixels>,
}

impl From<Quad> for Primitive {
    fn from(quad: Quad) -> Self {
        Primitive::Quad(quad)
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Underline {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub thickness: ScaledPixels,
    pub wavy: PaddedBool32,
}

impl From<Underline> for Primitive {
    fn from(underline: Underline) -> Self {
        Primitive::Underline(underline)
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Shadow {
    pub order: DrawOrder,
    pub blur_radius: ScaledPixels,
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub element_bounds: Bounds<ScaledPixels>,
    pub element_corner_radii: Corners<ScaledPixels>,
    /// 0 = drop shadow (rendered outside the element), 1 = inset shadow (rendered inside).
    pub inset: u32,
    pub pad: u32, // align to 8 bytes
}

impl From<Shadow> for Primitive {
    fn from(shadow: Shadow) -> Self {
        Primitive::Shadow(shadow)
    }
}

/// The style of a border.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub enum BorderStyle {
    /// A solid border.
    #[default]
    Solid = 0,
    /// A dashed border.
    Dashed = 1,
}

/// A data type representing a 2 dimensional transformation that can be applied to an element.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct TransformationMatrix {
    /// 2x2 matrix containing rotation and scale,
    /// stored row-major
    pub rotation_scale: [[f32; 2]; 2],
    /// translation vector
    pub translation: [f32; 2],
}

impl Eq for TransformationMatrix {}

impl TransformationMatrix {
    /// The unit matrix, has no effect.
    pub fn unit() -> Self {
        Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [0.0, 0.0],
        }
    }

    /// Move the origin by a given point
    pub fn translate(self, point: Point<ScaledPixels>) -> Self {
        self.compose(Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [point.x.0, point.y.0],
        })
    }

    /// Clockwise rotation in radians around the origin
    pub fn rotate(self, angle: Radians) -> Self {
        self.compose(Self {
            rotation_scale: [
                [angle.0.cos(), -angle.0.sin()],
                [angle.0.sin(), angle.0.cos()],
            ],
            translation: [0.0, 0.0],
        })
    }

    /// Scale around the origin
    pub fn scale(self, size: Size<f32>) -> Self {
        self.compose(Self {
            rotation_scale: [[size.width, 0.0], [0.0, size.height]],
            translation: [0.0, 0.0],
        })
    }

    /// Perform matrix multiplication with another transformation
    /// to produce a new transformation that is the result of
    /// applying both transformations: first, `other`, then `self`.
    #[inline]
    pub fn compose(self, other: TransformationMatrix) -> TransformationMatrix {
        if other == Self::unit() {
            return self;
        }
        // Perform matrix multiplication
        TransformationMatrix {
            rotation_scale: [
                [
                    self.rotation_scale[0][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][0],
                    self.rotation_scale[0][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][1],
                ],
                [
                    self.rotation_scale[1][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][0],
                    self.rotation_scale[1][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][1],
                ],
            ],
            translation: [
                self.translation[0]
                    + self.rotation_scale[0][0] * other.translation[0]
                    + self.rotation_scale[0][1] * other.translation[1],
                self.translation[1]
                    + self.rotation_scale[1][0] * other.translation[0]
                    + self.rotation_scale[1][1] * other.translation[1],
            ],
        }
    }

    /// Apply transformation to a point, mainly useful for debugging
    pub fn apply(&self, point: Point<Pixels>) -> Point<Pixels> {
        let input = [point.x.0, point.y.0];
        let mut output = self.translation;
        for (i, output_cell) in output.iter_mut().enumerate() {
            for (k, input_cell) in input.iter().enumerate() {
                *output_cell += self.rotation_scale[i][k] * *input_cell;
            }
        }
        Point::new(output[0].into(), output[1].into())
    }
}

impl Default for TransformationMatrix {
    fn default() -> Self {
        Self::unit()
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct MonochromeSprite {
    pub order: DrawOrder,
    pub pad: u32,
    pub tile: AtlasTile,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub transformation: TransformationMatrix,
}

impl From<MonochromeSprite> for Primitive {
    fn from(sprite: MonochromeSprite) -> Self {
        Primitive::MonochromeSprite(sprite)
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct SubpixelSprite {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub tile: AtlasTile,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub transformation: TransformationMatrix,
}

impl From<SubpixelSprite> for Primitive {
    fn from(sprite: SubpixelSprite) -> Self {
        Primitive::SubpixelSprite(sprite)
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct PolychromeSprite {
    pub order: DrawOrder,
    pub pad: u32,
    pub tile: AtlasTile,
    pub grayscale: PaddedBool32,
    pub opacity: f32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
}

impl From<PolychromeSprite> for Primitive {
    fn from(sprite: PolychromeSprite) -> Self {
        Primitive::PolychromeSprite(sprite)
    }
}

#[derive(Clone, Debug)]
#[allow(missing_docs)]
pub struct PaintSurface {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub image_buffer: objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>,
}

impl From<PaintSurface> for Primitive {
    fn from(surface: PaintSurface) -> Self {
        Primitive::Surface(surface)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[expect(missing_docs)]
pub struct PathId(pub usize);

/// A line made up of a series of vertices and control points.
#[derive(Clone, Debug)]
#[expect(missing_docs)]
pub struct Path<P: Clone + Debug + Default + PartialEq> {
    pub id: PathId,
    pub order: DrawOrder,
    pub bounds: Bounds<P>,
    pub content_mask: ContentMask<P>,
    pub vertices: Vec<PathVertex<P>>,
    pub color: Background,
    start: Point<P>,
    current: Point<P>,
    contour_count: usize,
}

impl Path<Pixels> {
    /// Create a new path with the given starting point.
    pub fn new(start: Point<Pixels>) -> Self {
        Self {
            id: PathId(0),
            order: DrawOrder::default(),
            vertices: Vec::new(),
            start,
            current: start,
            bounds: Bounds {
                origin: start,
                size: Default::default(),
            },
            content_mask: Default::default(),
            color: Default::default(),
            contour_count: 0,
        }
    }

    /// Scale this path by the given factor.
    pub fn scale(&self, factor: f32) -> Path<ScaledPixels> {
        Path {
            id: self.id,
            order: self.order,
            bounds: self.bounds.scale(factor),
            content_mask: self.content_mask.scale(factor),
            vertices: self
                .vertices
                .iter()
                .map(|vertex| vertex.scale(factor))
                .collect(),
            start: self.start.map(|start| start.scale(factor)),
            current: self.current.scale(factor),
            contour_count: self.contour_count,
            color: self.color,
        }
    }

    /// Move the start, current point to the given point.
    pub fn move_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        self.start = to;
        self.current = to;
    }

    /// Draw a straight line from the current point to the given point.
    pub fn line_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }
        self.current = to;
    }

    /// Draw a curve from the current point to the given point, using the given control point.
    pub fn curve_to(&mut self, to: Point<Pixels>, ctrl: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }

        self.push_triangle(
            (self.current, ctrl, to),
            (point(0., 0.), point(0.5, 0.), point(1., 1.)),
        );
        self.current = to;
    }

    /// Push a triangle to the Path.
    pub fn push_triangle(
        &mut self,
        xy: (Point<Pixels>, Point<Pixels>, Point<Pixels>),
        st: (Point<f32>, Point<f32>, Point<f32>),
    ) {
        self.bounds = self
            .bounds
            .union(&Bounds {
                origin: xy.0,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.1,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.2,
                size: Default::default(),
            });

        self.vertices.push(PathVertex {
            xy_position: xy.0,
            st_position: st.0,
            content_mask: Default::default(),
        });
        self.vertices.push(PathVertex {
            xy_position: xy.1,
            st_position: st.1,
            content_mask: Default::default(),
        });
        self.vertices.push(PathVertex {
            xy_position: xy.2,
            st_position: st.2,
            content_mask: Default::default(),
        });
    }
}

impl<T> Path<T>
where
    T: Clone + Debug + Default + PartialEq + PartialOrd + Add<T, Output = T> + Sub<Output = T>,
{
    #[allow(unused)]
    #[expect(missing_docs)]
    pub fn clipped_bounds(&self) -> Bounds<T> {
        self.bounds.intersect(&self.content_mask.bounds)
    }
}

impl From<Path<ScaledPixels>> for Primitive {
    fn from(path: Path<ScaledPixels>) -> Self {
        Primitive::Path(path)
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct PathVertex<P: Clone + Debug + Default + PartialEq> {
    pub xy_position: Point<P>,
    pub st_position: Point<f32>,
    pub content_mask: ContentMask<P>,
}

#[expect(missing_docs)]
impl PathVertex<Pixels> {
    pub fn scale(&self, factor: f32) -> PathVertex<ScaledPixels> {
        PathVertex {
            xy_position: self.xy_position.scale(factor),
            st_position: self.st_position,
            content_mask: self.content_mask.scale(factor),
        }
    }
}
