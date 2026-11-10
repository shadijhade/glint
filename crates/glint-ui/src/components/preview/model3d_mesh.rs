//! The one mesh every 3D loader fills, and the little vector maths around it (ModelView.tsx's three.js
//! `BufferGeometry`).
//!
//! A [`Mesh`] is indexed triangles with positions, optionally per-vertex colours, and either per-vertex normals
//! ([`Shading::Smooth`]) or none at all ([`Shading::Flat`]: the renderer takes each triangle's own normal, as
//! STL files and glTF files without normals mean). [`assemble`] is the only way to make one: it drops what can't
//! be drawn (indices past the vertices, triangles with a corner that isn't a number, collapsed triangles), finds
//! the bounds, and makes the normals the loader didn't have.

/// The most of a model read: past this it would hold up the preview (`MAX_BYTES`).
pub const MAX_BYTES: u64 = 256 * 1024 * 1024;
/// The most triangles drawn. A model with more is "too big to draw here" too: the mesh, the transformed copy the
/// renderer makes and the picture have to fit in memory together.
pub const MAX_TRIANGLES: usize = 12_000_000;

/// A point or direction.
pub type V3 = [f32; 3];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn length(a: V3) -> f32 {
    dot(a, a).sqrt()
}

/// `a` with length 1; a direction that has none stays zero.
pub fn normalize(a: V3) -> V3 {
    let len = length(a);
    if len > 0. && len.is_finite() {
        scale(a, 1. / len)
    } else {
        [0., 0., 0.]
    }
}

/// Why a model couldn't be drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// More than [`MAX_BYTES`] or [`MAX_TRIANGLES`].
    TooBig,
    /// The file is a model, with nothing in it to draw.
    Empty,
    /// Not the format it says, or cut short. The text is for the log.
    Damaged(&'static str),
    /// A glTF whose geometry is compressed (Draco, meshopt), which isn't read here.
    Compressed,
    /// The file (or a file it points to) couldn't be read.
    Unreadable(String),
}

/// How a mesh is lit.
#[derive(Clone, Debug, PartialEq)]
pub enum Shading {
    /// Each triangle's own normal: facets.
    Flat,
    /// A unit normal per vertex, interpolated across triangles.
    Smooth(Vec<V3>),
}

/// Triangles ready to draw.
#[derive(Clone, Debug)]
pub struct Mesh {
    pub positions: Vec<V3>,
    /// Three indices per triangle, all below `positions.len()`.
    pub indices: Vec<u32>,
    pub shading: Shading,
    /// Red, green, blue per vertex, when the file has them.
    pub colors: Option<Vec<[u8; 3]>>,
    /// The corners of the box around the triangles.
    pub min: V3,
    pub max: V3,
}

impl Mesh {
    /// How many triangles.
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    /// The box's width, height and depth (x, y, z of the file).
    pub fn size(&self) -> V3 {
        sub(self.max, self.min)
    }

    /// The middle of the box.
    pub fn center(&self) -> V3 {
        scale(add(self.min, self.max), 0.5)
    }

    /// Half the box's diagonal (`bounds.getSize().length() / 2`), never zero.
    pub fn radius(&self) -> f32 {
        (length(self.size()) / 2.).max(1e-6)
    }
}

/// What the loader knows about a mesh's normals.
pub enum Normals {
    /// None, and the model is made of facets (STL, glTF without normals, 3MF).
    Flat,
    /// None, and the model is smooth: they are worked out from the triangles around each vertex.
    Smooth,
    /// One per vertex, as the file has them.
    Given(Vec<V3>),
}

fn finite(p: V3) -> bool {
    p[0].is_finite() && p[1].is_finite() && p[2].is_finite()
}

/// Makes the mesh of `positions` and the triangles `indices` (three per triangle, extras ignored).
pub fn assemble(
    positions: Vec<V3>,
    mut indices: Vec<u32>,
    normals: Normals,
    colors: Option<Vec<[u8; 3]>>,
) -> Result<Mesh, LoadError> {
    let count = positions.len();
    if u32::try_from(count).is_err() {
        return Err(LoadError::TooBig);
    }
    // Keep the triangles that can be drawn, in place.
    let whole = indices.len() / 3 * 3;
    let mut write = 0;
    let mut read = 0;
    while read < whole {
        let tri = [indices[read], indices[read + 1], indices[read + 2]];
        read += 3;
        let corners = tri.map(|i| positions.get(i as usize).copied());
        let drawable =
            tri[0] != tri[1] && tri[1] != tri[2] && tri[0] != tri[2] && corners.iter().all(|c| c.is_some_and(finite));
        if drawable {
            indices[write..write + 3].copy_from_slice(&tri);
            write += 3;
        }
    }
    indices.truncate(write);
    if indices.is_empty() {
        return Err(LoadError::Empty);
    }
    if indices.len() / 3 > MAX_TRIANGLES {
        return Err(LoadError::TooBig);
    }
    let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for &i in &indices {
        if let Some(p) = positions.get(i as usize) {
            for (axis, v) in p.iter().enumerate() {
                min[axis] = min[axis].min(*v);
                max[axis] = max[axis].max(*v);
            }
        }
    }
    let shading = match normals {
        Normals::Flat => Shading::Flat,
        Normals::Given(given) if given.len() == count && given.iter().all(|n| finite(*n)) => {
            Shading::Smooth(given.into_iter().map(normalize).collect())
        }
        Normals::Given(_) | Normals::Smooth => Shading::Smooth(smooth_normals(&positions, &indices)),
    };
    let colors = colors.filter(|c| c.len() == count);
    Ok(Mesh {
        positions,
        indices,
        shading,
        colors,
        min,
        max,
    })
}

/// A normal per vertex from the triangles around it, each weighted by its area (`computeVertexNormals`).
pub fn smooth_normals(positions: &[V3], indices: &[u32]) -> Vec<V3> {
    let mut sums = vec![[0f32; 3]; positions.len()];
    for tri in indices.as_chunks::<3>().0 {
        let at = |k: usize| positions.get(tri[k] as usize).copied();
        let (Some(a), Some(b), Some(c)) = (at(0), at(1), at(2)) else {
            continue;
        };
        let face = cross(sub(b, a), sub(c, a));
        for &i in tri {
            if let Some(sum) = sums.get_mut(i as usize) {
                *sum = add(*sum, face);
            }
        }
    }
    sums.into_iter()
        .map(|n| {
            let unit = normalize(n);
            if unit == [0., 0., 0.] {
                [0., 0., 1.]
            } else {
                unit
            }
        })
        .collect()
}

/// A faster hash for the vertex-welding maps: the keys are the bits of floats and small integers, nothing an
/// attacker steers, and the default hasher is several times slower over millions of vertices.
#[derive(Default, Clone, Copy)]
pub struct FastHasher(u64);

impl std::hash::Hasher for FastHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }

    fn write_u32(&mut self, n: u32) {
        self.write_u64(u64::from(n));
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

pub type FastBuild = std::hash::BuildHasherDefault<FastHasher>;

/// Vertices shared by position: STL repeats every corner for every triangle.
#[derive(Default)]
pub struct Welder {
    seen: std::collections::HashMap<[u32; 3], u32, FastBuild>,
    pub positions: Vec<V3>,
}

impl Welder {
    /// The index of the vertex at `p`, added if it is new. `None` once there are too many vertices for an index.
    pub fn vertex(&mut self, p: V3) -> Option<u32> {
        // -0.0 and 0.0 are the same place.
        let key = p.map(|v| (v + 0.).to_bits());
        if let Some(&i) = self.seen.get(&key) {
            return Some(i);
        }
        let i = u32::try_from(self.positions.len()).ok()?;
        self.seen.insert(key, i);
        self.positions.push(p);
        Some(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> (Vec<V3>, Vec<u32>) {
        (
            vec![[0., 0., 0.], [2., 0., 0.], [2., 2., 0.], [0., 2., 0.]],
            vec![0, 1, 2, 0, 2, 3],
        )
    }

    #[test]
    fn a_mesh_knows_its_box_and_its_triangles() {
        let (positions, indices) = square();
        let mesh = assemble(positions, indices, Normals::Smooth, None).expect("a mesh");
        assert_eq!(mesh.triangles(), 2);
        assert_eq!(mesh.size(), [2., 2., 0.]);
        assert_eq!(mesh.center(), [1., 1., 0.]);
        assert!((mesh.radius() - 2f32.sqrt()).abs() < 1e-6);
        // Smooth normals of a flat square all face the same way.
        let Shading::Smooth(normals) = &mesh.shading else {
            panic!("smooth")
        };
        assert!(normals.iter().all(|n| (n[2] - 1.).abs() < 1e-6));
    }

    #[test]
    fn what_cannot_be_drawn_is_dropped_and_nothing_left_is_empty() {
        let (mut positions, mut indices) = square();
        // An index past the vertices, a collapsed triangle, a corner that isn't a number, a stray index.
        indices.extend([0, 1, 9, 1, 1, 2, 0, 1, 4, 3]);
        positions.push([f32::NAN, 0., 0.]);
        let mesh = assemble(positions.clone(), indices, Normals::Flat, None).expect("a mesh");
        assert_eq!(mesh.triangles(), 2);
        assert_eq!(mesh.shading, Shading::Flat);
        assert_eq!(
            assemble(positions, vec![0, 0, 1], Normals::Flat, None).err(),
            Some(LoadError::Empty)
        );
        assert_eq!(
            assemble(vec![], vec![], Normals::Flat, None).err(),
            Some(LoadError::Empty)
        );
    }

    #[test]
    fn given_normals_are_made_unit_and_wrong_ones_are_worked_out() {
        let (positions, indices) = square();
        let given = vec![[0., 0., 5.]; 4];
        let mesh = assemble(positions.clone(), indices.clone(), Normals::Given(given), None).expect("a mesh");
        let Shading::Smooth(normals) = &mesh.shading else {
            panic!("smooth")
        };
        assert_eq!(normals[0], [0., 0., 1.]);
        // The wrong number of normals is as good as none.
        let mesh = assemble(positions, indices, Normals::Given(vec![[1., 0., 0.]]), None).expect("a mesh");
        let Shading::Smooth(normals) = &mesh.shading else {
            panic!("smooth")
        };
        assert_eq!(normals.len(), 4);
        assert!((normals[0][2] - 1.).abs() < 1e-6);
    }

    #[test]
    fn welding_shares_vertices_by_position() {
        let mut welder = Welder::default();
        let a = welder.vertex([1., 2., 3.]);
        let b = welder.vertex([1., 2., 3.]);
        let c = welder.vertex([0., 0., 0.]);
        let d = welder.vertex([-0., 0., 0.]);
        assert_eq!(a, b);
        assert_eq!(c, d);
        assert_ne!(a, c);
        assert_eq!(welder.positions.len(), 2);
    }

    #[test]
    fn colors_of_the_wrong_length_are_dropped() {
        let (positions, indices) = square();
        let mesh = assemble(
            positions.clone(),
            indices.clone(),
            Normals::Flat,
            Some(vec![[1, 2, 3]; 4]),
        )
        .expect("a mesh");
        assert!(mesh.colors.is_some());
        let mesh = assemble(positions, indices, Normals::Flat, Some(vec![[1, 2, 3]; 3])).expect("a mesh");
        assert!(mesh.colors.is_none());
    }
}
