use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use voxel_core::{
    coords::CubePos,
    record::CubeRecord,
    storage::{CubeStorage, SnapshotStorage},
    streaming::{Limits, Streamer, Tickets},
};

fn generated(pos: CubePos) -> CubeRecord {
    let mut cube = CubeRecord::new(pos).unwrap();
    for index in [0, 17, 1024, 4095] {
        cube.set(
            index,
            ((pos.y.unsigned_abs() + index as u64) % 31 + 1) as u32,
        )
        .unwrap();
    }
    cube
}

pub fn run(steps: u64) -> Result<Value, Box<dyn std::error::Error>> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = PathBuf::from("../.local/foundation-worlds")
        .join(format!("cubic-{stamp}-{}", std::process::id()));
    let store = SnapshotStorage::open(&root)?;
    let limits = Limits {
        max_cubes: 27,
        max_resident_bytes: 27 * 32768,
        max_cube_bytes: 32768,
        max_pending_jobs: 4,
    };
    let mut stream = Streamer::new(store, limits)?;
    let mut expected = BTreeMap::<CubePos, u64>::new();
    let mut assertions = 0u64;
    for step in 0..steps {
        let y = match step % 4 {
            0 => -1_000_000 - (step % 12) as i64,
            1 => 1_000_000 + (step % 12) as i64,
            2 => -2,
            _ => 15,
        };
        let center = CubePos::new(-1, y, 2);
        let tickets = Tickets::around(center, [1; 3], [0; 3])?;
        for job in stream.set_tickets(tickets) {
            assert!(stream.acknowledge_canceled(job));
        }
        for _ in 0..32 {
            let plan = stream.plan(4, 27)?;
            if plan.loads.is_empty() {
                break;
            }
            // Reverse completion order to avoid relying on FIFO arrival.
            for job in plan.loads.into_iter().rev() {
                let record = stream.read_or_generate(job, generated)?;
                assert!(stream.complete(job, record)?);
            }
            assert!(stream.charged_bytes() <= limits.max_resident_bytes);
            assertions += 1;
        }
        assert!(stream.simulates(center));
        assert_eq!(stream.resident_positions().count(), 27);
        let payload = stream.record(center).unwrap().payload();
        let current = if payload.is_empty() {
            0
        } else {
            u64::from_le_bytes(payload.try_into()?)
        };
        assert_eq!(current, *expected.get(&center).unwrap_or(&0));
        let next = current + 1;
        stream.edit(center, |cube| {
            cube.replace_payload(next.to_le_bytes().to_vec())?;
            Ok(())
        })?;
        expected.insert(center, next);
        assertions += 4;
    }
    for job in stream.set_tickets(Tickets::default()) {
        assert!(stream.acknowledge_canceled(job));
    }
    stream.plan(0, 27)?;
    assert_eq!(stream.resident_positions().count(), 0);
    let stats = stream.stats().clone();
    drop(stream);
    let mut reopened = SnapshotStorage::open(&root)?;
    let mut sum = 0u64;
    for (pos, count) in &expected {
        let record = reopened.load(*pos)?.expect("visited cube saved");
        let loaded = u64::from_le_bytes(record.payload().try_into()?);
        assert_eq!(loaded, *count);
        sum += loaded;
        assertions += 1;
    }
    assert_eq!(sum, steps);
    assert!(
        stats.peak_cubes <= limits.max_cubes
            && stats.peak_bytes <= limits.max_resident_bytes
            && stats.peak_jobs <= limits.max_pending_jobs
    );
    Ok(
        json!({"status":"PASS","route_steps":steps,"unique_active_cubes":expected.len(),"counter_sum_after_reopen":sum,"assertions":assertions,"loads":stats.loaded,"unloads":stats.unloaded,"peak_cubes":stats.peak_cubes,"peak_charged_bytes":stats.peak_bytes,"byte_budget":limits.max_resident_bytes,"peak_pending_jobs":stats.peak_jobs,"world_directory":root,"vertical_coordinate_range":[-1_000_011,1_000_011],"persistence":"actual immutable filesystem snapshots; single-cube writes"}),
    )
}
