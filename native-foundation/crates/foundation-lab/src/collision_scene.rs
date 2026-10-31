//! Native resident-cube motion checks. Trajectories/geometry are synthetic, not player controls.
use crate::{
    factory,
    world_storage::{Backend, WorldStore},
};
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};
use voxel_core::{
    collision::{self, Body, Motion},
    coords::{BlockPos, CubePos},
    raycast::{Face, Point},
    record::CubeRecord,
    storage::CubeStorage,
    streaming::{Limits, Streamer, Tickets},
};
type Error = Box<dyn std::error::Error>;
fn generated(pos: CubePos) -> CubeRecord {
    let mut cube = CubeRecord::new(pos).unwrap();
    for z in 0..16 {
        for x in 0..16 {
            if !(2..4).contains(&x) || !(2..4).contains(&z) {
                cube.set(z * 16 + x, 1).unwrap();
            }
        }
    }
    for y in 1..5 {
        for z in 0..16 {
            cube.set(y * 256 + z * 16 + 12, 2).unwrap();
        }
    }
    cube
}
fn focus<S: CubeStorage>(world: &mut Streamer<S>, center: CubePos) -> Result<(), Error> {
    for job in world.set_tickets(Tickets::around(center, [1; 3], [0; 3])?) {
        assert!(world.acknowledge_canceled(job));
    }
    for _ in 0..16 {
        let jobs = world.plan(4, 27)?.loads;
        if jobs.is_empty() {
            break;
        }
        for job in jobs {
            let cube = world.read_or_generate(job, generated)?;
            assert!(world.complete(job, cube)?);
        }
    }
    assert_eq!(world.resident_positions().count(), 27);
    Ok(())
}
fn sample<S: CubeStorage>(world: &Streamer<S>, position: BlockPos) -> Option<u32> {
    world
        .record(position.cube())
        .map(|cube| cube.blocks()[position.local_index()])
}
fn move_body<S: CubeStorage>(
    world: &Streamer<S>,
    body: Body,
    delta: [f64; 3],
) -> Result<Motion, collision::Error> {
    collision::sweep(body, delta, collision::DEFAULT_STEPS, |p| sample(world, p))
}
fn at(origin: CubePos, cell: [i64; 3], fraction: [f64; 3]) -> Body {
    let p = origin.block_at(0).unwrap();
    Body::new(
        Point::new(
            BlockPos::new(p.x + cell[0], p.y + cell[1], p.z + cell[2]),
            fraction,
        )
        .unwrap(),
        [0.6, 1.8, 0.6],
    )
    .unwrap()
}
struct Stats {
    frames: u64,
    queries: u64,
    tested: u64,
    floor: u64,
    wall: u64,
    trace: u64,
}
impl Default for Stats {
    fn default() -> Self {
        Self {
            frames: 0,
            queries: 0,
            tested: 0,
            floor: 0,
            wall: 0,
            trace: 0xcbf29ce484222325,
        }
    }
}
fn pose(body: Body, origin: CubePos) -> Value {
    let base = origin.block_at(0).unwrap();
    let p = body.minimum().cell();
    json!({"cell_offset":[p.x-base.x,p.y-base.y,p.z-base.z],"fraction":body.minimum().fraction(),"size":body.size()})
}
fn checked_step(
    world: &Streamer<WorldStore>,
    body: Body,
    delta: [f64; 3],
    origin: CubePos,
    stats: &mut Stats,
) -> Result<Body, Error> {
    let loads = world.stats().loaded;
    let bytes = world.charged_bytes();
    let pending = world.pending_jobs();
    let revisions = world
        .resident_positions()
        .map(|p| (p, world.record(p).unwrap().revision()))
        .collect::<Vec<_>>();
    let motion = move_body(world, body, delta)?;
    assert_eq!(world.stats().loaded, loads);
    assert_eq!(world.charged_bytes(), bytes);
    assert_eq!(world.pending_jobs(), pending);
    assert_eq!(
        revisions,
        world
            .resident_positions()
            .map(|p| (p, world.record(p).unwrap().revision()))
            .collect::<Vec<_>>()
    );
    assert!(
        motion.contacts.len() <= 3
            && motion.queries <= motion.tested
            && motion.tested <= collision::DEFAULT_STEPS
    );
    stats.frames += 1;
    stats.queries += u64::from(motion.queries);
    stats.tested += u64::from(motion.tested);
    stats.floor += motion
        .contacts
        .iter()
        .filter(|h| h.face == Face::PositiveY)
        .count() as u64;
    stats.wall += motion
        .contacts
        .iter()
        .filter(|h| h.face == Face::NegativeX)
        .count() as u64;
    let base = origin.block_at(0).unwrap();
    let p = motion.body.minimum().cell();
    for value in [p.x - base.x, p.y - base.y, p.z - base.z] {
        for byte in value.to_le_bytes() {
            stats.trace = (stats.trace ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    for value in motion.body.minimum().fraction() {
        for byte in value.to_bits().to_le_bytes() {
            stats.trace = (stats.trace ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    for contact in &motion.contacts {
        for value in contact.face.normal() {
            stats.trace = (stats.trace ^ u64::from(value as u8)).wrapping_mul(0x100000001b3);
        }
    }
    Ok(motion.body)
}
pub fn run() -> Result<Value, Error> {
    let root = factory::world_root(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos())?;
    let mut world = Streamer::new(
        WorldStore::open(&root, Backend::Journal)?,
        Limits {
            max_cubes: 27,
            max_resident_bytes: 27 * 32768,
            max_cube_bytes: 32768,
            max_pending_jobs: 4,
        },
    )?;
    let origins = [
        CubePos::new(0, 0, 0),
        CubePos::new(-1, 1_000_000, -17),
        CubePos::new(i64::MAX / 16 - 100, i64::MIN / 16 + 100, -1_000_000),
    ];
    let (mut traces, mut rows) = (Vec::new(), Vec::new());
    for origin in origins {
        let mut stats = Stats::default();
        let mut walk = at(origin, [6, 4, 6], [0.5, 0., 0.5]);
        let begin = walk.minimum();
        for _ in 0..256 {
            focus(&mut world, walk.minimum().cell().cube())?;
            walk = checked_step(&world, walk, [0.2, -0.25, 0.1], origin, &mut stats)?;
        }
        let p = walk.minimum();
        let relative = [
            (p.cell().x - begin.cell().x) as f64 + p.fraction()[0] - begin.fraction()[0],
            (p.cell().y - begin.cell().y) as f64 + p.fraction()[1] - begin.fraction()[1],
            (p.cell().z - begin.cell().z) as f64 + p.fraction()[2] - begin.fraction()[2],
        ];
        for (actual, expected) in relative.into_iter().zip([4.9, -3., 25.6]) {
            assert!((actual - expected).abs() < 1e-8);
        }
        assert!(stats.floor > 100 && stats.wall > 100);
        let mut shaft = at(origin, [2, 4, 2], [0.1, 0., 0.1]);
        let before = shaft.minimum();
        let contacts_before = stats.floor + stats.wall;
        for _ in 0..128 {
            focus(&mut world, shaft.minimum().cell().cube())?;
            shaft = checked_step(&world, shaft, [0., 0.5, 0.], origin, &mut stats)?;
        }
        assert_eq!(shaft.minimum().cell().y - before.cell().y, 64);
        assert_eq!(shaft.minimum().fraction(), before.fraction());
        assert_eq!(stats.floor + stats.wall, contacts_before);
        focus(&mut world, origin.offset([50, 50, 50]).unwrap())?;
        let loads = world.stats().loaded;
        let pending = world.pending_jobs();
        let before = shaft;
        assert!(matches!(
            move_body(&world, shaft, [0., 0.25, 0.]),
            Err(collision::Error::Unavailable(_))
        ));
        assert_eq!(shaft, before);
        assert_eq!(world.stats().loaded, loads);
        assert_eq!(world.pending_jobs(), pending);
        traces.push(stats.trace);
        rows.push(json!({"origin_cube":[origin.x,origin.y,origin.z],"frames":stats.frames,"actual_cell_queries":stats.queries,"candidate_steps":stats.tested,"floor_contacts":stats.floor,"wall_contacts":stats.wall,"final_walk":pose(walk,origin),"final_shaft":pose(shaft,origin),"trace_hash":format!("{:016x}",stats.trace)}));
    }
    assert!(
        traces.iter().all(|hash| *hash == traces[0]),
        "64-bit translation changed collision trajectory"
    );
    assert!(world.stats().peak_cubes <= 27 && world.stats().peak_bytes <= 27 * 32768);
    Ok(
        json!({"status":"PASS","world_directory":root,"routes":rows,"frames":1152,"unavailable_checks":3,"translation_invariant":true,"peak_cubes":world.stats().peak_cubes,"peak_charged_cube_bytes":world.stats().peak_bytes,
        "scope":"Native readonly queries against real resident cube arrays with synthetic floor/wall/shaft generation. Clean generated cubes may be regenerated; no body-save, player-controller, rendering, Minecraft shape or real-time performance claim."}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use voxel_core::storage::MemoryStorage;
    #[test]
    fn resident_unrendered_cubes_remain_solid_and_queries_do_not_load_or_mutate() {
        let mut world = Streamer::new(
            MemoryStorage::default(),
            Limits {
                max_cubes: 27,
                max_resident_bytes: 27 * 32768,
                max_cube_bytes: 32768,
                max_pending_jobs: 4,
            },
        )
        .unwrap();
        for job in
            world.set_tickets(Tickets::around(CubePos::new(0, 0, 0), [0; 3], [1; 3]).unwrap())
        {
            assert!(world.acknowledge_canceled(job));
        }
        for _ in 0..8 {
            for job in world.plan(4, 27).unwrap().loads {
                let cube = world.read_or_generate(job, generated).unwrap();
                world.complete(job, cube).unwrap();
            }
        }
        let origin = CubePos::new(0, 1, 0);
        assert!(!world.renders(origin));
        assert!(world.record(origin).is_some());
        let before = world.record(origin).unwrap().encode();
        let loads = world.stats().loaded;
        let body = at(origin, [6, 1, 6], [0.5, 0., 0.5]);
        let motion = move_body(&world, body, [0., -0.25, 0.]).unwrap();
        assert_eq!(motion.applied, [0.; 3]);
        assert!(motion.contacts.iter().any(|h| h.face == Face::PositiveY));
        assert_eq!(world.stats().loaded, loads);
        assert_eq!(world.record(origin).unwrap().encode(), before);
        for job in world.set_tickets(Tickets::default()) {
            assert!(world.acknowledge_canceled(job));
        }
        world.plan(0, 27).unwrap();
        let loads = world.stats().loaded;
        assert!(matches!(
            move_body(&world, body, [0., -0.25, 0.]),
            Err(collision::Error::Unavailable(_))
        ));
        assert_eq!(world.stats().loaded, loads);
        assert_eq!(world.pending_jobs(), 0);
    }
}
