use super::*;
use voxel_core::coords::CubePos;
#[test]
fn neighbor_opacity_beside_air_does_not_invalidate_a_surface() {
    let mut cube = CubeRecord::new(CubePos::new(0, 0, 0)).unwrap();
    cube.set(8 * 256 + 8 * 16 + 8, 1).unwrap();
    let neighbor = filled(CubePos::new(1, 0, 0), 2);
    let with_neighbor = |cube: &CubeRecord| {
        Surface::new(cube, [None, Some(&neighbor), None, None, None, None])
            .unwrap()
            .identity()
    };
    assert_eq!(
        Surface::new(&cube, [None; 6]).unwrap().identity(),
        with_neighbor(&cube)
    );
    cube.set(8 * 256 + 8 * 16 + 15, 1).unwrap();
    assert_ne!(
        Surface::new(&cube, [None; 6]).unwrap().identity(),
        with_neighbor(&cube)
    );
}
fn filled(pos: CubePos, block: u32) -> CubeRecord {
    let mut c = CubeRecord::new(pos).unwrap();
    for i in 0..4096 {
        c.set(i, block).unwrap();
    }
    c
}
#[test]
fn dense_cube_merges_to_six_rectangles_and_exact_payload_size() {
    let cube = filled(CubePos::new(-1, 1_000_000, 0), 1);
    let surface = Surface::new(&cube, [None; 6]).unwrap();
    assert_eq!(mesh_size(&cube), 6 * 256 * 6 * VERTEX_BYTES);
    assert_eq!(surface.size(), 6 * 6 * VERTEX_BYTES);
    assert_eq!(surface.emit().len(), surface.size());
    assert_eq!(surface.emit().capacity(), surface.size());
}
#[test]
fn all_six_adjacent_solids_hide_boundaries_but_missing_neighbors_expose_them() {
    let cube = filled(CubePos::new(0, 0, 0), 1);
    let neighbors = DELTAS.map(|d| filled(cube.pos().offset(d.map(i64::from)).unwrap(), 2));
    let surface = Surface::new(&cube, std::array::from_fn(|i| Some(&neighbors[i]))).unwrap();
    assert_eq!(surface.size(), 0);
    assert!(surface.emit().is_empty());
    for face in 0..6 {
        let input = std::array::from_fn(|i| if i == face { None } else { Some(&neighbors[i]) });
        assert_eq!(Surface::new(&cube, input).unwrap().size(), 6 * VERTEX_BYTES);
    }
    let wrong = filled(CubePos::new(99, 0, 0), 1);
    assert!(Surface::new(&cube, [Some(&wrong), None, None, None, None, None]).is_err());
}
#[test]
fn identity_binds_blocks_and_boundary_occupancy_without_neighbor_interior_noise() {
    let mut cube = filled(CubePos::new(0, 0, 0), 1);
    let mut neighbor = filled(CubePos::new(1, 0, 0), 2);
    let get = |cube: &CubeRecord, neighbor: &CubeRecord| {
        Surface::new(cube, [None, Some(neighbor), None, None, None, None])
            .unwrap()
            .identity()
    };
    let before = get(&cube, &neighbor);
    neighbor.set(8 * 256 + 8 * 16 + 8, 0).unwrap();
    assert_eq!(get(&cube, &neighbor), before);
    neighbor.set(8 * 256 + 8 * 16, 0).unwrap();
    assert_ne!(get(&cube, &neighbor), before);
    let after = get(&cube, &neighbor);
    cube.set(0, 3).unwrap();
    assert_ne!(get(&cube, &neighbor), after);
    assert_ne!(
        Surface::new(&cube, [None; 6]).unwrap().identity(),
        get(&cube, &neighbor)
    );
}
#[test]
fn merged_vertices_preserve_local_precision_and_outward_winding() {
    let a = filled(CubePos::new(0, 0, 0), 1);
    let b = filled(CubePos::new(i64::MAX / 16, i64::MIN / 16, -1), 1);
    let emitted = Surface::new(&a, [None; 6]).unwrap().emit();
    assert_eq!(emitted, Surface::new(&b, [None; 6]).unwrap().emit());
    assert_eq!(emitted.len(), 6 * 6 * VERTEX_BYTES);
    for (face, quad) in emitted.chunks_exact(6 * VERTEX_BYTES).enumerate() {
        let vertices = quad
            .chunks_exact(VERTEX_BYTES)
            .map(|v| {
                std::array::from_fn::<_, 3, _>(|i| {
                    f32::from_le_bytes(v[i * 4..i * 4 + 4].try_into().unwrap())
                })
            })
            .collect::<Vec<_>>();
        let u = std::array::from_fn::<_, 3, _>(|i| vertices[1][i] - vertices[0][i]);
        let v = std::array::from_fn::<_, 3, _>(|i| vertices[2][i] - vertices[0][i]);
        let normal = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        assert!(
            normal
                .iter()
                .zip(DELTAS[face])
                .map(|(n, d)| n * d as f32)
                .sum::<f32>()
                > 0.
        );
        for vertex in vertices {
            assert!(
                vertex
                    .iter()
                    .all(|v| v.is_finite() && (0. ..=16.).contains(v))
            );
        }
    }
}

#[test]
fn three_hundred_seeded_halos_match_every_analytic_unit_face_without_overlap() {
    let mut seed = 0xabcd_0123_4567u64;
    fn random(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }
    for scenario in 0..300 {
        let center = CubePos::new(-17, 1_000_000, -1);
        let mut cube = CubeRecord::new(center).unwrap();
        let mut neighbors =
            DELTAS.map(|d| CubeRecord::new(center.offset(d.map(i64::from)).unwrap()).unwrap());
        for c in std::iter::once(&mut cube).chain(neighbors.iter_mut()) {
            for index in 0..4096 {
                let value = random(&mut seed);
                if value.is_multiple_of(5) {
                    c.set(index, 1 + (value % 9) as u32).unwrap();
                }
            }
        }
        let present = random(&mut seed);
        let surface = Surface::new(
            &cube,
            std::array::from_fn(|i| {
                if present & (1 << i) != 0 {
                    Some(&neighbors[i])
                } else {
                    None
                }
            }),
        )
        .unwrap();
        let mut expected = vec![0u32; 6 * 4096];
        for (index, &block) in cube.blocks().iter().enumerate() {
            if block == 0 {
                continue;
            }
            let xyz = [
                (index & 15) as i32,
                (index >> 8) as i32,
                ((index >> 4) & 15) as i32,
            ];
            for face in 0..6 {
                let next = std::array::from_fn::<_, 3, _>(|i| xyz[i] + DELTAS[face][i]);
                let wrapped = next.map(|n| n.rem_euclid(16) as usize);
                let at = wrapped[1] * 256 + wrapped[2] * 16 + wrapped[0];
                let neighbor = if next.iter().all(|n| (0..16).contains(n)) {
                    cube.blocks()[at]
                } else if present & (1 << face) != 0 {
                    neighbors[face].blocks()[at]
                } else {
                    0
                };
                if neighbor == 0 {
                    expected[face * 4096 + index] = block;
                }
            }
        }
        let mut actual = vec![0u32; 6 * 4096];
        let mut quads = 0;
        surface.quads(|q| {
            quads += 1;
            let axis = q.face / 2;
            let u = (axis + 1) % 3;
            let v = (axis + 2) % 3;
            assert_eq!(q.span[axis], 1);
            for du in 0..q.span[u] {
                for dv in 0..q.span[v] {
                    let mut xyz = q.origin;
                    xyz[u] += du;
                    xyz[v] += dv;
                    assert!(xyz.iter().all(|&n| n < 16));
                    let index = q.face * 4096
                        + usize::from(xyz[1]) * 256
                        + usize::from(xyz[2]) * 16
                        + usize::from(xyz[0]);
                    assert_eq!(
                        actual[index], 0,
                        "overlapping rectangles in scenario {scenario}"
                    );
                    actual[index] = q.block;
                }
            }
        });
        assert_eq!(
            actual, expected,
            "unit-face mismatch in scenario {scenario}"
        );
        assert!(quads <= expected.iter().filter(|&&b| b != 0).count());
        assert_eq!(surface.size(), quads * 6 * VERTEX_BYTES);
        let bytes = surface.emit();
        assert_eq!(bytes.len(), surface.size());
        assert_eq!(bytes.capacity(), bytes.len());
        for value in bytes.chunks_exact(4) {
            assert!(f32::from_le_bytes(value.try_into().unwrap()).is_finite());
        }
    }
}

#[test]
fn checkerboard_keeps_material_boundaries_and_exposes_oversized_payload_honestly() {
    let mut c = CubeRecord::new(CubePos::new(0, 0, 0)).unwrap();
    for index in 0..4096 {
        let parity = (index & 15) + (index >> 8) + ((index >> 4) & 15);
        if parity % 2 == 0 {
            c.set(index, 1).unwrap();
        }
    }
    let surface = Surface::new(&c, [None; 6]).unwrap();
    assert_eq!(surface.size(), 2048 * 6 * 6 * VERTEX_BYTES);
    assert_eq!(surface.size(), mesh_size(&c));
    assert!(surface.size() > 1024 * 1024);
    let c = filled(CubePos::new(0, 0, 0), 1);
    let empty = CubeRecord::new(CubePos::new(1, 0, 0)).unwrap();
    assert_eq!(
        Surface::new(&c, [None; 6]).unwrap().identity(),
        Surface::new(&c, [None, Some(&empty), None, None, None, None])
            .unwrap()
            .identity()
    );
}
