//! The software renderer: a mesh and a camera in, a picture out (the `WebGLRenderer` and `OrbitControls` of
//! ModelView.tsx, on the CPU).
//!
//! **What it draws.** Perspective (a 45 degree field of view, as the TypeScript's camera), a z-buffer, and one
//! light that sits at the camera: a pixel is the model's colour times `ambient + (1 - ambient) * |n . l|`, where
//! `l` points at the viewer. Faces are lit from both sides (`THREE.DoubleSide`). Normals are the mesh's own per
//! vertex ([`Shading::Smooth`], interpolated with the perspective taken into account) or the triangle's
//! ([`Shading::Flat`]). Per-vertex colours replace the model colour. What no triangle covers is transparent, so
//! the pane's own background (the theme's) shows through, as the TypeScript's `alpha: true` canvas did.
//!
//! **How it is fast enough.** Every vertex is moved into the camera's space and projected once; each
//! triangle then only looks up three projected corners. The picture is cut into horizontal bands, one thread
//! each (up to four, and only for pictures big enough to pay for them), every band drawing the triangles that
//! touch it into its own slice of the colour and depth buffers. Triangles crossing the near plane are clipped
//! to it. While the pointer drags, the view asks for half the size, no supersampling and at most
//! [`Params::max_triangles`] triangles (every n-th); when it stops, the full picture is drawn with 2 x 2
//! supersampling for smooth edges.
//!
//! **The camera** ([`Camera`]) turns about the middle of the model's box on its y axis (y is up, as in the
//! TypeScript), and its distance is the one at which the model's bounding sphere just fits the picture,
//! divided by the zoom, so a window of any shape starts with the whole model in view ("auto-fit").

use super::model3d_mesh::{add, cross, dot, normalize, scale, sub, Mesh, Shading, V3};
use image::RgbaImage;

/// The camera's field of view, top to bottom (`PerspectiveCamera(45, …)`).
pub const FOV_Y: f32 = std::f32::consts::FRAC_PI_4;
/// How far the wheel zooms.
pub const MIN_ZOOM: f32 = 0.05;
pub const MAX_ZOOM: f32 = 200.;
/// Just short of straight up and down, where "right" stops meaning anything (`OrbitControls`' polar limit).
const MAX_PITCH: f32 = 1.55;
/// The light every surface gets, facing it or not.
const AMBIENT: f32 = 0.32;
/// The most pixels worked on at once; a bigger picture is drawn without supersampling.
const MAX_WORK_PIXELS: u64 = 6_000_000;
/// How far a drag may move the model off the middle, in radii.
const MAX_PAN: f32 = 20.;

/// Where the model is looked at from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// About the vertical axis, radians; 0 looks along -z.
    pub yaw: f32,
    /// Above the horizon, radians.
    pub pitch: f32,
    /// 1 is the whole model in view; bigger is closer.
    pub zoom: f32,
    /// How far the model is moved across the picture, in radii of the model's box.
    pub pan: [f32; 2],
}

impl Camera {
    /// The first view: the TypeScript put the camera at `(r * 1.6, r * 1.1, r * 1.8)` looking at the middle.
    pub fn home() -> Camera {
        Camera {
            yaw: 1.6f32.atan2(1.8),
            pitch: 1.1f32.atan2(1.6f32.hypot(1.8)),
            zoom: 1.,
            pan: [0., 0.],
        }
    }

    /// Turned by `yaw` and `pitch` radians.
    pub fn turned(self, yaw: f32, pitch: f32) -> Camera {
        let wrapped = (self.yaw + yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        Camera {
            yaw: wrapped,
            pitch: (self.pitch + pitch).clamp(-MAX_PITCH, MAX_PITCH),
            ..self
        }
    }

    /// Closer by `factor` (more than 1 zooms in).
    pub fn zoomed(self, factor: f32) -> Camera {
        Camera {
            zoom: (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM),
            ..self
        }
    }

    /// Moved by `across` and `up` radii.
    pub fn panned(self, across: f32, up: f32) -> Camera {
        Camera {
            pan: [
                (self.pan[0] + across).clamp(-MAX_PAN, MAX_PAN),
                (self.pan[1] + up).clamp(-MAX_PAN, MAX_PAN),
            ],
            ..self
        }
    }
}

/// How far the camera must be from a sphere of `radius` for it to fit a picture `aspect` (width over height)
/// wide, with a little room.
pub fn fit_distance(radius: f32, aspect: f32) -> f32 {
    let half = ((FOV_Y / 2.).tan() * aspect.clamp(0.05, 1.)).atan();
    radius / half.sin() * 1.05
}

/// How many radii of the model one pixel of a picture `height` pixels tall is, at `zoom` (what a drag moves by).
pub fn radii_per_pixel(aspect: f32, height: f32, zoom: f32) -> f32 {
    2. * (FOV_Y / 2.).tan() * fit_distance(1., aspect) / zoom.max(MIN_ZOOM) / height.max(1.)
}

/// What to draw.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// The picture's size in pixels.
    pub width: u32,
    pub height: u32,
    /// Each pixel is this many samples across (1 or 2; more is only used for small pictures).
    pub supersample: u32,
    /// The model's colour where it has none of its own.
    pub color: [u8; 3],
    /// The most triangles drawn: with more, every n-th is.
    pub max_triangles: usize,
    /// Threads to draw with; 0 chooses by the size of the work.
    pub threads: usize,
}

/// The camera as vectors.
struct Basis {
    eye: V3,
    right: V3,
    up: V3,
    forward: V3,
    /// Points from the model at the camera: the light's direction.
    back: V3,
}

fn basis(camera: &Camera, mesh: &Mesh, aspect: f32) -> Basis {
    let radius = mesh.radius();
    let (sy, cy) = camera.yaw.sin_cos();
    let (sp, cp) = camera.pitch.sin_cos();
    let back = [cp * sy, sp, cp * cy];
    let forward = scale(back, -1.);
    let right = normalize(cross(forward, [0., 1., 0.]));
    let up = cross(right, forward);
    let moved = add(scale(right, camera.pan[0]), scale(up, camera.pan[1]));
    let target = add(mesh.center(), scale(moved, radius));
    let eye = add(target, scale(back, fit_distance(radius, aspect) / camera.zoom));
    Basis {
        eye,
        right,
        up,
        forward,
        back,
    }
}

/// A vertex in the camera's space, projected.
#[derive(Clone, Copy, Default)]
struct Proj {
    x: f32,
    y: f32,
    /// One over the depth; 0 for a vertex behind the near plane.
    iz: f32,
    /// The cosine between the normal and the light (smooth shading).
    c: f32,
}

/// What projecting needs.
struct Lens {
    basis: Basis,
    focal: f32,
    half_w: f32,
    half_h: f32,
    near: f32,
}

impl Lens {
    /// x right, y up, z the depth along the view.
    fn to_view(&self, p: V3) -> V3 {
        let d = sub(p, self.basis.eye);
        [
            dot(d, self.basis.right),
            dot(d, self.basis.up),
            dot(d, self.basis.forward),
        ]
    }

    fn project(&self, v: V3) -> (f32, f32, f32) {
        let iz = 1. / v[2];
        (
            self.half_w + v[0] * iz * self.focal,
            self.half_h - v[1] * iz * self.focal,
            iz,
        )
    }
}

fn project_range(mesh: &Mesh, lens: &Lens, first: usize, view: &mut [V3], proj: &mut [Proj]) {
    for (k, (v, out)) in view.iter_mut().zip(proj.iter_mut()).enumerate() {
        let i = first + k;
        *v = lens.to_view(mesh.positions[i]);
        let c = match &mesh.shading {
            Shading::Smooth(normals) => dot(normals[i], lens.basis.back),
            Shading::Flat => 0.,
        };
        *out = if v[2] > lens.near {
            let (x, y, iz) = lens.project(*v);
            Proj { x, y, iz, c }
        } else {
            Proj { c, ..Proj::default() }
        };
    }
}

/// A corner of a triangle on its way to being filled.
#[derive(Clone, Copy)]
struct Vert {
    x: f32,
    y: f32,
    iz: f32,
    c: f32,
    rgb: V3,
}

/// A corner in the camera's space, for clipping.
#[derive(Clone, Copy)]
struct Corner {
    p: V3,
    c: f32,
    rgb: V3,
}

/// A band of the picture: some rows of both buffers.
struct Band<'a> {
    first_row: usize,
    rows: usize,
    color: &'a mut [u8],
    depth: &'a mut [f32],
}

/// Everything the bands share.
struct Scene<'a> {
    mesh: &'a Mesh,
    lens: &'a Lens,
    view: &'a [V3],
    proj: &'a [Proj],
    stride: usize,
    solid: V3,
    width: usize,
}

fn shade(c: f32) -> f32 {
    AMBIENT + (1. - AMBIENT) * c.abs().min(1.)
}

fn to_byte(v: f32) -> u8 {
    v.round().clamp(0., 255.) as u8
}

impl Scene<'_> {
    fn rgb_of(&self, i: u32) -> V3 {
        self.mesh
            .colors
            .as_ref()
            .and_then(|c| c.get(i as usize))
            .map_or(self.solid, |c| [f32::from(c[0]), f32::from(c[1]), f32::from(c[2])])
    }

    /// Draws every triangle that touches `band` into it.
    fn draw(&self, band: &mut Band) {
        let (y_top, y_bottom) = (band.first_row as f32, (band.first_row + band.rows) as f32);
        let colored = self.mesh.colors.is_some();
        for t in (0..self.mesh.triangles()).step_by(self.stride) {
            let ix = [
                self.mesh.indices[t * 3],
                self.mesh.indices[t * 3 + 1],
                self.mesh.indices[t * 3 + 2],
            ];
            let at = ix.map(|i| i as usize);
            let (Some(a), Some(b), Some(c)) = (self.proj.get(at[0]), self.proj.get(at[1]), self.proj.get(at[2])) else {
                continue;
            };
            let flat = matches!(self.mesh.shading, Shading::Flat);
            if a.iz > 0. && b.iz > 0. && c.iz > 0. {
                if a.y.max(b.y).max(c.y) < y_top || a.y.min(b.y).min(c.y) > y_bottom {
                    continue;
                }
                let face = flat.then(|| self.face_cosine(at));
                let corner = |p: &Proj, i: u32| Vert {
                    x: p.x,
                    y: p.y,
                    iz: p.iz,
                    c: face.unwrap_or(p.c),
                    rgb: if colored { self.rgb_of(i) } else { self.solid },
                };
                self.fill(
                    band,
                    [corner(a, ix[0]), corner(b, ix[1]), corner(c, ix[2])],
                    face,
                    colored,
                );
            } else if a.iz > 0. || b.iz > 0. || c.iz > 0. {
                // Some of it is in front of the near plane, some of it behind.
                let face = flat.then(|| self.face_cosine(at));
                let corner = |k: usize| Corner {
                    p: self.view[at[k]],
                    c: face.unwrap_or(self.proj[at[k]].c),
                    rgb: if colored { self.rgb_of(ix[k]) } else { self.solid },
                };
                let (polygon, n) = clip_near([corner(0), corner(1), corner(2)], self.lens.near);
                let verts: Vec<Vert> = polygon[..n]
                    .iter()
                    .map(|q| {
                        let (x, y, iz) = self.lens.project(q.p);
                        Vert {
                            x,
                            y,
                            iz,
                            c: q.c,
                            rgb: q.rgb,
                        }
                    })
                    .collect();
                if let Some((first, rest)) = verts.split_first() {
                    for pair in rest.windows(2) {
                        self.fill(band, [*first, pair[0], pair[1]], face, colored);
                    }
                }
            }
        }
    }

    /// The cosine between a triangle's own normal and the light.
    fn face_cosine(&self, at: [usize; 3]) -> f32 {
        let (a, b, c) = (self.view[at[0]], self.view[at[1]], self.view[at[2]]);
        let n = cross(sub(b, a), sub(c, a));
        let len = dot(n, n).sqrt();
        if len > 0. && len.is_finite() {
            n[2] / len
        } else {
            0.
        }
    }

    /// Fills a projected triangle into the band, nearest wins.
    fn fill(&self, band: &mut Band, v: [Vert; 3], face: Option<f32>, colored: bool) {
        let [v0, v1, v2] = v;
        let area = (v1.x - v0.x) * (v2.y - v0.y) - (v2.x - v0.x) * (v1.y - v0.y);
        if area.abs() <= 1e-9 || !area.is_finite() {
            return;
        }
        let inv = 1. / area;
        let width = self.width as f32;
        let (min_x, max_x) = (v0.x.min(v1.x).min(v2.x), v0.x.max(v1.x).max(v2.x));
        let (min_y, max_y) = (v0.y.min(v1.y).min(v2.y), v0.y.max(v1.y).max(v2.y));
        let top = band.first_row as f32;
        let bottom = (band.first_row + band.rows) as f32 - 1.;
        if max_x < 0. || min_x > width - 1. || max_y < top || min_y > bottom + 1. {
            return;
        }
        let lo_x = min_x.floor().max(0.) as usize;
        let hi_x = max_x.ceil().min(width - 1.) as usize;
        let lo_y = min_y.floor().max(top) as usize;
        let hi_y = max_y.ceil().min(bottom) as usize;
        // One colour for the whole triangle: flat shading of a model with a single colour.
        let fixed = (!colored)
            .then_some(face)
            .flatten()
            .map(|c| self.solid.map(|k| to_byte(k * shade(c))));
        let edge = |a: &Vert, b: &Vert, px: f32, py: f32| (b.x - a.x) * (py - a.y) - (b.y - a.y) * (px - a.x);
        let (dx0, dx1, dx2) = (-(v2.y - v1.y) * inv, -(v0.y - v2.y) * inv, -(v1.y - v0.y) * inv);
        for y in lo_y..=hi_y {
            let py = y as f32 + 0.5;
            let px = lo_x as f32 + 0.5;
            let mut l0 = edge(&v1, &v2, px, py) * inv;
            let mut l1 = edge(&v2, &v0, px, py) * inv;
            let mut l2 = edge(&v0, &v1, px, py) * inv;
            let row = (y - band.first_row) * self.width;
            for x in lo_x..=hi_x {
                const EPS: f32 = -1e-5;
                if l0 >= EPS && l1 >= EPS && l2 >= EPS {
                    let iz = l0 * v0.iz + l1 * v1.iz + l2 * v2.iz;
                    let at = row + x;
                    if let Some(depth) = band.depth.get_mut(at) {
                        if iz > *depth {
                            *depth = iz;
                            let rgb = fixed.unwrap_or_else(|| {
                                let (k0, k1, k2) = (l0 * v0.iz, l1 * v1.iz, l2 * v2.iz);
                                let lit = shade((k0 * v0.c + k1 * v1.c + k2 * v2.c) / iz);
                                let base =
                                    |axis: usize| (k0 * v0.rgb[axis] + k1 * v1.rgb[axis] + k2 * v2.rgb[axis]) / iz;
                                [to_byte(base(0) * lit), to_byte(base(1) * lit), to_byte(base(2) * lit)]
                            });
                            if let Some(pixel) = band.color.get_mut(at * 4..at * 4 + 4) {
                                pixel.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
                            }
                        }
                    }
                }
                l0 += dx0;
                l1 += dx1;
                l2 += dx2;
            }
        }
    }
}

/// A triangle cut to the half-space in front of the near plane: a polygon of three or four corners.
fn clip_near(tri: [Corner; 3], near: f32) -> ([Corner; 4], usize) {
    let mut out = [tri[0]; 4];
    let mut n = 0;
    for k in 0..3 {
        let (a, b) = (tri[k], tri[(k + 1) % 3]);
        let (a_in, b_in) = (a.p[2] > near, b.p[2] > near);
        if a_in && n < 4 {
            out[n] = a;
            n += 1;
        }
        if a_in != b_in && n < 4 {
            let t = (near - a.p[2]) / (b.p[2] - a.p[2]);
            let lerp = |x: V3, y: V3| add(x, scale(sub(y, x), t));
            out[n] = Corner {
                p: lerp(a.p, b.p),
                c: a.c + (b.c - a.c) * t,
                rgb: lerp(a.rgb, b.rgb),
            };
            n += 1;
        }
    }
    (out, n)
}

fn thread_count(wanted: usize, work: usize) -> usize {
    if wanted > 0 {
        return wanted;
    }
    if work < 60_000 {
        return 1;
    }
    std::thread::available_parallelism().map_or(1, usize::from).clamp(1, 4)
}

/// Draws `mesh` as `camera` sees it. The picture is straight (not premultiplied) RGBA; where nothing was drawn
/// it is transparent.
pub fn render(mesh: &Mesh, camera: &Camera, params: &Params) -> RgbaImage {
    let out_w = params.width.max(1);
    let out_h = params.height.max(1);
    let mut ss = params.supersample.clamp(1, 3);
    if u64::from(out_w) * u64::from(out_h) * u64::from(ss * ss) > MAX_WORK_PIXELS {
        ss = 1;
    }
    let (w, h) = ((out_w * ss) as usize, (out_h * ss) as usize);
    let lens = Lens {
        basis: basis(camera, mesh, out_w as f32 / out_h as f32),
        focal: h as f32 / 2. / (FOV_Y / 2.).tan(),
        half_w: w as f32 / 2.,
        half_h: h as f32 / 2.,
        near: mesh.radius() * 0.002,
    };
    let stride = mesh.triangles().div_ceil(params.max_triangles.max(1)).max(1);
    let threads = thread_count(params.threads, mesh.triangles() / stride + w * h / 20).max(1);

    // Every vertex into the camera's space and onto the picture, once.
    let count = mesh.positions.len();
    let mut view = vec![[0f32; 3]; count];
    let mut proj = vec![Proj::default(); count];
    let per = count.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (n, (v, p)) in view.chunks_mut(per).zip(proj.chunks_mut(per)).enumerate() {
            let lens = &lens;
            if threads == 1 {
                project_range(mesh, lens, n * per, v, p);
            } else {
                scope.spawn(move || project_range(mesh, lens, n * per, v, p));
            }
        }
    });

    let scene = Scene {
        mesh,
        lens: &lens,
        view: &view,
        proj: &proj,
        stride,
        solid: params.color.map(f32::from),
        width: w,
    };
    let mut color = vec![0u8; w * h * 4];
    let mut depth = vec![0f32; w * h];
    let rows = h.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (n, (c, d)) in color
            .chunks_mut(rows * w * 4)
            .zip(depth.chunks_mut(rows * w))
            .enumerate()
        {
            let scene = &scene;
            let mut band = Band {
                first_row: n * rows,
                rows: d.len() / w,
                color: c,
                depth: d,
            };
            if threads == 1 {
                scene.draw(&mut band);
            } else {
                scope.spawn(move || scene.draw(&mut band));
            }
        }
    });
    if ss == 1 {
        return RgbaImage::from_raw(out_w, out_h, color).unwrap_or_else(|| RgbaImage::new(out_w, out_h));
    }
    downsample(&color, w, out_w as usize, out_h as usize, ss as usize)
}

/// Averages `ss` x `ss` samples to a pixel: the colour of the covered ones, the share covered as the opacity.
fn downsample(color: &[u8], width: usize, out_w: usize, out_h: usize, ss: usize) -> RgbaImage {
    let mut out = RgbaImage::new(out_w as u32, out_h as u32);
    let total = (ss * ss) as u32;
    for (x, y, pixel) in out.enumerate_pixels_mut() {
        let (mut r, mut g, mut b, mut covered) = (0u32, 0u32, 0u32, 0u32);
        for sy in 0..ss {
            for sx in 0..ss {
                let at = ((y as usize * ss + sy) * width + x as usize * ss + sx) * 4;
                if let Some([cr, cg, cb, ca]) = color.get(at..at + 4).and_then(|s| <[u8; 4]>::try_from(s).ok()) {
                    if ca > 0 {
                        r += u32::from(cr);
                        g += u32::from(cg);
                        b += u32::from(cb);
                        covered += 1;
                    }
                }
            }
        }
        if let (Some(r), Some(g), Some(b)) = (r.checked_div(covered), g.checked_div(covered), b.checked_div(covered)) {
            // Each is a mean of bytes, so it fits one.
            *pixel = image::Rgba([r as u8, g as u8, b as u8, (covered * 255 / total) as u8]);
        }
    }
    out
}

/// GPUI keeps pictures as BGRA: swaps red and blue in place.
pub fn to_bgra(image: &mut RgbaImage) {
    for pixel in image.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
}

#[cfg(test)]
mod tests {
    use super::super::model3d_load::load_obj;
    use super::super::model3d_load::tests::cube_obj;
    use super::*;

    fn cube() -> Mesh {
        load_obj(cube_obj().as_bytes()).expect("a cube")
    }

    fn params(width: u32, height: u32) -> Params {
        Params {
            width,
            height,
            supersample: 1,
            color: [200, 200, 220],
            max_triangles: usize::MAX,
            threads: 1,
        }
    }

    fn alpha(image: &RgbaImage, x: u32, y: u32) -> u8 {
        image.get_pixel(x, y)[3]
    }

    #[test]
    fn a_cube_fills_the_middle_and_leaves_the_corners_clear() {
        let image = render(&cube(), &Camera::home(), &params(200, 160));
        assert_eq!((image.width(), image.height()), (200, 160));
        let middle = image.get_pixel(100, 80);
        assert_eq!(middle[3], 255, "the middle is the cube");
        assert!(middle[0] > 40, "and it is lit: {middle:?}");
        for (x, y) in [(0, 0), (199, 0), (0, 159), (199, 159)] {
            assert_eq!(
                alpha(&image, x, y),
                0,
                "the corner {x},{y} shows the pane's own background"
            );
        }
        // The whole cube is in view (auto-fit): its edges stay inside the picture.
        let covered = image.pixels().filter(|p| p[3] > 0).count();
        assert!(covered > 2_000 && covered < 200 * 160, "{covered} pixels");
        for x in 0..200 {
            assert_eq!(alpha(&image, x, 0), 0);
            assert_eq!(alpha(&image, x, 159), 0);
        }
    }

    #[test]
    fn the_light_is_at_the_camera_so_faces_turned_away_are_darker() {
        // Looking straight down -z at a quad facing us, and at one tilted 60 degrees away.
        let facing = load_obj(b"v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\nf 1 2 3 4\n").expect("a quad");
        let head_on = Camera {
            yaw: 0.,
            pitch: 0.,
            zoom: 1.,
            pan: [0., 0.],
        };
        let flat = render(&facing, &head_on, &params(64, 64));
        let tilted = render(&facing, &Camera { yaw: 1.05, ..head_on }, &params(64, 64));
        let lit = |i: &RgbaImage| {
            let p = i.get_pixel(32, 32);
            u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])
        };
        assert!(
            lit(&flat) > lit(&tilted) + 60,
            "{} against {}",
            lit(&flat),
            lit(&tilted)
        );
        // A face seen from behind is lit as well as from the front (double sided).
        let behind = render(
            &facing,
            &Camera {
                yaw: std::f32::consts::PI,
                ..head_on
            },
            &params(64, 64),
        );
        assert_eq!(lit(&behind), lit(&flat));
    }

    #[test]
    fn turning_the_camera_changes_the_picture_and_zooming_changes_how_much_is_covered() {
        let mesh = cube();
        let home = render(&mesh, &Camera::home(), &params(120, 120));
        let turned = render(&mesh, &Camera::home().turned(0.7, 0.2), &params(120, 120));
        assert_ne!(home.as_raw(), turned.as_raw());
        let covered = |i: &RgbaImage| i.pixels().filter(|p| p[3] > 0).count();
        let near = render(&mesh, &Camera::home().zoomed(2.), &params(120, 120));
        assert!(
            covered(&near) > covered(&home) * 2,
            "{} against {}",
            covered(&near),
            covered(&home)
        );
        let far = render(&mesh, &Camera::home().zoomed(0.5), &params(120, 120));
        assert!(covered(&far) < covered(&home));
        // Panning moves the model across the picture.
        let moved = render(&mesh, &Camera::home().panned(1.5, 0.), &params(120, 120));
        assert_ne!(moved.as_raw(), home.as_raw());
    }

    #[test]
    fn a_wide_or_tall_picture_still_fits_the_whole_model() {
        let mesh = cube();
        for (w, h) in [(400u32, 100u32), (100, 400), (30, 30)] {
            let image = render(&mesh, &Camera::home(), &params(w, h));
            assert!(image.get_pixel(w / 2, h / 2)[3] > 0, "{w}x{h}");
            for x in 0..w {
                assert_eq!(alpha(&image, x, 0), 0, "{w}x{h}: the top row is clear");
                assert_eq!(alpha(&image, x, h - 1), 0, "{w}x{h}: the bottom row is clear");
            }
            for y in 0..h {
                assert_eq!(alpha(&image, 0, y), 0, "{w}x{h}: the left column is clear");
                assert_eq!(alpha(&image, w - 1, y), 0, "{w}x{h}: the right column is clear");
            }
        }
    }

    #[test]
    fn bands_drawn_by_several_threads_are_the_same_picture() {
        let mesh = cube();
        let one = render(&mesh, &Camera::home(), &params(150, 97));
        for threads in [2, 3, 4] {
            let many = render(
                &mesh,
                &Camera::home(),
                &Params {
                    threads,
                    ..params(150, 97)
                },
            );
            assert_eq!(one.as_raw(), many.as_raw(), "{threads} threads");
        }
    }

    #[test]
    fn supersampling_softens_edges_and_keeps_the_inside_solid() {
        let mesh = cube();
        let rough = render(&mesh, &Camera::home(), &params(100, 100));
        let smooth = render(
            &mesh,
            &Camera::home(),
            &Params {
                supersample: 2,
                ..params(100, 100)
            },
        );
        assert_eq!((smooth.width(), smooth.height()), (100, 100));
        assert!(rough.pixels().all(|p| p[3] == 0 || p[3] == 255));
        assert!(
            smooth.pixels().any(|p| p[3] > 0 && p[3] < 255),
            "edge pixels are partly covered"
        );
        assert_eq!(smooth.get_pixel(50, 50)[3], 255);
    }

    #[test]
    fn a_camera_inside_the_model_clips_at_the_near_plane_instead_of_failing() {
        let mesh = cube();
        // The most it can zoom: the eye is a hair from the cube's middle.
        let inside = render(&mesh, &Camera::home().zoomed(1000.), &params(80, 80));
        assert_eq!((inside.width(), inside.height()), (80, 80));
        // Pan the cube across the eye so that its faces straddle the near plane.
        for across in [0.4, 0.9, 1.2] {
            let _ = render(&mesh, &Camera::home().zoomed(60.).panned(across, 0.1), &params(80, 80));
        }
    }

    #[test]
    fn colours_in_the_file_replace_the_models_colour() {
        let mut mesh = load_obj(b"v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\nf 1 2 3 4\n").expect("a quad");
        mesh.colors = Some(vec![[255, 0, 0]; 4]);
        let head_on = Camera {
            yaw: 0.,
            pitch: 0.,
            zoom: 1.,
            pan: [0., 0.],
        };
        let image = render(&mesh, &head_on, &params(64, 64));
        let p = image.get_pixel(32, 32);
        assert!(p[0] > 200 && p[1] < 10 && p[2] < 10, "{p:?}");
    }

    #[test]
    fn a_big_mesh_drawn_for_a_drag_skips_triangles_but_still_shows_the_model() {
        let mesh = cube();
        let full = render(&mesh, &Camera::home(), &params(100, 100));
        let draft = render(
            &mesh,
            &Camera::home(),
            &Params {
                max_triangles: 4,
                ..params(50, 50)
            },
        );
        assert_eq!((draft.width(), draft.height()), (50, 50));
        assert!(draft.pixels().any(|p| p[3] > 0));
        let covered = |i: &RgbaImage| i.pixels().filter(|p| p[3] > 0).count() as f32 / (i.width() * i.height()) as f32;
        assert!(covered(&draft) < covered(&full));
    }

    #[test]
    fn nothing_breaks_for_an_empty_picture_or_a_camera_straight_up() {
        let mesh = cube();
        assert_eq!(render(&mesh, &Camera::home(), &params(0, 0)).dimensions(), (1, 1));
        let up = Camera::home().turned(0., 10.);
        assert!(up.pitch < std::f32::consts::FRAC_PI_2);
        let _ = render(&mesh, &up, &params(40, 40));
        let down = Camera::home().turned(0., -10.);
        let _ = render(&mesh, &down, &params(40, 40));
    }

    /// A sphere of `n` x `n` quads, smooth shaded.
    fn sphere(n: u32) -> Mesh {
        use super::super::model3d_mesh::{assemble, Normals};
        let mut positions = Vec::new();
        for i in 0..=n {
            let v = std::f32::consts::PI * i as f32 / n as f32;
            for j in 0..=n {
                let u = std::f32::consts::TAU * j as f32 / n as f32;
                positions.push([v.sin() * u.cos(), v.cos(), v.sin() * u.sin()]);
            }
        }
        let mut indices = Vec::new();
        for i in 0..n {
            for j in 0..n {
                let a = i * (n + 1) + j;
                let b = a + n + 1;
                indices.extend([a, b, a + 1, a + 1, b, b + 1]);
            }
        }
        assemble(positions, indices, Normals::Smooth, None).expect("a sphere")
    }

    #[test]
    fn a_sphere_is_lit_brightest_in_the_middle_and_darker_toward_its_edge() {
        let image = render(&sphere(40), &Camera::home(), &params(160, 160));
        let lum = |x: u32, y: u32| {
            let p = image.get_pixel(x, y);
            u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])
        };
        let edge = (0..160)
            .find(|x| image.get_pixel(*x, 80)[3] > 0)
            .expect("the sphere's left edge");
        assert!(
            lum(80, 80) > lum(edge + 3, 80) + 100,
            "{} against {}",
            lum(80, 80),
            lum(edge + 3, 80)
        );
    }

    /// `cargo test -p glint-ui --features preview-3d --release -- --ignored --nocapture sphere_timings`
    #[test]
    #[ignore = "a timing, not a check: run it by hand with --release"]
    fn sphere_timings() {
        let mesh = sphere(500);
        println!("{} triangles", mesh.triangles());
        for (label, params) in [
            (
                "full 1400 x 1000, 2 x 2",
                Params {
                    supersample: 2,
                    threads: 0,
                    ..params(1400, 1000)
                },
            ),
            (
                "draft 700 x 500",
                Params {
                    max_triangles: 150_000,
                    threads: 0,
                    ..params(700, 500)
                },
            ),
            (
                "draft 700 x 500, one thread",
                Params {
                    max_triangles: 150_000,
                    ..params(700, 500)
                },
            ),
        ] {
            let started = std::time::Instant::now();
            let image = render(&mesh, &Camera::home(), &params);
            println!(
                "{label}: {:?} ({}x{})",
                started.elapsed(),
                image.width(),
                image.height()
            );
        }
    }

    #[test]
    fn the_camera_keeps_to_its_limits() {
        let c = Camera::home();
        assert_eq!(c.zoomed(1e9).zoom, MAX_ZOOM);
        assert_eq!(c.zoomed(1e-9).zoom, MIN_ZOOM);
        assert!(c.turned(100., 0.).yaw.abs() <= std::f32::consts::PI);
        assert_eq!(c.panned(1e9, -1e9).pan, [MAX_PAN, -MAX_PAN]);
        let mut bgra = RgbaImage::from_raw(1, 1, vec![1, 2, 3, 4]).expect("a pixel");
        to_bgra(&mut bgra);
        assert_eq!(bgra.as_raw(), &[3, 2, 1, 4]);
    }
}
