//! The loaders for models made of a scene: glTF 2.0 (`.gltf` with its buffers, `.glb`) and 3MF.
//!
//! three.js had `GLTFLoader` and `ThreeMFLoader` for these (ModelView.tsx). Here both are flattened into the one
//! [`Mesh`], each instance of a mesh in the scene placed by the transforms above it.
//!
//! **glTF.** Reads triangle primitives (`TRIANGLES`, `TRIANGLE_STRIP`, `TRIANGLE_FAN`): `POSITION`, `NORMAL` and
//! the indices, from any of the component types (with `normalized`, so `KHR_mesh_quantization` reads), through
//! the buffer views' strides. Node `matrix` or translation, rotation and scale. Buffers are `data:` addresses, a
//! `.glb`'s binary chunk, or files next to the `.gltf` that the caller's resolver reads. A primitive without
//! normals is lit in facets, as the glTF specification says.
//!
//! **3MF.** A zip whose `3D/3dmodel.model` (or the one `_rels/.rels` points to) holds objects with vertices and
//! triangles, components that place other objects (also those in other model files of the production extension,
//! which PrusaSlicer and Bambu Studio write) and the build items that say what is on the plate. 3MF is z-up and
//! the camera y-up, so the model is stood up as `ThreeMFLoader` did. Lit in facets.
//!
//! Not read: materials, textures, animations, skins and morph targets, sparse accessors (their base values are
//! used), Draco and meshopt compression (the model is reported as compressed), 3MF materials and units.

use super::model3d_mesh::{assemble, cross, normalize, sub, LoadError, Mesh, Normals, MAX_BYTES, MAX_TRIANGLES, V3};
use crate::components::preview::markdown::base64_decode;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Read;
use std::rc::Rc;

/// A 4 x 4 matrix, column by column (glTF's order): element `(row, col)` is `m[col * 4 + row]`.
type Mat = [f32; 16];

const IDENTITY: Mat = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.];

/// The matrix product `a * b`: a point goes through `b` first, then `a`.
fn mul(a: &Mat, b: &Mat) -> Mat {
    let mut out = [0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

fn point(m: &Mat, p: V3) -> V3 {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

/// The matrix that turns normals the way `m` turns points (the cofactor matrix of its 3 x 3, which is the inverse
/// transpose times the determinant), and whether `m` mirrors, which turns triangles inside out.
fn normal_matrix(m: &Mat) -> ([f32; 9], bool) {
    let a = |r: usize, c: usize| m[c * 4 + r];
    let cof = |r: usize, c: usize| {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (c1, c2) = ((c + 1) % 3, (c + 2) % 3);
        a(r1, c1) * a(r2, c2) - a(r1, c2) * a(r2, c1)
    };
    let mut out = [0f32; 9];
    for r in 0..3 {
        for c in 0..3 {
            out[r * 3 + c] = cof(r, c);
        }
    }
    let det = a(0, 0) * out[0] + a(0, 1) * out[1] + a(0, 2) * out[2];
    let mirrored = det < 0.;
    if mirrored {
        out.iter_mut().for_each(|v| *v = -*v);
    }
    (out, mirrored)
}

fn turn(n: &[f32; 9], v: V3) -> V3 {
    [
        n[0] * v[0] + n[1] * v[1] + n[2] * v[2],
        n[3] * v[0] + n[4] * v[1] + n[5] * v[2],
        n[6] * v[0] + n[7] * v[1] + n[8] * v[2],
    ]
}

/// Translation, rotation (a unit quaternion x, y, z, w) and scale as one matrix.
fn from_trs(t: V3, q: [f32; 4], s: V3) -> Mat {
    let [x, y, z, w] = q;
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (wx, wy, wz) = (w * x, w * y, w * z);
    [
        (1. - 2. * (yy + zz)) * s[0],
        (2. * (xy + wz)) * s[0],
        (2. * (xz - wy)) * s[0],
        0.,
        (2. * (xy - wz)) * s[1],
        (1. - 2. * (xx + zz)) * s[1],
        (2. * (yz + wx)) * s[1],
        0.,
        (2. * (xz + wy)) * s[2],
        (2. * (yz - wx)) * s[2],
        (1. - 2. * (xx + yy)) * s[2],
        0.,
        t[0],
        t[1],
        t[2],
        1.,
    ]
}

// ---- glTF -------------------------------------------------------------------------------------------------

fn array<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn number(v: &Value, key: &str) -> Option<usize> {
    v.get(key)?.as_u64().and_then(|n| usize::try_from(n).ok())
}

fn floats<const N: usize>(v: &Value, key: &str) -> Option<[f32; N]> {
    let list = v.get(key)?.as_array()?;
    if list.len() != N {
        return None;
    }
    let mut out = [0f32; N];
    for (slot, value) in out.iter_mut().zip(list) {
        *slot = value.as_f64()? as f32;
    }
    Some(out)
}

#[derive(Clone, Copy)]
enum Comp {
    I8,
    U8,
    I16,
    U16,
    U32,
    F32,
}

impl Comp {
    fn of(code: u64) -> Option<Comp> {
        Some(match code {
            5120 => Comp::I8,
            5121 => Comp::U8,
            5122 => Comp::I16,
            5123 => Comp::U16,
            5125 => Comp::U32,
            5126 => Comp::F32,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Comp::I8 | Comp::U8 => 1,
            Comp::I16 | Comp::U16 => 2,
            Comp::U32 | Comp::F32 => 4,
        }
    }
}

/// Where an accessor's elements are, checked against the buffer when it is made.
struct Accessor<'d> {
    data: &'d [u8],
    start: usize,
    stride: usize,
    count: usize,
    comp: Comp,
    normalized: bool,
    width: usize,
    /// No buffer view: the specification says zeros.
    zero: bool,
}

impl Accessor<'_> {
    fn bytes(&self, element: usize, component: usize) -> Option<&[u8]> {
        let at = self
            .start
            .checked_add(element.checked_mul(self.stride)?)?
            .checked_add(component * self.comp.size())?;
        self.data.get(at..at.checked_add(self.comp.size())?)
    }

    fn float(&self, element: usize, component: usize) -> f32 {
        if self.zero {
            return 0.;
        }
        let Some(b) = self.bytes(element, component) else {
            return 0.;
        };
        match self.comp {
            Comp::I8 => {
                let v = f32::from(i8::from_le_bytes([b[0]]));
                if self.normalized {
                    (v / 127.).max(-1.)
                } else {
                    v
                }
            }
            Comp::U8 => {
                let v = f32::from(b[0]);
                if self.normalized {
                    v / 255.
                } else {
                    v
                }
            }
            Comp::I16 => {
                let v = f32::from(i16::from_le_bytes([b[0], b[1]]));
                if self.normalized {
                    (v / 32767.).max(-1.)
                } else {
                    v
                }
            }
            Comp::U16 => {
                let v = f32::from(u16::from_le_bytes([b[0], b[1]]));
                if self.normalized {
                    v / 65535.
                } else {
                    v
                }
            }
            Comp::U32 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32,
            Comp::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        }
    }

    fn index(&self, element: usize) -> u32 {
        if self.zero {
            return 0;
        }
        let Some(b) = self.bytes(element, 0) else {
            return 0;
        };
        match self.comp {
            Comp::U8 | Comp::I8 => u32::from(b[0]),
            Comp::U16 | Comp::I16 => u32::from(u16::from_le_bytes([b[0], b[1]])),
            Comp::U32 | Comp::F32 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        }
    }
}

/// A primitive's triangles, in the mesh's own space.
struct Prim {
    positions: Vec<V3>,
    normals: Option<Vec<V3>>,
    indices: Vec<u32>,
}

struct Doc<'a> {
    json: Value,
    buffers: Vec<Option<Cow<'a, [u8]>>>,
    prims: HashMap<(usize, usize), Option<Rc<Prim>>>,
}

impl<'a> Doc<'a> {
    fn accessor(&self, index: usize) -> Option<Accessor<'_>> {
        let accessor = array(&self.json, "accessors").get(index)?;
        let comp = Comp::of(accessor.get("componentType")?.as_u64()?)?;
        let count = number(accessor, "count")?;
        let width = match accessor.get("type")?.as_str()? {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" | "MAT2" => 4,
            "MAT3" => 9,
            "MAT4" => 16,
            _ => return None,
        };
        let normalized = accessor.get("normalized").and_then(Value::as_bool).unwrap_or(false);
        let Some(view) = number(accessor, "bufferView") else {
            return Some(Accessor {
                data: &[],
                start: 0,
                stride: 0,
                count,
                comp,
                normalized,
                width,
                zero: true,
            });
        };
        let view = array(&self.json, "bufferViews").get(view)?;
        let buffer = self.buffers.get(number(view, "buffer")?)?.as_deref()?;
        let element = comp.size().checked_mul(width)?;
        let stride = number(view, "byteStride").filter(|s| *s >= element).unwrap_or(element);
        let start = number(view, "byteOffset")
            .unwrap_or(0)
            .checked_add(number(accessor, "byteOffset").unwrap_or(0))?;
        let view_end = number(view, "byteOffset")
            .unwrap_or(0)
            .checked_add(number(view, "byteLength")?)?
            .min(buffer.len());
        if count > 0 {
            let last = start
                .checked_add((count - 1).checked_mul(stride)?)?
                .checked_add(element)?;
            if last > view_end {
                return None;
            }
        }
        Some(Accessor {
            data: buffer,
            start,
            stride,
            count,
            comp,
            normalized,
            width,
            zero: false,
        })
    }

    fn vec3s(&self, index: usize) -> Option<Vec<V3>> {
        let accessor = self.accessor(index)?;
        if accessor.width != 3 || accessor.count > MAX_TRIANGLES * 3 {
            return None;
        }
        Some(
            (0..accessor.count)
                .map(|i| [accessor.float(i, 0), accessor.float(i, 1), accessor.float(i, 2)])
                .collect(),
        )
    }

    /// The triangles of primitive `p` of mesh `m` (once; instances share it).
    fn prim(&mut self, m: usize, p: usize) -> Option<Rc<Prim>> {
        if let Some(found) = self.prims.get(&(m, p)) {
            return found.clone();
        }
        let made = self.read_prim(m, p).map(Rc::new);
        self.prims.insert((m, p), made.clone());
        made
    }

    fn read_prim(&self, m: usize, p: usize) -> Option<Prim> {
        let primitive = array(array(&self.json, "meshes").get(m)?, "primitives").get(p)?;
        let attributes = primitive.get("attributes")?;
        let positions = self.vec3s(number(attributes, "POSITION")?)?;
        let normals = number(attributes, "NORMAL")
            .and_then(|n| self.vec3s(n))
            .filter(|n| n.len() == positions.len());
        let sequence: Vec<u32>;
        let source: Vec<u32> = match number(primitive, "indices") {
            Some(ix) => {
                let accessor = self.accessor(ix)?;
                if accessor.width != 1 || accessor.count > MAX_TRIANGLES * 3 {
                    return None;
                }
                (0..accessor.count).map(|i| accessor.index(i)).collect()
            }
            None => {
                sequence = (0..u32::try_from(positions.len()).ok()?).collect();
                sequence
            }
        };
        let mut indices = Vec::new();
        match primitive.get("mode").and_then(Value::as_u64).unwrap_or(4) {
            4 => indices.extend_from_slice(&source[..source.len() / 3 * 3]),
            5 => {
                for (k, w) in source.windows(3).enumerate() {
                    if k % 2 == 0 {
                        indices.extend([w[0], w[1], w[2]]);
                    } else {
                        indices.extend([w[1], w[0], w[2]]);
                    }
                }
            }
            6 => {
                if let Some((first, rest)) = source.split_first() {
                    for pair in rest.windows(2) {
                        indices.extend([*first, pair[0], pair[1]]);
                    }
                }
            }
            _ => return None,
        }
        let count = u32::try_from(positions.len()).ok()?;
        let drawable = indices.as_chunks::<3>().0.iter().all(|t| t.iter().all(|i| *i < count));
        if !drawable {
            let mut kept = Vec::with_capacity(indices.len());
            for t in indices.as_chunks::<3>().0 {
                if t.iter().all(|i| *i < count) {
                    kept.extend_from_slice(t);
                }
            }
            indices = kept;
        }
        Some(Prim {
            positions,
            normals,
            indices,
        })
    }

    /// The node's own matrix.
    fn local(&self, node: &Value) -> Mat {
        if let Some(m) = floats::<16>(node, "matrix") {
            return m;
        }
        from_trs(
            floats::<3>(node, "translation").unwrap_or([0.; 3]),
            floats::<4>(node, "rotation").unwrap_or([0., 0., 0., 1.]),
            floats::<3>(node, "scale").unwrap_or([1.; 3]),
        )
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |k: usize| bytes.get(k).and_then(|b| (*b as char).to_digit(16));
        if bytes[i] == b'%' {
            if let (Some(hi), Some(lo)) = (hex(i + 1), hex(i + 2)) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The chunks of a `.glb`: the JSON and, if there is one, the binary.
fn glb_chunks(bytes: &[u8]) -> Option<(&[u8], Option<&[u8]>)> {
    let word = |at: usize| {
        bytes
            .get(at..at + 4)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map(u32::from_le_bytes)
    };
    if word(4)? != 2 {
        return None;
    }
    let (mut json, mut bin) = (None, None);
    let mut at = 12;
    while let (Some(length), Some(kind)) = (word(at), word(at + 4)) {
        let length = usize::try_from(length).ok()?;
        let data = bytes.get(at + 8..(at + 8).checked_add(length)?)?;
        match kind {
            0x4E4F_534A => json = json.or(Some(data)),
            0x004E_4942 => bin = bin.or(Some(data)),
            _ => {}
        }
        at = (at + 8).checked_add(length.checked_add(3)? / 4 * 4)?;
    }
    Some((json?, bin))
}

/// A glTF or glb file. `resolve` reads a file named in the model, relative to the model's folder, and gives why
/// it can't.
pub fn load_gltf(bytes: &[u8], resolve: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> Result<Mesh, LoadError> {
    let (json, bin) = if bytes.starts_with(b"glTF") {
        let (json, bin) = glb_chunks(bytes).ok_or(LoadError::Damaged("a damaged glb"))?;
        (json, bin)
    } else {
        (bytes, None)
    };
    let json: Value = serde_json::from_slice(json).map_err(|_| LoadError::Damaged("not a glTF file"))?;
    if json
        .get("asset")
        .and_then(|a| a.get("version"))
        .and_then(Value::as_str)
        .is_none_or(|v| !v.starts_with('2'))
    {
        return Err(LoadError::Damaged("not glTF 2.0"));
    }
    let needs = |name: &str| {
        array(&json, "extensionsRequired")
            .iter()
            .any(|e| e.as_str() == Some(name))
    };
    if needs("KHR_draco_mesh_compression") || needs("EXT_meshopt_compression") {
        return Err(LoadError::Compressed);
    }
    let mut loaded = 0usize;
    let mut first_failure: Option<String> = None;
    let mut buffers: Vec<Option<Cow<[u8]>>> = Vec::new();
    for (i, buffer) in array(&json, "buffers").iter().enumerate() {
        let data: Option<Cow<[u8]>> = match buffer.get("uri").and_then(Value::as_str) {
            Some(uri) if uri.starts_with("data:") => uri
                .split_once(',')
                .filter(|(head, _)| head.contains("base64"))
                .and_then(|(_, body)| base64_decode(body))
                .map(Cow::Owned),
            // Only files next to the model, by relative name.
            Some(uri) if uri.contains(':') || uri.starts_with('/') || uri.starts_with('\\') => None,
            Some(uri) => match resolve(&percent_decode(uri)) {
                Ok(data) => Some(Cow::Owned(data)),
                Err(reason) => {
                    first_failure.get_or_insert(reason);
                    None
                }
            },
            None if i == 0 => bin.map(Cow::Borrowed),
            None => None,
        };
        loaded = loaded.saturating_add(data.as_ref().map_or(0, |d| d.len()));
        if loaded as u64 > MAX_BYTES {
            return Err(LoadError::TooBig);
        }
        buffers.push(data);
    }
    let mut doc = Doc {
        json,
        buffers,
        prims: HashMap::new(),
    };
    // What to draw: the scene's nodes, else every node nobody is a child of.
    let nodes = array(&doc.json, "nodes").to_vec();
    let scene = number(&doc.json, "scene").unwrap_or(0);
    let mut roots: Vec<usize> = array(&doc.json, "scenes")
        .get(scene)
        .map(|s| {
            array(s, "nodes")
                .iter()
                .filter_map(|n| n.as_u64().and_then(|n| usize::try_from(n).ok()))
                .collect()
        })
        .unwrap_or_default();
    if roots.is_empty() {
        let children: Vec<usize> = nodes
            .iter()
            .flat_map(|n| array(n, "children"))
            .filter_map(|c| c.as_u64().and_then(|c| usize::try_from(c).ok()))
            .collect();
        roots = (0..nodes.len()).filter(|n| !children.contains(n)).collect();
    }
    // Every placement of a primitive: its triangles and where they stand.
    let mut placed: Vec<(Rc<Prim>, Mat)> = Vec::new();
    let mut triangles = 0usize;
    let mut stack: Vec<(usize, Mat, usize)> = roots.into_iter().rev().map(|n| (n, IDENTITY, 0)).collect();
    if nodes.is_empty() {
        // No nodes at all: every mesh where it is.
        for m in 0..array(&doc.json, "meshes").len() {
            for p in 0..array(&array(&doc.json, "meshes")[m], "primitives").len() {
                if let Some(prim) = doc.prim(m, p) {
                    triangles += prim.indices.len() / 3;
                    placed.push((prim, IDENTITY));
                }
            }
        }
    }
    while let Some((n, parent, depth)) = stack.pop() {
        let Some(node) = nodes.get(n) else { continue };
        let world = mul(&parent, &doc.local(node));
        if let Some(m) = number(node, "mesh") {
            let count = array(array(&doc.json, "meshes").get(m).unwrap_or(&Value::Null), "primitives").len();
            for p in 0..count {
                if let Some(prim) = doc.prim(m, p) {
                    triangles += prim.indices.len() / 3;
                    placed.push((prim, world));
                }
            }
        }
        if triangles > MAX_TRIANGLES || placed.len() > 200_000 {
            return Err(LoadError::TooBig);
        }
        if depth < 64 {
            for child in array(node, "children").iter().rev() {
                if let Some(child) = child.as_u64().and_then(|c| usize::try_from(c).ok()) {
                    stack.push((child, world, depth + 1));
                }
            }
        }
    }
    if placed.is_empty() {
        return Err(match first_failure {
            Some(reason) => LoadError::Unreadable(reason),
            None => LoadError::Empty,
        });
    }
    // Normals: the file's where there are any, facets where a primitive has none among primitives that do.
    let any_normals = placed.iter().any(|(prim, _)| prim.normals.is_some());
    let mut positions: Vec<V3> = Vec::new();
    let mut normals: Vec<V3> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for (prim, world) in &placed {
        let (turner, mirrored) = normal_matrix(world);
        let base = u32::try_from(positions.len()).map_err(|_| LoadError::TooBig)?;
        let flip = |t: &[u32]| {
            if mirrored {
                [t[0], t[2], t[1]]
            } else {
                [t[0], t[1], t[2]]
            }
        };
        match (&prim.normals, any_normals) {
            (Some(own), _) => {
                positions.extend(prim.positions.iter().map(|p| point(world, *p)));
                normals.extend(own.iter().map(|n| normalize(turn(&turner, *n))));
                for t in prim.indices.as_chunks::<3>().0 {
                    indices.extend(flip(t).map(|i| base + i));
                }
            }
            (None, true) => {
                // Unshared corners, each with its triangle's normal.
                for t in prim.indices.as_chunks::<3>().0 {
                    let corners = [0, 1, 2].map(|k| prim.positions.get(t[k] as usize).copied().unwrap_or([0.; 3]));
                    let face = normalize(turn(
                        &turner,
                        cross(sub(corners[1], corners[0]), sub(corners[2], corners[0])),
                    ));
                    let first = u32::try_from(positions.len()).map_err(|_| LoadError::TooBig)?;
                    for corner in corners {
                        positions.push(point(world, corner));
                        normals.push(face);
                    }
                    indices.extend(flip(&[0, 1, 2]).map(|i| first + i));
                }
            }
            (None, false) => {
                positions.extend(prim.positions.iter().map(|p| point(world, *p)));
                for t in prim.indices.as_chunks::<3>().0 {
                    indices.extend(flip(t).map(|i| base + i));
                }
            }
        }
    }
    assemble(
        positions,
        indices,
        if any_normals {
            Normals::Given(normals)
        } else {
            Normals::Flat
        },
        None,
    )
}

// ---- 3MF --------------------------------------------------------------------------------------------------

/// A 3MF `transform` (twelve numbers: three rows of the 3 x 3, then the offset; points are rows) as a matrix.
fn threemf_matrix(text: Option<&str>) -> Mat {
    let Some(text) = text else { return IDENTITY };
    let t: Vec<f32> = text.split_whitespace().filter_map(|w| w.parse().ok()).collect();
    if t.len() != 12 {
        return IDENTITY;
    }
    [
        t[0], t[1], t[2], 0., t[3], t[4], t[5], 0., t[6], t[7], t[8], 0., t[9], t[10], t[11], 1.,
    ]
}

struct Part {
    positions: Vec<V3>,
    triangles: Vec<[u32; 3]>,
}

struct Component {
    object: u32,
    path: Option<String>,
    transform: Mat,
}

#[derive(Default)]
struct Object {
    part: Option<Part>,
    components: Vec<Component>,
}

#[derive(Default)]
struct Model {
    objects: HashMap<u32, Object>,
    build: Vec<Component>,
}

fn attr<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes().find(|a| a.name() == name).map(|a| a.value())
}

fn children<'a, 'i>(
    node: roxmltree::Node<'a, 'i>,
    name: &'static str,
) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> {
    node.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

fn component_of(node: roxmltree::Node) -> Option<Component> {
    Some(Component {
        object: attr(node, "objectid")?.trim().parse().ok()?,
        path: attr(node, "path").map(|p| p.trim_start_matches('/').to_string()),
        transform: threemf_matrix(attr(node, "transform")),
    })
}

fn parse_model(bytes: &[u8]) -> Option<Model> {
    let text = std::str::from_utf8(bytes).ok()?;
    let doc = roxmltree::Document::parse(text).ok()?;
    let root = doc.root_element();
    let mut model = Model::default();
    for resources in children(root, "resources") {
        for object in children(resources, "object") {
            let Some(id) = attr(object, "id").and_then(|i| i.trim().parse::<u32>().ok()) else {
                continue;
            };
            let mut out = Object::default();
            if let Some(mesh) = children(object, "mesh").next() {
                let number = |n: roxmltree::Node, name: &str| attr(n, name).and_then(|v| v.trim().parse::<f32>().ok());
                let positions = children(mesh, "vertices")
                    .flat_map(|v| children(v, "vertex"))
                    .map(|v| {
                        [
                            number(v, "x").unwrap_or(f32::NAN),
                            number(v, "y").unwrap_or(f32::NAN),
                            number(v, "z").unwrap_or(f32::NAN),
                        ]
                    })
                    .collect();
                let index = |n: roxmltree::Node, name: &str| attr(n, name).and_then(|v| v.trim().parse::<u32>().ok());
                let triangles = children(mesh, "triangles")
                    .flat_map(|t| children(t, "triangle"))
                    .map(|t| {
                        [
                            index(t, "v1").unwrap_or(u32::MAX),
                            index(t, "v2").unwrap_or(u32::MAX),
                            index(t, "v3").unwrap_or(u32::MAX),
                        ]
                    })
                    .collect();
                out.part = Some(Part { positions, triangles });
            }
            out.components = children(object, "components")
                .flat_map(|c| children(c, "component"))
                .filter_map(component_of)
                .collect();
            model.objects.insert(id, out);
        }
    }
    for build in children(root, "build") {
        model.build.extend(children(build, "item").filter_map(component_of));
    }
    Some(model)
}

/// What a 3MF file's expansion collects.
struct Plate<'z, R: std::io::Read + std::io::Seek> {
    zip: &'z mut zip::ZipArchive<R>,
    models: HashMap<String, Option<Rc<Model>>>,
    positions: Vec<V3>,
    indices: Vec<u32>,
    failure: Option<LoadError>,
}

fn entry_bytes<R: std::io::Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Option<Vec<u8>> {
    let wanted = name.trim_start_matches('/').to_ascii_lowercase();
    let index = (0..zip.len()).find(|&i| {
        zip.by_index_raw(i).is_ok_and(|f| {
            f.name()
                .is_ok_and(|n| n.replace('\\', "/").to_ascii_lowercase() == wanted)
        })
    })?;
    let file = zip.by_index(index).ok()?;
    let mut out = Vec::new();
    file.take(MAX_BYTES).read_to_end(&mut out).ok()?;
    Some(out)
}

impl<R: std::io::Read + std::io::Seek> Plate<'_, R> {
    fn model(&mut self, path: &str) -> Option<Rc<Model>> {
        if let Some(found) = self.models.get(path) {
            return found.clone();
        }
        let parsed = entry_bytes(self.zip, path).and_then(|b| parse_model(&b)).map(Rc::new);
        self.models.insert(path.to_string(), parsed.clone());
        parsed
    }

    /// Puts object `id` of the model at `path`, placed by `world`, on the plate.
    fn place(&mut self, path: &str, id: u32, world: &Mat, depth: usize) {
        if depth > 16 || self.failure.is_some() {
            return;
        }
        let Some(model) = self.model(path) else { return };
        let Some(object) = model.objects.get(&id) else { return };
        if let Some(part) = &object.part {
            let Ok(base) = u32::try_from(self.positions.len()) else {
                self.failure = Some(LoadError::TooBig);
                return;
            };
            // z-up to y-up: (x, y, z) becomes (x, z, -y).
            self.positions.extend(part.positions.iter().map(|p| {
                let q = point(world, *p);
                [q[0], q[2], -q[1]]
            }));
            let count = u32::try_from(part.positions.len()).unwrap_or(0);
            for t in &part.triangles {
                if t.iter().all(|v| *v < count) {
                    self.indices.extend(t.iter().map(|v| base + v));
                }
            }
            if self.indices.len() / 3 > MAX_TRIANGLES {
                self.failure = Some(LoadError::TooBig);
                return;
            }
        }
        for component in &object.components {
            let inner = component.path.as_deref().unwrap_or(path).to_string();
            self.place(&inner, component.object, &mul(world, &component.transform), depth + 1);
        }
    }
}

/// A 3MF file.
pub fn load_3mf(bytes: &[u8]) -> Result<Mesh, LoadError> {
    let mut zip =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| LoadError::Damaged("not a 3MF file"))?;
    // The model is where the package's relationships say, else where everyone puts it.
    let main = entry_bytes(&mut zip, "_rels/.rels")
        .and_then(|rels| {
            let text = String::from_utf8(rels).ok()?;
            let doc = roxmltree::Document::parse(&text).ok()?;
            doc.descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "Relationship")
                .find(|n| attr(*n, "Type").is_some_and(|t| t.ends_with("3dmodel")))
                .and_then(|n| attr(n, "Target").map(|t| t.trim_start_matches('/').to_string()))
        })
        .unwrap_or_else(|| "3D/3dmodel.model".to_string());
    let mut plate = Plate {
        zip: &mut zip,
        models: HashMap::new(),
        positions: Vec::new(),
        indices: Vec::new(),
        failure: None,
    };
    let model = plate
        .model(&main)
        .ok_or(LoadError::Damaged("no model in the 3MF file"))?;
    if model.build.is_empty() {
        // No plate described: every object that is a mesh of its own.
        let mut ids: Vec<u32> = model
            .objects
            .iter()
            .filter(|(_, o)| o.part.is_some())
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        for id in ids {
            plate.place(&main, id, &IDENTITY, 0);
        }
    } else {
        for item in &model.build {
            let path = item.path.clone().unwrap_or_else(|| main.clone());
            plate.place(&path, item.object, &item.transform, 0);
        }
    }
    if let Some(failure) = plate.failure.take() {
        return Err(failure);
    }
    assemble(
        std::mem::take(&mut plate.positions),
        std::mem::take(&mut plate.indices),
        Normals::Flat,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::super::model3d_load::tests::cube;
    use super::super::model3d_mesh::Shading;
    use super::*;
    use std::io::Write;

    fn base64(bytes: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
            for k in 0..4 {
                if k <= chunk.len() {
                    out.push(A[(n >> (18 - 6 * k) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// The cube's buffer: 8 positions (f32), 36 indices (u16), and with `normals` 8 normals straight up (f32).
    fn cube_buffer(normals: bool) -> Vec<u8> {
        let (corners, triangles) = cube();
        let mut out = Vec::new();
        for c in &corners {
            for v in c {
                out.extend(v.to_le_bytes());
            }
        }
        for t in triangles {
            for i in t {
                out.extend((i as u16).to_le_bytes());
            }
        }
        if normals {
            for _ in &corners {
                for v in [0f32, 1., 0.] {
                    out.extend(v.to_le_bytes());
                }
            }
        }
        out
    }

    fn data_uri(normals: bool) -> String {
        format!("data:application/octet-stream;base64,{}", base64(&cube_buffer(normals)))
    }

    /// A glTF of the cube: `nodes` is the JSON of the nodes, `roots` the scene's list.
    fn gltf(uri: Option<&str>, nodes: &str, roots: &str, normals: bool) -> String {
        let uri = uri.map_or(String::new(), |u| format!(r#","uri":"{u}""#));
        let attributes = if normals {
            r#""POSITION":0,"NORMAL":2"#
        } else {
            r#""POSITION":0"#
        };
        let length = 96 + 72 + if normals { 96 } else { 0 };
        format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[{roots}]}}],"nodes":[{nodes}],
            "meshes":[{{"primitives":[{{"attributes":{{{attributes}}},"indices":1}}]}}],
            "buffers":[{{"byteLength":{length}{uri}}}],
            "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":96}},{{"buffer":0,"byteOffset":96,"byteLength":72}},
                           {{"buffer":0,"byteOffset":168,"byteLength":96}}],
            "accessors":[{{"bufferView":0,"componentType":5126,"count":8,"type":"VEC3"}},
                         {{"bufferView":1,"componentType":5123,"count":36,"type":"SCALAR"}},
                         {{"bufferView":2,"componentType":5126,"count":8,"type":"VEC3"}}]}}"#
        )
    }

    fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
        let mut json = json.as_bytes().to_vec();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let mut bin = bin.to_vec();
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let total = 12 + 8 + json.len() + 8 + bin.len();
        let mut out = b"glTF".to_vec();
        out.extend(2u32.to_le_bytes());
        out.extend((total as u32).to_le_bytes());
        out.extend((json.len() as u32).to_le_bytes());
        out.extend(0x4E4F_534Au32.to_le_bytes());
        out.extend(json);
        out.extend((bin.len() as u32).to_le_bytes());
        out.extend(0x004E_4942u32.to_le_bytes());
        out.extend(bin);
        out
    }

    fn nothing(_: &str) -> Result<Vec<u8>, String> {
        Err("no files here".to_string())
    }

    #[test]
    fn a_gltf_with_its_buffer_in_a_data_address_is_a_cube_lit_in_facets() {
        let json = gltf(Some(&data_uri(false)), r#"{"mesh":0}"#, "0", false);
        let mesh = load_gltf(json.as_bytes(), &nothing).expect("a cube");
        assert_eq!(mesh.triangles(), 12);
        assert_eq!(mesh.positions.len(), 8);
        assert_eq!(mesh.shading, Shading::Flat);
        assert_eq!(mesh.size(), [2., 2., 2.]);
    }

    #[test]
    fn a_glb_keeps_its_buffer_in_the_binary_chunk() {
        let json = gltf(None, r#"{"mesh":0}"#, "0", false);
        let whole = glb(&json, &cube_buffer(false));
        let mesh = load_gltf(&whole, &nothing).expect("a cube");
        assert_eq!(mesh.triangles(), 12);
        // A cut glb is damage, not a panic.
        assert!(load_gltf(&whole[..whole.len() - 40], &nothing).is_err());
        assert!(load_gltf(b"glTF\x02\0\0\0", &nothing).is_err());
    }

    #[test]
    fn a_gltf_reads_its_buffer_from_a_file_next_to_it_and_says_when_it_cannot() {
        let json = gltf(Some("my%20cube.bin"), r#"{"mesh":0}"#, "0", false);
        let asked = std::cell::RefCell::new(Vec::new());
        let reader = |name: &str| {
            asked.borrow_mut().push(name.to_string());
            Ok(cube_buffer(false))
        };
        let mesh = load_gltf(json.as_bytes(), &reader).expect("a cube");
        assert_eq!(mesh.triangles(), 12);
        assert_eq!(asked.borrow().as_slice(), ["my cube.bin"]);
        assert_eq!(
            load_gltf(json.as_bytes(), &nothing).err(),
            Some(LoadError::Unreadable("no files here".into()))
        );
        // Nothing outside the model's folder is read by address.
        let outside = gltf(Some("C:/secret.bin"), r#"{"mesh":0}"#, "0", false);
        assert_eq!(load_gltf(outside.as_bytes(), &reader).err(), Some(LoadError::Empty));
        assert_eq!(asked.borrow().len(), 1);
    }

    #[test]
    fn node_transforms_place_the_mesh_and_every_instance_counts() {
        let uri = data_uri(false);
        let moved = gltf(
            Some(&uri),
            r#"{"mesh":0,"translation":[10,0,0],"scale":[2,2,2]}"#,
            "0",
            false,
        );
        let mesh = load_gltf(moved.as_bytes(), &nothing).expect("a cube");
        assert_eq!(mesh.center(), [10., 0., 0.]);
        assert_eq!(mesh.size(), [4., 4., 4.]);
        // Two nodes use the one mesh, one of them a child of the other: two cubes.
        let twice = gltf(
            Some(&uri),
            r#"{"mesh":0,"children":[1]},{"mesh":0,"translation":[0,5,0]}"#,
            "0",
            false,
        );
        let mesh = load_gltf(twice.as_bytes(), &nothing).expect("two cubes");
        assert_eq!(mesh.triangles(), 24);
        assert_eq!(mesh.size(), [2., 7., 2.]);
        // A matrix: stretch x by 4, then a quarter turn about z, so the long side is up.
        let turned = gltf(
            Some(&uri),
            r#"{"mesh":0,"matrix":[0,4,0,0,-1,0,0,0,0,0,1,0,0,0,0,1]}"#,
            "0",
            false,
        );
        let mesh = load_gltf(turned.as_bytes(), &nothing).expect("a cube");
        assert_eq!(mesh.size(), [2., 8., 2.]);
        // A mirrored node keeps its triangles.
        let mirrored = gltf(Some(&uri), r#"{"mesh":0,"scale":[-1,1,1]}"#, "0", false);
        assert_eq!(
            load_gltf(mirrored.as_bytes(), &nothing).expect("a cube").triangles(),
            12
        );
    }

    #[test]
    fn a_scene_less_gltf_draws_the_nodes_nobody_is_a_child_of() {
        let json = gltf(
            Some(&data_uri(false)),
            r#"{"mesh":0},{"mesh":0,"translation":[0,0,9]}"#,
            "",
            false,
        )
        .replace(r#""scenes":[{"nodes":[]}],"#, "");
        let mesh = load_gltf(json.as_bytes(), &nothing).expect("two cubes");
        assert_eq!(mesh.triangles(), 24);
    }

    #[test]
    fn compressed_and_wrong_versions_are_named_and_a_cycle_does_not_hang() {
        let draco = r#"{"asset":{"version":"2.0"},"extensionsRequired":["KHR_draco_mesh_compression"]}"#;
        assert_eq!(load_gltf(draco.as_bytes(), &nothing).err(), Some(LoadError::Compressed));
        let old = r#"{"asset":{"version":"1.0"}}"#;
        assert!(matches!(
            load_gltf(old.as_bytes(), &nothing),
            Err(LoadError::Damaged(_))
        ));
        assert!(load_gltf(b"not json", &nothing).is_err());
        let cycle =
            r#"{"asset":{"version":"2.0"},"nodes":[{"children":[1]},{"children":[0]}],"scenes":[{"nodes":[0]}]}"#;
        assert_eq!(load_gltf(cycle.as_bytes(), &nothing).err(), Some(LoadError::Empty));
    }

    #[test]
    fn a_gltf_with_normals_is_lit_smooth_with_them() {
        let json = gltf(Some(&data_uri(true)), r#"{"mesh":0}"#, "0", true);
        let mesh = load_gltf(json.as_bytes(), &nothing).expect("a cube with normals");
        let Shading::Smooth(normals) = &mesh.shading else {
            panic!("the file's normals")
        };
        assert_eq!(normals[0], [0., 1., 0.]);
    }

    /// A zip with the given files, stored.
    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(&mut out);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, text) in files {
            writer.start_file(*name, options).expect("a file starts");
            writer.write_all(text.as_bytes()).expect("a file is written");
        }
        writer.finish().expect("the zip ends");
        out.into_inner()
    }

    fn cube_model(objects: &str, build: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
            <model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
            <resources>{objects}</resources><build>{build}</build></model>"#
        )
    }

    fn cube_object(id: u32) -> String {
        let (corners, triangles) = cube();
        let vertices: String = corners
            .iter()
            .map(|[x, y, z]| format!(r#"<vertex x="{x}" y="{y}" z="{z}"/>"#))
            .collect();
        let tris: String = triangles
            .iter()
            .map(|[a, b, c]| format!(r#"<triangle v1="{a}" v2="{b}" v3="{c}"/>"#))
            .collect();
        format!(
            r#"<object id="{id}" type="model"><mesh><vertices>{vertices}</vertices><triangles>{tris}</triangles></mesh></object>"#
        )
    }

    #[test]
    fn a_3mf_is_stood_up_and_placed_by_its_build_items() {
        let model = cube_model(
            &cube_object(1),
            r#"<item objectid="1" transform="1 0 0 0 1 0 0 0 1 0 0 30"/>"#,
        );
        let file = zip_of(&[("3D/3dmodel.model", &model)]);
        let mesh = load_3mf(&file).expect("a cube");
        assert_eq!(mesh.triangles(), 12);
        assert_eq!(mesh.shading, Shading::Flat);
        // Raised 30 along z, which is up in a 3MF and y here.
        assert_eq!(mesh.center(), [0., 30., 0.]);
        assert_eq!(mesh.size(), [2., 2., 2.]);
    }

    #[test]
    fn a_3mf_places_components_also_from_other_model_files_and_finds_the_model_through_its_relationships() {
        let part = cube_model(&cube_object(7), "");
        let main = cube_model(
            r#"<object id="1"><components>
                 <component objectid="7" p:path="/3D/Objects/part.model" xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06"/>
                 <component objectid="7" p:path="/3D/Objects/part.model" xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06" transform="1 0 0 0 1 0 0 0 1 10 0 0"/>
               </components></object>"#,
            r#"<item objectid="1"/>"#,
        );
        let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
            <Relationship Target="/Ground/مجسم.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>"#;
        let file = zip_of(&[
            ("_rels/.rels", rels),
            ("Ground/مجسم.model", &main),
            ("3D/Objects/part.model", &part),
        ]);
        let mesh = load_3mf(&file).expect("two cubes");
        assert_eq!(mesh.triangles(), 24);
        assert_eq!(mesh.size(), [12., 2., 2.]);
    }

    #[test]
    fn a_3mf_without_a_build_section_shows_every_mesh_and_a_broken_one_is_an_error() {
        let model = cube_model(&format!("{}{}", cube_object(1), cube_object(2)), "");
        let mesh = load_3mf(&zip_of(&[("3D/3dmodel.model", &model)])).expect("two cubes");
        assert_eq!(mesh.triangles(), 24);
        assert!(load_3mf(b"not a zip").is_err());
        assert!(matches!(
            load_3mf(&zip_of(&[("other.txt", "x")])),
            Err(LoadError::Damaged(_))
        ));
        let empty = cube_model("", "");
        assert_eq!(
            load_3mf(&zip_of(&[("3D/3dmodel.model", &empty)])).err(),
            Some(LoadError::Empty)
        );
    }
}
