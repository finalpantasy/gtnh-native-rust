//! Bounded full-voxel swept boxes; no player-controller or Minecraft shape parity claim.
use crate::{
    coords::BlockPos,
    raycast::{Face, Point},
};
use std::fmt;
pub const DEFAULT_STEPS: u32 = 4096;
const POSITION_EPSILON: f64 = 1e-10;
const TIME_EPSILON: f64 = 1e-12;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    minimum: Point,
    size: [f64; 3],
}
impl Body {
    pub fn new(minimum: Point, size: [f64; 3]) -> Result<Self, Error> {
        if size
            .iter()
            .any(|v| !v.is_finite() || !(1e-6..=16.).contains(v))
        {
            return Err(Error::InvalidBody);
        }
        let last = std::array::from_fn(|i| (minimum.fraction()[i] + size[i]).ceil() as i64 - 1);
        offset(minimum.cell(), last)?;
        Ok(Self { minimum, size })
    }
    pub fn minimum(self) -> Point {
        self.minimum
    }
    pub fn size(self) -> [f64; 3] {
        self.size
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub face: Face,
    pub position: BlockPos,
    pub block: u32,
    pub time: f64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Motion {
    pub body: Body,
    pub applied: [f64; 3],
    pub contacts: Vec<Contact>,
    pub tested: u32,
    pub queries: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidBody,
    InvalidDelta,
    InvalidBudget,
    WorldBoundary,
    Unavailable(BlockPos),
    InitialOverlap { position: BlockPos, block: u32 },
    QueryLimit,
    NoProgress,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
fn coordinates(p: BlockPos) -> [i64; 3] {
    [p.x, p.y, p.z]
}
fn offset(base: BlockPos, delta: [i64; 3]) -> Result<BlockPos, Error> {
    Ok(BlockPos::new(
        base.x.checked_add(delta[0]).ok_or(Error::WorldBoundary)?,
        base.y.checked_add(delta[1]).ok_or(Error::WorldBoundary)?,
        base.z.checked_add(delta[2]).ok_or(Error::WorldBoundary)?,
    ))
}
fn point(base: BlockPos, local: [f64; 3]) -> Result<Point, Error> {
    let mut shifts = local.map(|v| v.floor() as i64);
    let mut fraction = std::array::from_fn(|i| local[i] - shifts[i] as f64);
    for axis in 0..3 {
        // Subtracting floor(-tiny) can round the remainder to exactly 1.0.
        // Carry that rounded value instead of rejecting valid finite motion.
        if fraction[axis] == 1. {
            shifts[axis] += 1;
            fraction[axis] = 0.;
        }
    }
    Point::new(offset(base, shifts)?, fraction).map_err(|_| Error::InvalidBody)
}
#[derive(Clone, Copy)]
struct Interval {
    time: f64,
    normals: [i8; 3],
    overlap: bool,
}
fn interval(body: Body, delta: [f64; 3], cell: [i64; 3]) -> Option<Interval> {
    let minimum = body.minimum.fraction();
    let maximum = std::array::from_fn::<_, 3, _>(|i| minimum[i] + body.size[i]);
    let low = cell.map(|v| v as f64);
    if (0..3).all(|i| {
        maximum[i] > low[i] + POSITION_EPSILON && minimum[i] < low[i] + 1. - POSITION_EPSILON
    }) {
        return Some(Interval {
            time: 0.,
            normals: [0; 3],
            overlap: true,
        });
    }
    for axis in 0..3 {
        if (maximum[axis] <= low[axis] + POSITION_EPSILON && delta[axis] <= 0.)
            || (minimum[axis] >= low[axis] + 1. - POSITION_EPSILON && delta[axis] >= 0.)
        {
            return None;
        }
    }
    let mut entry = [f64::NEG_INFINITY; 3];
    let mut exit = f64::INFINITY;
    for axis in 0..3 {
        if delta[axis] == 0. {
            if maximum[axis] <= low[axis] + POSITION_EPSILON
                || minimum[axis] >= low[axis] + 1. - POSITION_EPSILON
            {
                return None;
            }
        } else {
            let a = (low[axis] - maximum[axis]) / delta[axis];
            let b = (low[axis] + 1. - minimum[axis]) / delta[axis];
            entry[axis] = a.min(b);
            exit = exit.min(a.max(b));
        }
    }
    let enter = entry.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !enter.is_finite() || enter > 1. || exit <= enter.max(0.) {
        return None;
    }
    if enter < 0. {
        // A tolerance-sized overlap is not a historic entry through the opposite block face.
        // Only the nearby face being moved into can constrain this request.
        let normals = std::array::from_fn(|axis| {
            if delta[axis] > 0.
                && maximum[axis] >= low[axis] - POSITION_EPSILON
                && maximum[axis] <= low[axis] + POSITION_EPSILON
            {
                -1
            } else if delta[axis] < 0.
                && minimum[axis] >= low[axis] + 1. - POSITION_EPSILON
                && minimum[axis] <= low[axis] + 1. + POSITION_EPSILON
            {
                1
            } else {
                0
            }
        });
        return normals.iter().any(|&n| n != 0).then_some(Interval {
            time: 0.,
            normals,
            overlap: false,
        });
    }
    let normals = std::array::from_fn(|axis| {
        if (entry[axis] - enter).abs() <= TIME_EPSILON && delta[axis] != 0. {
            if delta[axis] > 0. { -1 } else { 1 }
        } else {
            0
        }
    });
    Some(Interval {
        time: enter.max(0.),
        normals,
        overlap: false,
    })
}
fn face(axis: usize, normal: i8) -> Face {
    match (axis, normal > 0) {
        (0, false) => Face::NegativeX,
        (0, true) => Face::PositiveX,
        (1, false) => Face::NegativeY,
        (1, true) => Face::PositiveY,
        (2, false) => Face::NegativeZ,
        (2, true) => Face::PositiveZ,
        _ => unreachable!(),
    }
}
#[derive(Clone, Copy)]
struct AxisHit {
    position: BlockPos,
    block: u32,
    normal: i8,
    plane: i64,
}
struct Hit {
    time: f64,
    axes: [Option<AxisHit>; 3],
}
struct Candidates {
    time: f64,
    families: [Option<[Option<AxisHit>; 3]>; 8],
}
struct Work {
    limit: u32,
    tested: u32,
    queries: u32,
}
fn nearest(
    body: Body,
    delta: [f64; 3],
    work: &mut Work,
    sample: &mut impl FnMut(BlockPos) -> Option<u32>,
) -> Result<Option<Hit>, Error> {
    let start = body.minimum.fraction();
    let base = body.minimum.cell();
    // Include the destination face in the direction of motion: interval() treats t=1
    // as contact. Stationary and trailing faces remain half-open to avoid tangents.
    let lower = std::array::from_fn::<_, 3, _>(|i| {
        if delta[i] < 0. {
            (start[i] + delta[i]).ceil() as i64 - 1
        } else {
            start[i].floor() as i64
        }
    });
    let upper = std::array::from_fn::<_, 3, _>(|i| {
        if delta[i] > 0. {
            (start[i] + delta[i] + body.size[i]).floor() as i64
        } else {
            (start[i] + body.size[i]).ceil() as i64 - 1
        }
    });
    offset(base, lower)?;
    offset(base, upper)?;
    let mut best: Option<Candidates> = None;
    let mut missing: Option<(f64, BlockPos)> = None;
    for y in lower[1]..=upper[1] {
        for z in lower[2]..=upper[2] {
            for x in lower[0]..=upper[0] {
                if work.tested == work.limit {
                    return Err(Error::QueryLimit);
                }
                work.tested += 1;
                let relative = [x, y, z];
                let Some(candidate) = interval(body, delta, relative) else {
                    continue;
                };
                if best
                    .as_ref()
                    .is_some_and(|hit| candidate.time > hit.time + TIME_EPSILON)
                {
                    continue;
                }
                let position = offset(base, relative)?;
                work.queries += 1;
                let Some(block) = sample(position) else {
                    if missing.is_none_or(|(time, _)| candidate.time < time) {
                        missing = Some((candidate.time, position));
                    }
                    continue;
                };
                if block == 0 {
                    continue;
                }
                if candidate.overlap {
                    return Err(Error::InitialOverlap { position, block });
                }
                if best
                    .as_ref()
                    .is_none_or(|hit| candidate.time < hit.time - TIME_EPSILON)
                {
                    best = Some(Candidates {
                        time: candidate.time,
                        families: [None; 8],
                    });
                }
                let hit = best.as_mut().expect("new hit installed above");
                if (hit.time - candidate.time).abs() <= TIME_EPSILON {
                    hit.time = hit.time.min(candidate.time);
                    let mut mask = 0usize;
                    let mut axes = [None; 3];
                    for (axis, &normal) in candidate.normals.iter().enumerate() {
                        if normal != 0 {
                            mask |= 1 << axis;
                            axes[axis] = Some(AxisHit {
                                position,
                                block,
                                normal,
                                plane: relative[axis],
                            });
                        }
                    }
                    if mask == 0 {
                        return Err(Error::NoProgress);
                    }
                    if hit.families[mask].is_none() {
                        hit.families[mask] = Some(axes);
                    }
                }
            }
        }
    }
    if let Some((time, position)) = missing
        && best
            .as_ref()
            .is_none_or(|hit| time <= hit.time + TIME_EPSILON)
    {
        return Err(Error::Unavailable(position));
    }
    Ok(best.map(|candidates| {
        let mut pure = 0usize;
        for axis in 0..3 {
            if candidates.families[1 << axis].is_some() {
                pure |= 1 << axis;
            }
        }
        let mut axes = [None; 3];
        // A pure surface contact already blocks a tied corner from an adjoining voxel.
        // Do not turn that voxel's interior grid edge into another wall normal.
        for mask in 1usize..8 {
            if (mask.count_ones() == 1 || mask & pure == 0)
                && let Some(family) = candidates.families[mask]
            {
                for axis in 0..3 {
                    if axes[axis].is_none() {
                        axes[axis] = family[axis];
                    }
                }
            }
        }
        Hit {
            time: candidates.time,
            axes,
        }
    }))
}
pub fn sweep(
    body: Body,
    delta: [f64; 3],
    max_steps: u32,
    mut sample: impl FnMut(BlockPos) -> Option<u32>,
) -> Result<Motion, Error> {
    if !(1..=1_000_000).contains(&max_steps) {
        return Err(Error::InvalidBudget);
    }
    if delta.iter().any(|v| !v.is_finite() || v.abs() > 64.) {
        return Err(Error::InvalidDelta);
    }
    let mut current = Body::new(body.minimum, body.size)?;
    let mut remaining = delta;
    let mut elapsed = 0.;
    let mut work = Work {
        limit: max_steps,
        tested: 0,
        queries: 0,
    };
    let mut contacts = Vec::with_capacity(3);
    let mut finished = false;
    for _ in 0..3 {
        let Some(hit) = nearest(current, remaining, &mut work, &mut sample)? else {
            current = Body::new(
                point(
                    current.minimum.cell(),
                    std::array::from_fn(|i| current.minimum.fraction()[i] + remaining[i]),
                )?,
                current.size,
            )?;
            finished = true;
            break;
        };
        let mut target = std::array::from_fn::<_, 3, _>(|i| {
            current.minimum.fraction()[i] + remaining[i] * hit.time
        });
        let time = elapsed + (1. - elapsed) * hit.time;
        let mut blocked = 0;
        for (axis, contact) in hit.axes.into_iter().enumerate() {
            if let Some(contact) = contact {
                if remaining[axis] == 0. || contacts.len() == 3 {
                    return Err(Error::NoProgress);
                }
                blocked += 1;
                target[axis] = if contact.normal > 0 {
                    contact.plane as f64 + 1.
                } else {
                    contact.plane as f64 - current.size[axis]
                };
                contacts.push(Contact {
                    face: face(axis, contact.normal),
                    position: contact.position,
                    block: contact.block,
                    time,
                });
            }
        }
        if blocked == 0 {
            return Err(Error::NoProgress);
        }
        current = Body::new(point(current.minimum.cell(), target)?, current.size)?;
        for (axis, contact) in hit.axes.into_iter().enumerate() {
            remaining[axis] = if contact.is_some() {
                0.
            } else {
                remaining[axis] * (1. - hit.time)
            };
        }
        elapsed = time;
        if remaining == [0.; 3] {
            finished = true;
            break;
        }
    }
    if !finished {
        return Err(Error::NoProgress);
    }
    let before = coordinates(body.minimum.cell());
    let after = coordinates(current.minimum.cell());
    let mut applied = [0.; 3];
    for axis in 0..3 {
        applied[axis] = after[axis]
            .checked_sub(before[axis])
            .ok_or(Error::WorldBoundary)? as f64
            + current.minimum.fraction()[axis]
            - body.minimum.fraction()[axis];
    }
    Ok(Motion {
        body: current,
        applied,
        contacts,
        tested: work.tested,
        queries: work.queries,
    })
}
#[cfg(test)]
mod tests;
