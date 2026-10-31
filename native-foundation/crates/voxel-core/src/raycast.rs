//! Bounded full-voxel DDA queries. Absolute coordinates stay integer; fractions/distances stay local.
use crate::coords::BlockPos;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    cell: BlockPos,
    fraction: [f64; 3],
}
impl Point {
    pub fn new(cell: BlockPos, fraction: [f64; 3]) -> Result<Self, RayError> {
        if fraction
            .iter()
            .any(|v| !v.is_finite() || !(0. ..1.).contains(v))
        {
            return Err(RayError::InvalidOrigin);
        }
        Ok(Self { cell, fraction })
    }
    pub fn cell(self) -> BlockPos {
        self.cell
    }
    pub fn fraction(self) -> [f64; 3] {
        self.fraction
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    NegativeX,
    PositiveX,
    NegativeY,
    PositiveY,
    NegativeZ,
    PositiveZ,
}
impl Face {
    pub fn normal(self) -> [i8; 3] {
        match self {
            Self::NegativeX => [-1, 0, 0],
            Self::PositiveX => [1, 0, 0],
            Self::NegativeY => [0, -1, 0],
            Self::PositiveY => [0, 1, 0],
            Self::NegativeZ => [0, 0, -1],
            Self::PositiveZ => [0, 0, 1],
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownPolicy {
    Stop,
    SkipForInspection,
}
#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub origin: Point,
    pub direction: [f64; 3],
    pub max_distance: f64,
    pub max_queries: u32,
    pub unknown: UnknownPolicy,
}
impl Ray {
    pub fn new(origin: Point, direction: [f64; 3], max_distance: f64) -> Result<Self, RayError> {
        let ray = Self {
            origin,
            direction,
            max_distance,
            max_queries: 4096,
            unknown: UnknownPolicy::Stop,
        };
        ray.validate()?;
        Ok(ray)
    }
    fn validate(self) -> Result<[f64; 3], RayError> {
        if !self.max_distance.is_finite() || !(0. ..=1_000_000.).contains(&self.max_distance) {
            return Err(RayError::InvalidRange);
        }
        if !(1..=1_000_000).contains(&self.max_queries) {
            return Err(RayError::InvalidQueryBudget);
        }
        if self.direction.iter().any(|d| !d.is_finite()) {
            return Err(RayError::InvalidDirection);
        }
        let scale = self.direction.iter().map(|v| v.abs()).fold(0., f64::max);
        if scale == 0. {
            return Err(RayError::InvalidDirection);
        }
        let scaled = self.direction.map(|d| d / scale);
        let length = scaled.iter().map(|d| d * d).sum::<f64>().sqrt();
        Ok(scaled.map(|d| d / length))
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub position: BlockPos,
    pub block: u32,
    pub face: Option<Face>,
    pub distance: f64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    Hit(Hit),
    Miss { distance: f64 },
    Unavailable { position: BlockPos, distance: f64 },
    WorldBoundary { distance: f64 },
    QueryLimit { distance: f64 },
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trace {
    pub outcome: Outcome,
    pub queries: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RayError {
    InvalidOrigin,
    InvalidDirection,
    InvalidRange,
    InvalidQueryBudget,
}
impl fmt::Display for RayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for RayError {}
/// `None` is unavailable, `Some(0)` is air, and any other block ID is a full solid voxel.
/// Corner ties within 1e-12 distance units advance together; the lowest tied axis supplies the normal.
pub fn cast(ray: Ray, mut sample: impl FnMut(BlockPos) -> Option<u32>) -> Result<Trace, RayError> {
    let direction = ray.validate()?;
    let steps = direction.map(|d| {
        if d > 0. {
            1i64
        } else if d < 0. {
            -1
        } else {
            0
        }
    });
    let delta = direction.map(|d| if d == 0. { f64::INFINITY } else { 1. / d.abs() });
    let mut boundary = std::array::from_fn::<_, 3, _>(|i| match steps[i] {
        1 => (1. - ray.origin.fraction[i]) / direction[i],
        -1 => ray.origin.fraction[i] / -direction[i],
        _ => f64::INFINITY,
    });
    let mut cell = ray.origin.cell;
    let mut distance = 0.;
    let mut face = None;
    for index in 0..ray.max_queries {
        let queries = index + 1;
        match sample(cell) {
            Some(block) if block != 0 => {
                return Ok(Trace {
                    outcome: Outcome::Hit(Hit {
                        position: cell,
                        block,
                        face,
                        distance,
                    }),
                    queries,
                });
            }
            None if ray.unknown == UnknownPolicy::Stop => {
                return Ok(Trace {
                    outcome: Outcome::Unavailable {
                        position: cell,
                        distance,
                    },
                    queries,
                });
            }
            _ => {}
        }
        let next = boundary.iter().copied().fold(f64::INFINITY, f64::min);
        if next > ray.max_distance {
            return Ok(Trace {
                outcome: Outcome::Miss {
                    distance: ray.max_distance,
                },
                queries,
            });
        }
        let mut coordinates = [cell.x, cell.y, cell.z];
        let mut first = None;
        for axis in 0..3 {
            if boundary[axis] <= next + 1e-12 {
                let Some(coordinate) = coordinates[axis].checked_add(steps[axis]) else {
                    return Ok(Trace {
                        outcome: Outcome::WorldBoundary { distance: next },
                        queries,
                    });
                };
                coordinates[axis] = coordinate;
                boundary[axis] += delta[axis];
                first.get_or_insert(axis);
            }
        }
        let axis = first.expect("a finite normalized ray always advances at least one axis");
        face = Some(match (axis, steps[axis] > 0) {
            (0, true) => Face::NegativeX,
            (0, false) => Face::PositiveX,
            (1, true) => Face::NegativeY,
            (1, false) => Face::PositiveY,
            (2, true) => Face::NegativeZ,
            (2, false) => Face::PositiveZ,
            _ => unreachable!(),
        });
        cell = BlockPos::new(coordinates[0], coordinates[1], coordinates[2]);
        distance = next;
    }
    Ok(Trace {
        outcome: Outcome::QueryLimit { distance },
        queries: ray.max_queries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn origin(cell: BlockPos) -> Point {
        Point::new(cell, [0.5; 3]).unwrap()
    }
    fn hit(trace: Trace) -> Hit {
        match trace.outcome {
            Outcome::Hit(hit) => hit,
            other => panic!("expected hit, got {other:?}"),
        }
    }
    #[test]
    fn axis_rays_report_all_six_entry_faces_and_distance() {
        for (direction, target, face) in [
            ([1., 0., 0.], BlockPos::new(2, 0, 0), Face::NegativeX),
            ([-1., 0., 0.], BlockPos::new(-2, 0, 0), Face::PositiveX),
            ([0., 1., 0.], BlockPos::new(0, 2, 0), Face::NegativeY),
            ([0., -1., 0.], BlockPos::new(0, -2, 0), Face::PositiveY),
            ([0., 0., 1.], BlockPos::new(0, 0, 2), Face::NegativeZ),
            ([0., 0., -1.], BlockPos::new(0, 0, -2), Face::PositiveZ),
        ] {
            let found = hit(cast(
                Ray::new(origin(BlockPos::new(0, 0, 0)), direction, 10.).unwrap(),
                |pos| Some(if pos == target { 7 } else { 0 }),
            )
            .unwrap());
            assert_eq!(found.position, target);
            assert_eq!(found.face, Some(face));
            assert_eq!(found.block, 7);
            assert_eq!(found.distance, 1.5);
        }
    }
    #[test]
    fn negative_cube_boundaries_preserve_the_exact_cell() {
        let start = Point::new(BlockPos::new(-16, -16, -16), [0.25; 3]).unwrap();
        let target = BlockPos::new(-17, -16, -16);
        let found = hit(cast(Ray::new(start, [-1., 0., 0.], 2.).unwrap(), |p| {
            Some(if p == target { 1 } else { 0 })
        })
        .unwrap());
        assert_eq!(found.position, target);
        assert_eq!(found.distance, 0.25);
        assert_eq!(found.position.cube().x, -2);
    }
    #[test]
    fn initial_solid_is_distinct_from_an_entry_face_even_at_zero_range() {
        let found = hit(cast(
            Ray::new(origin(BlockPos::new(0, 0, 0)), [1., 0., 0.], 0.).unwrap(),
            |_| Some(4),
        )
        .unwrap());
        assert_eq!(found.distance, 0.);
        assert_eq!(found.face, None);
    }
    #[test]
    fn exact_corner_ties_skip_zero_volume_tangent_cells_deterministically() {
        let found = hit(cast(
            Ray::new(origin(BlockPos::new(0, 0, 0)), [1., 1., 0.], 4.).unwrap(),
            |p| {
                Some(if p == BlockPos::new(1, 0, 0) {
                    99
                } else if p == BlockPos::new(1, 1, 0) {
                    3
                } else {
                    0
                })
            },
        )
        .unwrap());
        assert_eq!(found.position, BlockPos::new(1, 1, 0));
        assert_eq!(found.block, 3);
        assert_eq!(found.face, Some(Face::NegativeX));
        assert!((found.distance - 0.5 * 2f64.sqrt()).abs() < 1e-12);
    }
    #[test]
    fn unknown_is_not_air_and_inspection_skip_is_explicit() {
        let sample = |p: BlockPos| {
            if p.x == 1 {
                None
            } else {
                Some(if p.x == 2 { 8 } else { 0 })
            }
        };
        let ray = Ray::new(origin(BlockPos::new(0, 0, 0)), [1., 0., 0.], 4.).unwrap();
        assert_eq!(
            cast(ray, sample).unwrap().outcome,
            Outcome::Unavailable {
                position: BlockPos::new(1, 0, 0),
                distance: 0.5
            }
        );
        let found = hit(cast(
            Ray {
                unknown: UnknownPolicy::SkipForInspection,
                ..ray
            },
            sample,
        )
        .unwrap());
        assert_eq!(found.position.x, 2);
        assert_eq!(found.distance, 1.5);
    }
    #[test]
    fn distance_is_inclusive_and_query_limits_never_masquerade_as_misses() {
        let sample = |p: BlockPos| Some(if p.x == 1 { 1 } else { 0 });
        let ray = Ray::new(origin(BlockPos::new(0, 0, 0)), [1., 0., 0.], 0.5).unwrap();
        assert!(matches!(
            cast(ray, sample).unwrap().outcome,
            Outcome::Hit(_)
        ));
        assert_eq!(
            cast(
                Ray {
                    max_distance: 0.49,
                    ..ray
                },
                sample
            )
            .unwrap()
            .outcome,
            Outcome::Miss { distance: 0.49 }
        );
        let limited = cast(
            Ray {
                max_queries: 1,
                max_distance: 10.,
                ..ray
            },
            sample,
        )
        .unwrap();
        assert_eq!(limited.queries, 1);
        assert!(matches!(limited.outcome, Outcome::QueryLimit { .. }));
    }
    #[test]
    fn world_edges_return_a_boundary_without_wrapping() {
        for (cell, direction, expected) in [
            (BlockPos::new(i64::MAX, 0, 0), [1., 0., 0.], 0.5),
            (BlockPos::new(0, i64::MIN, 0), [0., -1., 0.], 0.5),
        ] {
            let trace = cast(Ray::new(origin(cell), direction, 4.).unwrap(), |_| Some(0)).unwrap();
            assert_eq!(trace.outcome, Outcome::WorldBoundary { distance: expected });
            assert_eq!(trace.queries, 1);
        }
    }
    #[test]
    fn directions_normalize_without_overflow_or_underflow_and_inputs_are_validated() {
        for direction in [[f64::MAX, 0., 0.], [1e-320, 0., 0.]] {
            let found = hit(cast(
                Ray::new(origin(BlockPos::new(0, 0, 0)), direction, 4.).unwrap(),
                |p| Some(if p.x == 1 { 1 } else { 0 }),
            )
            .unwrap());
            assert_eq!(found.distance, 0.5);
        }
        let point = origin(BlockPos::new(0, 0, 0));
        for direction in [[0.; 3], [f64::NAN, 1., 0.], [f64::INFINITY, 0., 0.]] {
            assert!(Ray::new(point, direction, 4.).is_err());
        }
        for distance in [-1., f64::NAN, f64::INFINITY, 1e9] {
            assert!(Ray::new(point, [1., 0., 0.], distance).is_err());
        }
        assert!(Point::new(point.cell(), [1., 0., 0.]).is_err());
        assert!(Point::new(point.cell(), [-0.1, 0., 0.]).is_err());
        let ray = Ray {
            max_queries: 0,
            ..Ray::new(point, [1., 0., 0.], 4.).unwrap()
        };
        let mut calls = 0;
        assert!(
            cast(ray, |_| {
                calls += 1;
                Some(0)
            })
            .is_err()
        );
        assert_eq!(calls, 0);
    }
    #[test]
    fn giant_translations_do_not_change_traversal_distance_or_normals() {
        for offset in [
            BlockPos::new(0, 0, 0),
            BlockPos::new(i64::MAX - 100, i64::MIN + 100, 1_000_000_000_000),
        ] {
            let target = BlockPos::new(offset.x + 3, offset.y - 2, offset.z + 1);
            let found = hit(
                cast(Ray::new(origin(offset), [3., -2., 1.], 10.).unwrap(), |p| {
                    Some(if p == target { 5 } else { 0 })
                })
                .unwrap(),
            );
            assert_eq!(found.position, target);
            assert_eq!(found.face, Some(Face::NegativeX));
            assert!((found.distance - (2.5 / 3.) * 14f64.sqrt()).abs() < 1e-10);
        }
    }

    #[test]
    fn three_thousand_rays_match_independent_analytic_box_intersections() {
        use std::collections::BTreeMap;
        fn next(seed: &mut u64) -> u64 {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            *seed
        }
        fn analytic(start: [f64; 3], direction: [f64; 3], cell: [i64; 3]) -> Option<f64> {
            let (mut enter, mut exit) = (f64::NEG_INFINITY, f64::INFINITY);
            for axis in 0..3 {
                let lo = cell[axis] as f64;
                let hi = lo + 1.;
                if direction[axis] == 0. {
                    if start[axis] < lo || start[axis] >= hi {
                        return None;
                    }
                } else {
                    let a = (lo - start[axis]) / direction[axis];
                    let b = (hi - start[axis]) / direction[axis];
                    enter = enter.max(a.min(b));
                    exit = exit.min(a.max(b));
                }
            }
            if exit <= enter + 1e-12 || exit < 0. || enter > 30. {
                None
            } else {
                Some(enter.max(0.))
            }
        }
        let mut seed = 0x4567_1234_9876u64;
        for scenario in 0..3000 {
            let mut blocks = BTreeMap::new();
            for id in 1..=32 {
                let cell = std::array::from_fn::<_, 3, _>(|_| (next(&mut seed) % 9) as i64 - 4);
                blocks.insert((cell[0], cell[1], cell[2]), id);
            }
            let cell = std::array::from_fn::<_, 3, _>(|_| (next(&mut seed) % 5) as i64 - 2);
            let start =
                Point::new(BlockPos::new(cell[0], cell[1], cell[2]), [0.23, 0.57, 0.91]).unwrap();
            let mut direction =
                std::array::from_fn::<_, 3, _>(|_| (next(&mut seed) % 31) as f64 / 15. - 1.);
            if direction == [0.; 3] {
                direction = [1., 0., 0.];
            }
            let length = direction.iter().map(|v| v * v).sum::<f64>().sqrt();
            let unit = direction.map(|v| v / length);
            let start_float = std::array::from_fn(|i| cell[i] as f64 + start.fraction()[i]);
            let mut expected = None;
            for (&(x, y, z), &block) in &blocks {
                if let Some(distance) = analytic(start_float, unit, [x, y, z])
                    && expected
                        .as_ref()
                        .is_none_or(|(_, _, best)| distance < *best)
                {
                    expected = Some((BlockPos::new(x, y, z), block, distance));
                }
            }
            let trace = cast(Ray::new(start, direction, 30.).unwrap(), |p| {
                Some(blocks.get(&(p.x, p.y, p.z)).copied().unwrap_or(0))
            })
            .unwrap();
            match (expected, trace.outcome) {
                (Some((position, block, distance)), Outcome::Hit(found)) => {
                    assert_eq!(found.position, position, "scenario {scenario}");
                    assert_eq!(found.block, block);
                    assert!(
                        (found.distance - distance).abs() < 1e-9,
                        "scenario {scenario}: {} != {distance}",
                        found.distance
                    );
                }
                (None, Outcome::Miss { .. }) => {}
                (expected, actual) => {
                    panic!("scenario {scenario}: analytic {expected:?}, DDA {actual:?}")
                }
            }
        }
    }
}
