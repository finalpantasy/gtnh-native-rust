//! Deterministic face-culling mesh from actual cube blocks; GPU upload is separately budgeted.

use voxel_core::record::CubeRecord;
pub const VERTEX_BYTES: usize = 6 * 4;

const DELTAS: [[i32; 3]; 6] = [
    [-1, 0, 0],
    [1, 0, 0],
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
];
const CORNERS: [[[f32; 3]; 4]; 6] = [
    [[0., 0., 0.], [0., 0., 1.], [0., 1., 1.], [0., 1., 0.]],
    [[1., 0., 1.], [1., 0., 0.], [1., 1., 0.], [1., 1., 1.]],
    [[0., 0., 1.], [0., 0., 0.], [1., 0., 0.], [1., 0., 1.]],
    [[0., 1., 0.], [0., 1., 1.], [1., 1., 1.], [1., 1., 0.]],
    [[1., 0., 0.], [0., 0., 0.], [0., 1., 0.], [1., 1., 0.]],
    [[0., 0., 1.], [1., 0., 1.], [1., 1., 1.], [0., 1., 1.]],
];
fn visible(cube: &CubeRecord, mut visit: impl FnMut([i32; 3], u32, usize)) {
    for (index, block) in cube
        .blocks()
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, b)| *b != 0)
    {
        let xyz = [
            (index & 15) as i32,
            (index >> 8) as i32,
            ((index >> 4) & 15) as i32,
        ];
        for (face, delta) in DELTAS.iter().enumerate() {
            let n = std::array::from_fn::<_, 3, _>(|i| xyz[i] + delta[i]);
            if n.iter().all(|v| (0..16).contains(v))
                && cube.blocks()[(n[1] * 256 + n[2] * 16 + n[0]) as usize] != 0
            {
                continue;
            }
            visit(xyz, block, face);
        }
    }
}
pub fn mesh_size(cube: &CubeRecord) -> usize {
    let mut faces = 0;
    visible(cube, |_, _, _| faces += 1);
    faces * 6 * VERTEX_BYTES
}
pub fn mesh(cube: &CubeRecord) -> Vec<u8> {
    let mut data = Vec::with_capacity(mesh_size(cube));
    visible(cube, |xyz, block, face| {
        let color = match block {
            1 => [0.20, 0.47, 0.34],
            2 => [0.46, 0.53, 0.57],
            3 => [0.37, 0.43, 0.50],
            4 => [0.57, 0.59, 0.53],
            5 => [0.95, 0.48, 0.16],
            6 => [0.22, 0.58, 0.95],
            7 => [0.68, 0.42, 0.91],
            8 => [0.98, 0.79, 0.25],
            _ => [0.9, 0.1, 0.8],
        };
        let shade = [0.65, 0.85, 0.4, 1.0, 0.70, 0.9][face];
        for corner in [0, 1, 2, 0, 2, 3] {
            for axis in 0..3 {
                data.extend_from_slice(
                    &(xyz[axis] as f32 + CORNERS[face][corner][axis]).to_le_bytes(),
                );
            }
            for component in color {
                data.extend_from_slice(&(component * shade as f32).to_le_bytes());
            }
        }
    });
    debug_assert_eq!(data.len(), data.capacity());
    data
}

/// Borrowed full-voxel surface with at most six rendered resident neighbors.
pub struct Surface<'a> {
    cube: &'a CubeRecord,
    neighbors: [Option<&'a CubeRecord>; 6],
}
#[derive(Clone, Copy, Debug)]
struct Quad {
    origin: [u8; 3],
    span: [u8; 3],
    block: u32,
    face: usize,
}
impl<'a> Surface<'a> {
    pub fn rendered<S: voxel_core::storage::CubeStorage>(
        world: &'a voxel_core::streaming::Streamer<S>,
        pos: voxel_core::coords::CubePos,
    ) -> Result<Self, &'static str> {
        if !world.renders(pos) {
            return Err("mesh cube is not rendered and resident");
        }
        let cube = world.record(pos).ok_or("mesh cube is unavailable")?;
        let neighbors = std::array::from_fn(|face| {
            pos.offset(DELTAS[face].map(i64::from))
                .filter(|p| world.renders(*p))
                .and_then(|p| world.record(p))
        });
        Self::new(cube, neighbors)
    }
    pub fn new(
        cube: &'a CubeRecord,
        neighbors: [Option<&'a CubeRecord>; 6],
    ) -> Result<Self, &'static str> {
        for (face, neighbor) in neighbors.iter().enumerate() {
            if let Some(neighbor) = neighbor
                && cube.pos().offset(DELTAS[face].map(i64::from)) != Some(neighbor.pos())
            {
                return Err("misaddressed mesh neighbor");
            }
        }
        Ok(Self { cube, neighbors })
    }
    fn adjacent(&self, xyz: [i32; 3], face: usize) -> u32 {
        let next = std::array::from_fn::<_, 3, _>(|i| xyz[i] + DELTAS[face][i]);
        let index = |p: [i32; 3]| {
            (p[1].rem_euclid(16) * 256 + p[2].rem_euclid(16) * 16 + p[0].rem_euclid(16)) as usize
        };
        if next.iter().all(|v| (0..16).contains(v)) {
            self.cube.blocks()[index(next)]
        } else {
            self.neighbors[face].map_or(0, |n| n.blocks()[index(next)])
        }
    }
    fn quads(&self, mut visit: impl FnMut(Quad)) {
        // One fixed mask is reused for each plane. Zero is air/hidden, otherwise the exact material ID.
        let mut mask = [0u32; 256];
        for face in 0..6 {
            let axis = face / 2;
            let u = (axis + 1) % 3;
            let v = (axis + 2) % 3;
            for slice in 0..16 {
                for row in 0..16 {
                    for column in 0..16 {
                        let mut xyz = [0; 3];
                        xyz[axis] = slice;
                        xyz[u] = column;
                        xyz[v] = row;
                        let block =
                            self.cube.blocks()[(xyz[1] * 256 + xyz[2] * 16 + xyz[0]) as usize];
                        mask[(row * 16 + column) as usize] =
                            if block != 0 && self.adjacent(xyz, face) == 0 {
                                block
                            } else {
                                0
                            };
                    }
                }
                for row in 0..16usize {
                    let mut column = 0;
                    while column < 16 {
                        let block = mask[row * 16 + column];
                        if block == 0 {
                            column += 1;
                            continue;
                        }
                        let mut width = 1;
                        while column + width < 16 && mask[row * 16 + column + width] == block {
                            width += 1;
                        }
                        let mut height = 1;
                        while row + height < 16
                            && (0..width).all(|x| mask[(row + height) * 16 + column + x] == block)
                        {
                            height += 1;
                        }
                        let mut origin = [0; 3];
                        origin[axis] = slice as u8;
                        origin[u] = column as u8;
                        origin[v] = row as u8;
                        let mut span = [1; 3];
                        span[u] = width as u8;
                        span[v] = height as u8;
                        visit(Quad {
                            origin,
                            span,
                            block,
                            face,
                        });
                        for y in row..row + height {
                            mask[y * 16 + column..y * 16 + column + width].fill(0);
                        }
                        column += width;
                    }
                }
            }
        }
    }
    pub fn size(&self) -> usize {
        let mut quads = 0;
        self.quads(|_| quads += 1);
        quads * 6 * VERTEX_BYTES
    }
    pub fn emit(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.size());
        self.quads(|quad| {
            let color = face_color(quad.block, quad.face);
            for corner in [0, 1, 2, 0, 2, 3] {
                for (axis, coordinate) in CORNERS[quad.face][corner].into_iter().enumerate() {
                    bytes.extend_from_slice(
                        &(f32::from(quad.origin[axis]) + coordinate * f32::from(quad.span[axis]))
                            .to_le_bytes(),
                    );
                }
                for component in color {
                    bytes.extend_from_slice(&component.to_le_bytes());
                }
            }
        });
        debug_assert_eq!(bytes.len(), bytes.capacity());
        bytes
    }
    pub fn identity(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(b"voxel-surface-greedy-v2");
        for block in self.cube.blocks() {
            hash.update(block.to_le_bytes());
        }
        // Only boundary opacity affects this cube's surface. Neighbor interiors/material colors do not.
        for face in 0..6 {
            let axis = face / 2;
            let u = (axis + 1) % 3;
            let v = (axis + 2) % 3;
            for row in 0..16 {
                for column in 0..16 {
                    let mut xyz = [0; 3];
                    xyz[axis] = if face % 2 == 0 { 0 } else { 15 };
                    xyz[u] = column;
                    xyz[v] = row;
                    let index = (xyz[1] * 256 + xyz[2] * 16 + xyz[0]) as usize;
                    hash.update([u8::from(
                        self.cube.blocks()[index] != 0 && self.adjacent(xyz, face) != 0,
                    )]);
                }
            }
        }
        hash.finalize().into()
    }
}
fn face_color(block: u32, face: usize) -> [f32; 3] {
    let color = match block {
        1 => [0.20, 0.47, 0.34],
        2 => [0.46, 0.53, 0.57],
        3 => [0.37, 0.43, 0.50],
        4 => [0.57, 0.59, 0.53],
        5 => [0.95, 0.48, 0.16],
        6 => [0.22, 0.58, 0.95],
        7 => [0.68, 0.42, 0.91],
        8 => [0.98, 0.79, 0.25],
        _ => [0.9, 0.1, 0.8],
    };
    let shade = [0.65, 0.85, 0.4, 1., 0.70, 0.9][face];
    color.map(|v| v * shade)
}
#[cfg(test)]
mod surface_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_core::coords::CubePos;
    #[test]
    fn neighboring_blocks_cull_shared_faces_and_reserve_exact_payload_bytes() {
        let mut cube = CubeRecord::new(CubePos::new(-1, 1_000_000, 0)).unwrap();
        assert_eq!(mesh_size(&cube), 0);
        cube.set(0, 1).unwrap();
        assert_eq!(mesh_size(&cube), 6 * 6 * VERTEX_BYTES);
        cube.set(1, 1).unwrap();
        assert_eq!(mesh_size(&cube), 10 * 6 * VERTEX_BYTES);
        let data = mesh(&cube);
        assert_eq!(data.len(), mesh_size(&cube));
        assert_eq!(data.capacity(), data.len());
        for float in data.chunks_exact(4) {
            assert!(f32::from_le_bytes(float.try_into().unwrap()).is_finite());
        }
    }
    #[test]
    fn local_geometry_does_not_lose_precision_at_extreme_world_coordinates() {
        let mut a = CubeRecord::new(CubePos::new(0, 0, 0)).unwrap();
        let mut b = CubeRecord::new(CubePos::new(i64::MAX / 16, i64::MIN / 16, -1)).unwrap();
        for i in [0, 15, 256, 4095] {
            a.set(i, 5).unwrap();
            b.set(i, 5).unwrap();
        }
        assert_eq!(mesh(&a), mesh(&b));
        assert!(!mesh(&a).is_empty());
    }
}
