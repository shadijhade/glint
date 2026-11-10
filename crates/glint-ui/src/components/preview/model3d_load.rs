//! The simple loaders: STL (ASCII and binary), OBJ and PLY (ASCII, binary little and big endian).
//!
//! three.js had `STLLoader`, `OBJLoader` and `PLYLoader` for these (ModelView.tsx); these are written to give the
//! same pictures from the same files:
//!
//! - **STL** is facets: corners are welded by position (a binary STL repeats every corner for every triangle,
//!   which would be six times the memory) and the mesh is lit [`Shading::Flat`], as the file's own normals did.
//! - **OBJ** (`v`, `vn`, `f`; polygons fanned; negative indices count back from the end; `\` continues a line)
//!   uses the file's normals when every corner has one, else facets.
//! - **PLY** reads `x y z`, `nx ny nz` and `red green blue` of the vertex element and the face element's list
//!   of vertex indices (fanned); every other element is read past. Without normals it is lit smooth, as
//!   `computeVertexNormals` did.
//!
//! None of them panics on a damaged file: an index or a count that is wrong drops that triangle or stops the
//! read, and what was read before is drawn.

use super::model3d_mesh::{assemble, FastBuild, LoadError, Mesh, Normals, Welder, MAX_TRIANGLES, V3};
use std::borrow::Cow;
use std::collections::HashMap;

fn f32_at(bytes: &[u8], at: usize) -> f32 {
    bytes
        .get(at..at + 4)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map_or(f32::NAN, f32::from_le_bytes)
}

/// A polygon as triangles that share its first corner.
fn fan(polygon: &[u32], indices: &mut Vec<u32>) {
    if let Some((first, rest)) = polygon.split_first() {
        for pair in rest.windows(2) {
            indices.extend([*first, pair[0], pair[1]]);
        }
    }
}

fn starts_with_ci(line: &str, word: &str) -> bool {
    line.get(..word.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(word))
}

// ---- STL --------------------------------------------------------------------------------------------------

/// An STL file, binary or ASCII. A binary file is told by its length (an 80-byte header, a count, then 50 bytes
/// a facet), because some binary files begin with "solid" like an ASCII one.
pub fn load_stl(bytes: &[u8]) -> Result<Mesh, LoadError> {
    let expected = bytes
        .get(80..84)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map(|b| 84u64 + 50 * u64::from(u32::from_le_bytes(b)));
    let len = bytes.len() as u64;
    let ascii = looks_like_ascii_stl(bytes);
    match expected {
        Some(end) if end == len => load_stl_binary(bytes),
        Some(end) if end < len && !ascii => load_stl_binary(bytes),
        _ if ascii => load_stl_ascii(bytes),
        _ => Err(LoadError::Damaged("not an STL file")),
    }
}

fn looks_like_ascii_stl(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let head = head.trim_start();
    starts_with_ci(head, "solid") && (head.contains("facet") || head.contains("vertex") || head.contains("endsolid"))
}

fn load_stl_binary(bytes: &[u8]) -> Result<Mesh, LoadError> {
    let count = bytes
        .get(80..84)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map_or(0, u32::from_le_bytes) as usize;
    if count > MAX_TRIANGLES {
        return Err(LoadError::TooBig);
    }
    let mut welder = Welder::default();
    let mut indices = Vec::with_capacity(count * 3);
    for facet in 0..count {
        // 12 bytes of normal (ignored: the corners say it), three corners, 2 bytes of attributes.
        let base = 84 + facet * 50 + 12;
        for corner in 0..3 {
            let at = base + corner * 12;
            let p = [f32_at(bytes, at), f32_at(bytes, at + 4), f32_at(bytes, at + 8)];
            indices.push(welder.vertex(p).ok_or(LoadError::TooBig)?);
        }
    }
    assemble(welder.positions, indices, Normals::Flat, None)
}

fn load_stl_ascii(bytes: &[u8]) -> Result<Mesh, LoadError> {
    let text = String::from_utf8_lossy(bytes);
    let mut welder = Welder::default();
    let mut indices: Vec<u32> = Vec::new();
    let mut corners = 0;
    for line in text.lines() {
        let line = line.trim_start();
        if !starts_with_ci(line, "vertex") {
            continue;
        }
        let mut numbers = line
            .get(6..)
            .unwrap_or_default()
            .split_whitespace()
            .map(|word| word.parse::<f32>().unwrap_or(f32::NAN));
        let p = [
            numbers.next().unwrap_or(f32::NAN),
            numbers.next().unwrap_or(f32::NAN),
            numbers.next().unwrap_or(f32::NAN),
        ];
        indices.push(welder.vertex(p).ok_or(LoadError::TooBig)?);
        corners += 1;
        if corners % 3 == 0 && indices.len() / 3 > MAX_TRIANGLES {
            return Err(LoadError::TooBig);
        }
    }
    assemble(welder.positions, indices, Normals::Flat, None)
}

// ---- OBJ --------------------------------------------------------------------------------------------------

/// An index as OBJ writes it (1 is the first, -1 the last) as a position in a list of `count`.
fn resolve(index: i64, count: usize) -> Option<usize> {
    let count = i64::try_from(count).ok()?;
    let at = if index > 0 { index - 1 } else { count + index };
    (index != 0 && (0..count).contains(&at)).then_some(at as usize)
}

fn three<'a>(mut words: impl Iterator<Item = &'a str>) -> V3 {
    let mut next = || words.next().and_then(|w| w.parse::<f32>().ok()).unwrap_or(f32::NAN);
    [next(), next(), next()]
}

/// An OBJ file: its positions, normals and faces.
pub fn load_obj(bytes: &[u8]) -> Result<Mesh, LoadError> {
    let text = String::from_utf8_lossy(bytes);
    let mut file_positions: Vec<V3> = Vec::new();
    let mut file_normals: Vec<V3> = Vec::new();
    // A corner is a position and perhaps a normal; the same pair is the same vertex.
    let mut corners: HashMap<(u32, u32), u32, FastBuild> = HashMap::default();
    let mut positions: Vec<V3> = Vec::new();
    let mut normals: Vec<V3> = Vec::new();
    let mut every_corner_has_a_normal = true;
    let mut indices: Vec<u32> = Vec::new();
    let mut carry = String::new();
    let mut face: Vec<u32> = Vec::new();
    for raw in text.lines() {
        let line: Cow<str> = if let Some(start) = raw.strip_suffix('\\') {
            carry.push_str(start);
            carry.push(' ');
            continue;
        } else if carry.is_empty() {
            Cow::Borrowed(raw)
        } else {
            carry.push_str(raw);
            Cow::Owned(std::mem::take(&mut carry))
        };
        let mut words = line.split_whitespace();
        match words.next() {
            Some("v") => file_positions.push(three(words)),
            Some("vn") => file_normals.push(three(words)),
            Some("f") => {
                face.clear();
                let mut whole = true;
                for word in words {
                    let mut parts = word.split('/');
                    let position = parts
                        .next()
                        .and_then(|s| s.parse::<i64>().ok())
                        .and_then(|i| resolve(i, file_positions.len()));
                    let normal = parts
                        .nth(1)
                        .filter(|s| !s.is_empty())
                        .and_then(|s| s.parse::<i64>().ok())
                        .and_then(|i| resolve(i, file_normals.len()));
                    let Some(position) = position else {
                        whole = false;
                        break;
                    };
                    let key = (
                        u32::try_from(position).map_err(|_| LoadError::TooBig)?,
                        normal
                            .map_or(Ok(u32::MAX), u32::try_from)
                            .map_err(|_| LoadError::TooBig)?,
                    );
                    let index = match corners.get(&key) {
                        Some(&index) => index,
                        None => {
                            let index = u32::try_from(positions.len()).map_err(|_| LoadError::TooBig)?;
                            positions.push(file_positions.get(position).copied().unwrap_or([f32::NAN; 3]));
                            match normal.and_then(|n| file_normals.get(n)) {
                                Some(n) => normals.push(*n),
                                None => {
                                    every_corner_has_a_normal = false;
                                    normals.push([0., 0., 1.]);
                                }
                            }
                            corners.insert(key, index);
                            index
                        }
                    };
                    face.push(index);
                }
                if whole {
                    fan(&face, &mut indices);
                    if indices.len() / 3 > MAX_TRIANGLES {
                        return Err(LoadError::TooBig);
                    }
                }
            }
            _ => {}
        }
    }
    let normals = if every_corner_has_a_normal && !positions.is_empty() {
        Normals::Given(normals)
    } else {
        Normals::Flat
    };
    assemble(positions, indices, normals, None)
}

// ---- PLY --------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Kind {
    fn parse(word: &str) -> Option<Kind> {
        Some(match word {
            "char" | "int8" => Kind::I8,
            "uchar" | "uint8" => Kind::U8,
            "short" | "int16" => Kind::I16,
            "ushort" | "uint16" => Kind::U16,
            "int" | "int32" => Kind::I32,
            "uint" | "uint32" => Kind::U32,
            "float" | "float32" => Kind::F32,
            "double" | "float64" => Kind::F64,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Kind::I8 | Kind::U8 => 1,
            Kind::I16 | Kind::U16 => 2,
            Kind::I32 | Kind::U32 | Kind::F32 => 4,
            Kind::F64 => 8,
        }
    }

    fn is_float(self) -> bool {
        matches!(self, Kind::F32 | Kind::F64)
    }
}

enum Layout {
    Scalar(Kind),
    List { count: Kind, item: Kind },
}

struct Property {
    name: String,
    layout: Layout,
}

struct Element {
    name: String,
    count: usize,
    properties: Vec<Property>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Ascii,
    Little,
    Big,
}

/// The data after the header, read value by value.
struct Body<'a> {
    bytes: &'a [u8],
    at: usize,
    big: bool,
    words: Option<std::str::SplitAsciiWhitespace<'a>>,
}

impl Body<'_> {
    fn scalar(&mut self, kind: Kind) -> Option<f64> {
        if let Some(words) = &mut self.words {
            return words.next()?.parse::<f64>().ok();
        }
        let n = kind.size();
        let chunk = self.bytes.get(self.at..self.at + n)?;
        self.at += n;
        let mut a = [0u8; 8];
        a[..n].copy_from_slice(chunk);
        if self.big {
            a[..n].reverse();
        }
        Some(match kind {
            Kind::U8 => f64::from(a[0]),
            Kind::I8 => f64::from(i8::from_le_bytes([a[0]])),
            Kind::I16 => f64::from(i16::from_le_bytes([a[0], a[1]])),
            Kind::U16 => f64::from(u16::from_le_bytes([a[0], a[1]])),
            Kind::I32 => f64::from(i32::from_le_bytes([a[0], a[1], a[2], a[3]])),
            Kind::U32 => f64::from(u32::from_le_bytes([a[0], a[1], a[2], a[3]])),
            Kind::F32 => f64::from(f32::from_le_bytes([a[0], a[1], a[2], a[3]])),
            Kind::F64 => f64::from_le_bytes(a),
        })
    }

    /// A list's length.
    fn length(&mut self, kind: Kind) -> Option<usize> {
        let n = self.scalar(kind)?;
        (0. ..=1e9).contains(&n).then_some(n as usize)
    }

    /// Reads past one value of `layout` (a list is all of its items).
    fn skip(&mut self, layout: &Layout) -> Option<()> {
        match layout {
            Layout::Scalar(kind) => {
                self.scalar(*kind)?;
            }
            Layout::List { count, item } => {
                for _ in 0..self.length(*count)? {
                    self.scalar(*item)?;
                }
            }
        }
        Some(())
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// The header: the format and the elements, and where the data starts.
fn read_header(bytes: &[u8]) -> Result<(Format, Vec<Element>, usize), LoadError> {
    let damaged = LoadError::Damaged("not a PLY file");
    let end = find(&bytes[..bytes.len().min(1 << 20)], b"end_header").ok_or(damaged.clone())?;
    let mut start = end + "end_header".len();
    if bytes.get(start) == Some(&b'\r') {
        start += 1;
    }
    if bytes.get(start) == Some(&b'\n') {
        start += 1;
    }
    let header = String::from_utf8_lossy(&bytes[..end]);
    let mut lines = header.lines().map(str::trim);
    if lines.next() != Some("ply") {
        return Err(damaged);
    }
    let mut format = None;
    let mut elements: Vec<Element> = Vec::new();
    for line in lines {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["format", kind, ..] => {
                format = match *kind {
                    "ascii" => Some(Format::Ascii),
                    "binary_little_endian" => Some(Format::Little),
                    "binary_big_endian" => Some(Format::Big),
                    _ => return Err(LoadError::Damaged("an unknown PLY format")),
                }
            }
            ["element", name, count] => elements.push(Element {
                name: (*name).to_string(),
                count: count.parse().map_err(|_| LoadError::Damaged("a bad PLY count"))?,
                properties: Vec::new(),
            }),
            ["property", "list", count, item, name] => {
                let (Some(count), Some(item)) = (Kind::parse(count), Kind::parse(item)) else {
                    return Err(LoadError::Damaged("a bad PLY property"));
                };
                if let Some(element) = elements.last_mut() {
                    element.properties.push(Property {
                        name: (*name).to_string(),
                        layout: Layout::List { count, item },
                    });
                }
            }
            ["property", kind, name] => {
                let Some(kind) = Kind::parse(kind) else {
                    return Err(LoadError::Damaged("a bad PLY property"));
                };
                if let Some(element) = elements.last_mut() {
                    element.properties.push(Property {
                        name: (*name).to_string(),
                        layout: Layout::Scalar(kind),
                    });
                }
            }
            _ => {}
        }
    }
    Ok((format.ok_or(damaged)?, elements, start))
}

fn index_of(element: &Element, names: &[&str]) -> Option<usize> {
    element.properties.iter().position(|p| names.contains(&p.name.as_str()))
}

/// A PLY file: vertices (with normals and colours when it has them) and faces.
pub fn load_ply(bytes: &[u8]) -> Result<Mesh, LoadError> {
    let (format, elements, start) = read_header(bytes)?;
    let rest = bytes.get(start..).unwrap_or_default();
    let text;
    let mut body = if format == Format::Ascii {
        text = String::from_utf8_lossy(rest);
        Body {
            bytes: &[],
            at: 0,
            big: false,
            words: Some(text.split_ascii_whitespace()),
        }
    } else {
        Body {
            bytes: rest,
            at: 0,
            big: format == Format::Big,
            words: None,
        }
    };
    let mut positions: Vec<V3> = Vec::new();
    let mut normals: Vec<V3> = Vec::new();
    let mut colors: Vec<[u8; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    'elements: for element in &elements {
        // A count the data can't hold is damage, not a reason to allocate.
        let least: usize = element
            .properties
            .iter()
            .map(|p| match p.layout {
                Layout::Scalar(kind) => kind.size(),
                Layout::List { count, .. } => count.size(),
            })
            .sum();
        let room = if format == Format::Ascii {
            rest.len()
        } else {
            rest.len() / least.max(1)
        };
        if element.count > room {
            return Err(LoadError::Damaged("a PLY count is more than the file holds"));
        }
        let xyz = ["x", "y", "z"].map(|n| index_of(element, &[n]));
        let nxyz = ["nx", "ny", "nz"].map(|n| index_of(element, &[n]));
        let rgb = [
            ["red", "r", "diffuse_red"],
            ["green", "g", "diffuse_green"],
            ["blue", "b", "diffuse_blue"],
        ]
        .map(|names| index_of(element, &names));
        let corners = index_of(element, &["vertex_indices", "vertex_index"]);
        let is_vertex = element.name == "vertex";
        let is_face = element.name == "face";
        let mut face: Vec<u32> = Vec::new();
        for _ in 0..element.count {
            let mut p = [f32::NAN; 3];
            let mut n = [0f32; 3];
            let mut c = [255u8; 3];
            face.clear();
            for (at, property) in element.properties.iter().enumerate() {
                match &property.layout {
                    Layout::Scalar(kind) => {
                        let Some(value) = body.scalar(*kind) else {
                            break 'elements;
                        };
                        if is_vertex {
                            if let Some(axis) = xyz.iter().position(|i| *i == Some(at)) {
                                p[axis] = value as f32;
                            } else if let Some(axis) = nxyz.iter().position(|i| *i == Some(at)) {
                                n[axis] = value as f32;
                            } else if let Some(axis) = rgb.iter().position(|i| *i == Some(at)) {
                                let scaled = if kind.is_float() { value * 255. } else { value };
                                c[axis] = scaled.round().clamp(0., 255.) as u8;
                            }
                        }
                    }
                    Layout::List { count, item } => {
                        if is_face && corners == Some(at) {
                            let Some(length) = body.length(*count) else {
                                break 'elements;
                            };
                            for _ in 0..length {
                                let Some(value) = body.scalar(*item) else {
                                    break 'elements;
                                };
                                if (0. ..=f64::from(u32::MAX)).contains(&value) {
                                    face.push(value as u32);
                                }
                            }
                        } else if body.skip(&property.layout).is_none() {
                            break 'elements;
                        }
                    }
                }
            }
            if is_vertex {
                positions.push(p);
                normals.push(n);
                colors.push(c);
            } else if is_face {
                fan(&face, &mut indices);
                if indices.len() / 3 > MAX_TRIANGLES {
                    return Err(LoadError::TooBig);
                }
            }
        }
    }
    let vertex = elements.iter().find(|e| e.name == "vertex");
    let has = |names: [&str; 3]| vertex.is_some_and(|v| names.iter().all(|n| index_of(v, &[n]).is_some()));
    let normals = if has(["nx", "ny", "nz"]) {
        Normals::Given(normals)
    } else {
        Normals::Smooth
    };
    let colors = (has(["red", "green", "blue"])
        || has(["r", "g", "b"])
        || has(["diffuse_red", "diffuse_green", "diffuse_blue"]))
    .then_some(colors);
    assemble(positions, indices, normals, colors)
}

#[cfg(test)]
pub(super) mod tests {
    use super::super::model3d_mesh::Shading;
    use super::*;

    /// The eight corners of a cube of side 2 about the origin, and its twelve triangles.
    pub fn cube() -> (Vec<V3>, Vec<[u32; 3]>) {
        let corners = vec![
            [-1., -1., -1.],
            [1., -1., -1.],
            [1., 1., -1.],
            [-1., 1., -1.],
            [-1., -1., 1.],
            [1., -1., 1.],
            [1., 1., 1.],
            [-1., 1., 1.],
        ];
        let triangles = vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [2, 3, 7],
            [2, 7, 6],
            [1, 2, 6],
            [1, 6, 5],
            [3, 0, 4],
            [3, 4, 7],
        ];
        (corners, triangles)
    }

    /// The cube as a binary STL (with a header that starts with "solid", as some exporters write it).
    pub fn cube_binary_stl() -> Vec<u8> {
        let (corners, triangles) = cube();
        let mut out = vec![0u8; 80];
        out[..5].copy_from_slice(b"solid");
        out.extend((triangles.len() as u32).to_le_bytes());
        for tri in triangles {
            out.extend([0u8; 12]);
            for i in tri {
                for v in corners[i as usize] {
                    out.extend(v.to_le_bytes());
                }
            }
            out.extend([0u8; 2]);
        }
        out
    }

    pub fn cube_ascii_stl() -> String {
        let (corners, triangles) = cube();
        let mut out = String::from("solid مكعب\n");
        for tri in triangles {
            out.push_str("  facet normal 0 0 0\n    outer loop\n");
            for i in tri {
                let [x, y, z] = corners[i as usize];
                out.push_str(&format!("      vertex {x:e} {y} {z}\n"));
            }
            out.push_str("    endloop\n  endfacet\n");
        }
        out.push_str("endsolid\n");
        out
    }

    pub fn cube_obj() -> String {
        let (corners, triangles) = cube();
        let mut out = String::from("# a cube\no cube\n");
        for [x, y, z] in corners {
            out.push_str(&format!("v {x} {y} {z}\n"));
        }
        for [a, b, c] in triangles {
            out.push_str(&format!("f {} {} {}\n", a + 1, b + 1, c + 1));
        }
        out
    }

    #[test]
    fn a_binary_stl_is_welded_into_eight_corners_and_lit_in_facets() {
        let mesh = load_stl(&cube_binary_stl()).expect("a cube");
        assert_eq!(mesh.triangles(), 12);
        assert_eq!(mesh.positions.len(), 8);
        assert_eq!(mesh.shading, Shading::Flat);
        assert_eq!(mesh.size(), [2., 2., 2.]);
        assert_eq!(mesh.center(), [0., 0., 0.]);
    }

    #[test]
    fn an_ascii_stl_gives_the_same_cube_even_with_an_arabic_name_inside() {
        let mesh = load_stl(cube_ascii_stl().as_bytes()).expect("a cube");
        assert_eq!(mesh.triangles(), 12);
        assert_eq!(mesh.positions.len(), 8);
        let binary = load_stl(&cube_binary_stl()).expect("a cube");
        assert_eq!(mesh.size(), binary.size());
    }

    #[test]
    fn a_damaged_stl_is_an_error_and_never_a_panic() {
        assert!(load_stl(b"").is_err());
        assert!(load_stl(b"hello there").is_err());
        // A count that promises more facets than the file has.
        let mut cut = cube_binary_stl();
        cut.truncate(84 + 50 * 3 + 7);
        cut[80..84].copy_from_slice(&1000u32.to_le_bytes());
        assert!(load_stl(&cut).is_err());
        // "solid" and nothing else.
        assert_eq!(load_stl(b"solid x\nendsolid x\n").err(), Some(LoadError::Empty));
    }

    #[test]
    fn an_obj_has_positions_faces_and_fans_polygons() {
        let mesh = load_obj(cube_obj().as_bytes()).expect("a cube");
        assert_eq!(mesh.triangles(), 12);
        assert_eq!(mesh.positions.len(), 8);
        // No normals in the file: facets.
        assert_eq!(mesh.shading, Shading::Flat);
        let quad = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\n";
        assert_eq!(load_obj(quad.as_bytes()).expect("a quad").triangles(), 2);
        let pentagon = "v 0 0 0\nv 1 0 0\nv 2 1 0\nv 1 2 0\nv 0 1 0\nf 1 2 3 4 5\n";
        assert_eq!(load_obj(pentagon.as_bytes()).expect("a pentagon").triangles(), 3);
    }

    #[test]
    fn an_obj_reads_all_four_corner_forms_negative_indices_and_continued_lines() {
        let text = "v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvn 0 0 1\n\
                    f 1//1 2//1 3//1\n\
                    f 1/1/1 2/1/1 3/1/1\n\
                    f -3 -2 -1\n\
                    f 1 2 \\\n 3\n";
        let mesh = load_obj(text.as_bytes()).expect("triangles");
        assert_eq!(mesh.triangles(), 4);
        // The last two have no normal for some corner: facets.
        assert_eq!(mesh.shading, Shading::Flat);
        let with_normals = "v 0 0 0\nv 1 0 0\nv 0 1 0\nvn 0 0 2\nf 1//1 2//1 3//1\nf -3//-1 -2//-1 -1//-1\n";
        let mesh = load_obj(with_normals.as_bytes()).expect("triangles");
        assert_eq!(mesh.triangles(), 2);
        // The same corners are the same vertices.
        assert_eq!(mesh.positions.len(), 3);
        let Shading::Smooth(normals) = &mesh.shading else {
            panic!("the file's normals are used")
        };
        assert_eq!(normals[0], [0., 0., 1.]);
    }

    #[test]
    fn an_obj_with_bad_indices_keeps_the_good_faces() {
        let text = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\nf 1 2 9\nf 0 1 2\nf a b c\nf -9 1 2\n";
        let mesh = load_obj(text.as_bytes()).expect("one triangle");
        assert_eq!(mesh.triangles(), 1);
        assert_eq!(load_obj(b"v 1 2 3\n").err(), Some(LoadError::Empty));
        assert_eq!(load_obj(b"").err(), Some(LoadError::Empty));
    }

    fn ply_header(format: &str, vertex_props: &str, faces: usize) -> String {
        format!(
            "ply\nformat {format} 1.0\ncomment made by a test\nelement vertex 4\n{vertex_props}\
             element face {faces}\nproperty list uchar int vertex_indices\nend_header\n"
        )
    }

    #[test]
    fn an_ascii_ply_has_colours_and_a_fanned_quad() {
        let text = ply_header(
            "ascii",
            "property float x\nproperty float y\nproperty float z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\n",
            1,
        ) + "0 0 0 255 0 0\n1 0 0 0 255 0\n1 1 0 0 0 255\n0 1 0 10 20 30\n4 0 1 2 3\n";
        let mesh = load_ply(text.as_bytes()).expect("a quad");
        assert_eq!(mesh.triangles(), 2);
        assert_eq!(mesh.colors.as_ref().map(|c| c[0]), Some([255, 0, 0]));
        assert_eq!(mesh.colors.as_ref().map(|c| c[3]), Some([10, 20, 30]));
        // No normals in the file: smooth ones are worked out.
        assert!(matches!(mesh.shading, Shading::Smooth(_)));
    }

    fn binary_ply(format: &str, big: bool) -> Vec<u8> {
        let mut out = ply_header(
            format,
            "property float x\nproperty float y\nproperty float z\nproperty float nx\nproperty float ny\nproperty float nz\n",
            2,
        )
        .into_bytes();
        let float = |v: f32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        for (x, y) in [(0., 0.), (2., 0.), (2., 3.), (0., 3.)] {
            for v in [x, y, 0., 0., 0., 1.] {
                out.extend(float(v));
            }
        }
        for face in [[0i32, 1, 2], [0, 2, 3]] {
            out.push(3);
            for i in face {
                out.extend(if big { i.to_be_bytes() } else { i.to_le_bytes() });
            }
        }
        out
    }

    #[test]
    fn binary_ply_files_read_in_either_byte_order() {
        for (format, big) in [("binary_little_endian", false), ("binary_big_endian", true)] {
            let mesh = load_ply(&binary_ply(format, big)).expect(format);
            assert_eq!(mesh.triangles(), 2, "{format}");
            assert_eq!(mesh.size(), [2., 3., 0.], "{format}");
            let Shading::Smooth(normals) = &mesh.shading else {
                panic!("the file's normals")
            };
            assert_eq!(normals[1], [0., 0., 1.], "{format}");
        }
    }

    #[test]
    fn a_ply_reads_past_elements_it_does_not_use_and_refuses_counts_the_file_cannot_hold() {
        let text = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\n\
                    element edge 1\nproperty int a\nproperty int b\n\
                    element face 1\nproperty list uchar int vertex_indices\nproperty int flags\nend_header\n\
                    0 0 0\n1 0 0\n0 1 0\n0 1\n3 0 1 2 7\n";
        assert_eq!(load_ply(text.as_bytes()).expect("a triangle").triangles(), 1);
        let huge = "ply\nformat binary_little_endian 1.0\nelement vertex 4000000000\nproperty float x\nend_header\n";
        assert!(matches!(load_ply(huge.as_bytes()), Err(LoadError::Damaged(_))));
        assert!(load_ply(b"nothing").is_err());
        assert!(load_ply(b"ply\nformat ascii 1.0\nend_header\n").is_err());
        // A file cut in the middle of a vertex still tries what it has.
        let mut cut = binary_ply("binary_little_endian", false);
        cut.truncate(cut.len() - 20);
        let _ = load_ply(&cut);
    }
}
