//! Absolute cubic coordinates. These do not use vanilla's truncated wire packing.

pub const EDGE: i64 = 16;
pub const VOLUME: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CubePos {
    pub x: i64,
    pub y: i64,
    pub z: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockPos {
    pub x: i64,
    pub y: i64,
    pub z: i64,
}

impl BlockPos {
    pub const fn new(x: i64, y: i64, z: i64) -> Self {
        Self { x, y, z }
    }
    pub fn cube(self) -> CubePos {
        CubePos::new(
            self.x.div_euclid(EDGE),
            self.y.div_euclid(EDGE),
            self.z.div_euclid(EDGE),
        )
    }
    pub fn local_index(self) -> usize {
        ((self.y.rem_euclid(EDGE) * EDGE + self.z.rem_euclid(EDGE)) * EDGE
            + self.x.rem_euclid(EDGE)) as usize
    }
}

impl CubePos {
    pub const fn new(x: i64, y: i64, z: i64) -> Self {
        Self { x, y, z }
    }
    pub fn block_at(self, index: usize) -> Option<BlockPos> {
        if index >= VOLUME {
            return None;
        }
        Some(BlockPos::new(
            self.x.checked_mul(EDGE)?.checked_add((index & 15) as i64)?,
            self.y.checked_mul(EDGE)?.checked_add((index >> 8) as i64)?,
            self.z
                .checked_mul(EDGE)?
                .checked_add(((index >> 4) & 15) as i64)?,
        ))
    }
    pub fn offset(self, delta: [i64; 3]) -> Option<Self> {
        Some(Self::new(
            self.x.checked_add(delta[0])?,
            self.y.checked_add(delta[1])?,
            self.z.checked_add(delta[2])?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_boundaries_use_euclidean_coordinates() {
        for (block, cube, local) in [
            (-17, -2, 15),
            (-16, -1, 0),
            (-1, -1, 15),
            (0, 0, 0),
            (15, 0, 15),
            (16, 1, 0),
        ] {
            let p = BlockPos::new(block, block, block);
            assert_eq!(p.cube(), CubePos::new(cube, cube, cube));
            assert_eq!(p.local_index(), local * 256 + local * 16 + local);
            assert_eq!(p.cube().block_at(p.local_index()), Some(p));
        }
    }

    #[test]
    fn extreme_blocks_roundtrip_without_truncation() {
        for p in [
            BlockPos::new(i64::MIN, i64::MAX, -1),
            BlockPos::new(i64::MAX, i64::MIN, 1),
            BlockPos::new(50_000_000, 50_000_000, -50_000_000),
        ] {
            assert_eq!(p.cube().block_at(p.local_index()), Some(p));
        }
    }

    #[test]
    fn each_local_index_is_unique_and_reconstructs() {
        let cube = CubePos::new(-1, 12_345, -50);
        for index in 0..VOLUME {
            let block = cube.block_at(index).expect("valid local index");
            assert_eq!(block.cube(), cube);
            assert_eq!(block.local_index(), index);
        }
        assert_eq!(cube.block_at(VOLUME), None);
    }

    #[test]
    fn invalid_origins_and_offsets_fail_without_wrapping() {
        assert_eq!(CubePos::new(i64::MAX, 0, 0).block_at(0), None);
        assert_eq!(CubePos::new(i64::MAX, 0, 0).offset([1, 0, 0]), None);
        assert_eq!(CubePos::new(i64::MIN, 0, 0).offset([-1, 0, 0]), None);
        assert_eq!(
            CubePos::new(1, 2, 3).offset([-2, 5, -7]),
            Some(CubePos::new(-1, 7, -4))
        );
    }
}
