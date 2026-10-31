//! Read-only picking over the exact resident render set. Never loads or generates cubes.
use voxel_core::{
    raycast::{Ray, RayError, Trace, cast},
    storage::CubeStorage,
    streaming::Streamer,
};

pub fn inspect<S: CubeStorage>(world: &Streamer<S>, ray: Ray) -> Result<Trace, RayError> {
    cast(ray, |position| {
        let cube = position.cube();
        if !world.renders(cube) {
            return None;
        }
        world
            .record(cube)
            .map(|record| record.blocks()[position.local_index()])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_core::{
        coords::{BlockPos, CubePos},
        raycast::{Outcome, Point},
        record::CubeRecord,
        storage::MemoryStorage,
        streaming::{Limits, Tickets},
    };
    #[test]
    fn inspection_obeys_render_tickets_without_mutating_loading_or_saving() {
        let center = CubePos::new(0, 0, 0);
        let mut world = Streamer::new(
            MemoryStorage::default(),
            Limits {
                max_cubes: 27,
                max_resident_bytes: 27 * 32768,
                max_cube_bytes: 32768,
                max_pending_jobs: 27,
            },
        )
        .unwrap();
        world.set_tickets(Tickets::around(center, [0; 3], [1; 3]).unwrap());
        let plan = world.plan(27, 27).unwrap();
        for job in plan.loads {
            let mut cube = CubeRecord::new(job.pos).unwrap();
            cube.set(0, 7).unwrap();
            world.complete(job, cube).unwrap();
        }
        let before: Vec<_> = world
            .resident_positions()
            .map(|p| (p, world.record(p).unwrap().encode()))
            .collect();
        let ray = |cell| Ray::new(Point::new(cell, [0.5; 3]).unwrap(), [1., 0., 0.], 1.).unwrap();
        let hit = inspect(&world, ray(BlockPos::new(0, 0, 0))).unwrap();
        assert!(
            matches!(hit.outcome, Outcome::Hit(h) if h.block==7 && h.position==BlockPos::new(0,0,0))
        );
        let hidden = BlockPos::new(16, 0, 0);
        assert!(world.record(hidden.cube()).is_some());
        assert_eq!(
            inspect(&world, ray(hidden)).unwrap().outcome,
            Outcome::Unavailable {
                position: hidden,
                distance: 0.
            }
        );
        let missing = BlockPos::new(160, 0, 0);
        assert_eq!(
            inspect(&world, ray(missing)).unwrap().outcome,
            Outcome::Unavailable {
                position: missing,
                distance: 0.
            }
        );
        let after: Vec<_> = world
            .resident_positions()
            .map(|p| (p, world.record(p).unwrap().encode()))
            .collect();
        assert_eq!(before, after);
        assert_eq!(world.pending_jobs(), 0);
    }
}
