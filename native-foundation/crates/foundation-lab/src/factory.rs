//! Combined real filesystem streaming and a transactional two-machine factory.

use industrial_sim::{
    content::{Amount, Catalog, Id},
    inventory::Limits as PortLimits,
    machine::{Energy, Machine},
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use voxel_core::{
    coords::CubePos,
    record::{CubeRecord, RecordError},
    storage::CubeStorage,
    streaming::{Limits, Streamer, Tickets},
};

pub type Error = Box<dyn std::error::Error>;
use crate::world_storage::{Backend, WorldStore};
pub struct Outcome {
    pub report: Value,
    pub final_state: Vec<Vec<u8>>,
}
fn id(s: &str) -> Id {
    Id::new(format!("lab:{s}")).unwrap()
}
pub(crate) fn catalog() -> Result<Catalog, Error> {
    Ok(Catalog::from_json(include_bytes!(
        "../../../fixtures/industrial-v1.json"
    ))?)
}

pub(crate) fn world_root(stamp: u128) -> Result<PathBuf, Error> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let base = worlds_base()?;
    for _ in 0..1024 {
        let sequence = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| "scenario sequence exhausted")?;
        let path = base.join(format!("factory-{stamp}-{}-{sequence}", std::process::id()));
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err("could not allocate an exclusive private scenario directory".into())
}

pub(crate) fn worlds_base() -> Result<PathBuf, Error> {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../.local/foundation-worlds");
    std::fs::create_dir_all(&base)?;
    Ok(base.canonicalize()?)
}

pub fn generated(pos: CubePos) -> CubeRecord {
    let mut cube = CubeRecord::new(pos).unwrap();
    // Sparse procedural platforms leave all three streaming axes visible.
    for z in 4..12 {
        for x in 4..12 {
            cube.set(z * 16 + x, 1).unwrap();
        }
    }
    for y in 1..4 {
        cube.set(y * 256 + 8 * 16 + 8, 2 + (pos.y.unsigned_abs() % 3) as u32)
            .unwrap();
    }
    cube
}

pub fn run(steps: u64) -> Result<Outcome, Error> {
    run_with_backend(steps, Backend::Journal, |_, _, _| Ok(()))
}

pub(crate) const MACHINE_POSITIONS: [CubePos; 2] = [
    CubePos::new(-1, 1_000_000, 0),
    CubePos::new(-1, 1_000_001, 0),
];

pub(crate) fn initial_records(catalog: &Catalog) -> Result<Vec<CubeRecord>, Error> {
    let limits = PortLimits {
        items: 16,
        fluid_mb: 16000,
        kinds: 8,
    };
    let mut initial = Vec::new();
    for (pos, kind) in MACHINE_POSITIONS.iter().zip(["crusher", "smelter"]) {
        let mut machine = Machine::new(id(kind), limits, limits, Energy::new(4096, 32, 2)?)?;
        for _ in 0..64 {
            assert_eq!(machine.energy_mut().receive(32, 2)?, 64);
        }
        if kind == "crusher" {
            machine.inputs_mut().apply(
                catalog,
                &[],
                &[Amount {
                    resource: id("iron_ore"),
                    quantity: 12,
                }],
            )?;
        }
        let mut record = generated(*pos);
        record.set(256 + 8 * 16 + 8, if kind == "crusher" { 5 } else { 6 })?;
        record.replace_payload(machine.encode(catalog)?)?;
        initial.push(record);
    }
    Ok(initial)
}

pub(crate) fn advance(
    records: &mut std::collections::BTreeMap<CubePos, CubeRecord>,
    catalog: &Catalog,
) -> Result<(u64, u64), RecordError> {
    let mut machines = std::collections::BTreeMap::new();
    let mut spent = 0;
    for (pos, record) in records.iter() {
        if !MACHINE_POSITIONS.contains(pos) {
            return Err(RecordError::Invalid("non-machine tick participant"));
        }
        let mut machine = Machine::decode(record.payload(), catalog)
            .map_err(|_| RecordError::Invalid("factory machine decode"))?;
        spent += machine
            .tick(catalog)
            .map_err(|_| RecordError::Invalid("factory tick"))?
            .energy_used;
        machines.insert(*pos, machine);
    }
    let mut moved = 0;
    if machines.len() == 2 {
        let mut source = machines.remove(&MACHINE_POSITIONS[0]).unwrap();
        let target = machines.get_mut(&MACHINE_POSITIONS[1]).unwrap();
        moved = source
            .outputs_mut()
            .transfer(target.inputs_mut(), catalog, &id("iron_dust"), 4)
            .map_err(|_| RecordError::Invalid("factory transfer"))?;
        machines.insert(MACHINE_POSITIONS[0], source);
    }
    for (pos, machine) in machines {
        let bytes = machine
            .encode(catalog)
            .map_err(|_| RecordError::Invalid("factory encode"))?;
        records.get_mut(&pos).unwrap().replace_payload(bytes)?;
    }
    Ok((spent, moved))
}

pub fn run_with_backend(
    steps: u64,
    backend: Backend,
    mut observe: impl FnMut(&Streamer<WorldStore>, CubePos, u64) -> Result<(), Error>,
) -> Result<Outcome, Error> {
    if !(32..=4096).contains(&steps) {
        return Err("factory route steps must be 32..4096".into());
    }
    let catalog = catalog()?;
    let positions = MACHINE_POSITIONS;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = world_root(stamp)?;
    let mut store = WorldStore::open(&root, backend)?;
    let initial = initial_records(&catalog)?;
    store.save_batch(&initial.iter().collect::<Vec<_>>())?;
    let limits = Limits {
        max_cubes: 27,
        max_resident_bytes: 27 * 32768,
        max_cube_bytes: 32768,
        max_pending_jobs: 4,
    };
    let mut stream = Streamer::new(store, limits)?;
    let mut trace = 0xcbf29ce484222325u64;
    let (mut used, mut transfers, mut assertions, mut unloaded_checks, mut render_checks) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    for step in 0..steps {
        let (center, simulation) = match step % 8 {
            2 => (positions[0], [0; 3]),
            3 => (CubePos::new(-5, -1_000_000, 7), [0; 3]),
            5 => (positions[1], [0; 3]),
            6 => (CubePos::new(5, 1_000_012, -7), [0; 3]),
            _ => (positions[0], [1; 3]),
        };
        let tickets = Tickets::around(center, [1; 3], simulation)?;
        for job in stream.set_tickets(tickets) {
            assert!(stream.acknowledge_canceled(job));
        }
        for _ in 0..32 {
            let plan = stream.plan(4, 27)?;
            if plan.loads.is_empty() {
                break;
            }
            for job in plan.loads.into_iter().rev() {
                let cube = stream.read_or_generate(job, generated)?;
                assert!(stream.complete(job, cube)?);
            }
        }
        assert_eq!(stream.resident_positions().count(), 27);
        for _ in 0..6 {
            let before = load_machines(&mut stream, &positions, &catalog)?;
            let active: Vec<_> = positions
                .iter()
                .copied()
                .filter(|p| stream.simulates(*p))
                .collect();
            if !active.is_empty() {
                let (spent, moved) =
                    stream.transact(&active, |records| advance(records, &catalog))?;
                used += spent;
                transfers += moved;
            }
            let after = load_machines(&mut stream, &positions, &catalog)?;
            for ((pos, old), new) in positions.iter().zip(&before).zip(&after) {
                if !stream.simulates(*pos) {
                    assert_eq!(old, new, "inactive cube advanced machine state");
                    if stream.renders(*pos) {
                        render_checks += 1;
                    } else {
                        unloaded_checks += 1;
                    }
                }
            }
            assert_eq!(mass(&after), 24);
            assert_eq!(
                after.iter().map(|m| m.energy().stored()).sum::<u64>() + used,
                8192
            );
            assert!(stream.charged_bytes() <= limits.max_resident_bytes);
            for machine in &after {
                for byte in machine.encode(&catalog)? {
                    trace = (trace ^ u64::from(byte)).wrapping_mul(0x100000001b3);
                }
            }
            assertions += 3;
        }
        observe(&stream, center, step)?;
    }
    let final_machines = load_machines(&mut stream, &positions, &catalog)?;
    let final_state = final_machines
        .iter()
        .map(|m| m.encode(&catalog))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(final_machines[1].outputs().amount(&id("iron_ingot")), 12);
    assert_eq!(used, 1824);
    assert_eq!(transfers, 24);
    for job in stream.set_tickets(Tickets::default()) {
        assert!(stream.acknowledge_canceled(job));
    }
    stream.plan(0, 27)?;
    let stats = stream.stats().clone();
    let sequence = stream.storage_mut().sequence()?;
    assert_eq!(stream.resident_positions().count(), 0);
    drop(stream);
    let mut reopened = WorldStore::open(&root, backend)?;
    for (pos, expected) in positions.iter().zip(&final_state) {
        let cube = reopened
            .load(*pos)?
            .ok_or("factory cube missing after reopen")?;
        assert_eq!(cube.payload(), expected);
        assert_eq!(
            Machine::decode(cube.payload(), &catalog)?.encode(&catalog)?,
            *expected
        );
        assertions += 2;
    }
    Ok(Outcome {
        report: json!({"status":"PASS","fixture":"synthetic; not GTNH recipe parity","route_steps":steps,"simulated_route_ticks":steps*6,"final_ingots":12,"initial_material_units":24,"final_material_units":mass(&final_machines),"initial_energy":8192,"energy_consumed":used,"energy_remaining":8192-used,"transferred_dust":transfers,"unloaded_pause_checks":unloaded_checks,"render_only_pause_checks":render_checks,"trace_hash":format!("{trace:016x}"),"assertions":assertions,"storage_backend":backend.name(),"store_commits":sequence,"stream_transactions":stats.transactions,"loads":stats.loaded,"unloads":stats.unloaded,"peak_cubes":stats.peak_cubes,"peak_charged_bytes":stats.peak_bytes,"cube_byte_budget":limits.max_resident_bytes,"persistence":"real filesystem atomic multi-cube transactions; close/reopen verified","world_directory":root}),
        final_state,
    })
}

fn load_machines(
    stream: &mut Streamer<WorldStore>,
    positions: &[CubePos; 2],
    catalog: &Catalog,
) -> Result<Vec<Machine>, Error> {
    positions
        .iter()
        .map(|pos| {
            let cube = match stream.record(*pos) {
                Some(c) => c.clone(),
                None => stream
                    .storage_mut()
                    .load(*pos)?
                    .ok_or("factory state missing")?,
            };
            Ok(Machine::decode(cube.payload(), catalog)?)
        })
        .collect()
}
pub(crate) fn mass(machines: &[Machine]) -> u64 {
    let weight = |resource: &Id| match resource.as_str() {
        "lab:iron_ore" | "lab:iron_ingot" => 2,
        "lab:iron_dust" => 1,
        _ => 0,
    };
    machines
        .iter()
        .map(|m| {
            m.inputs()
                .contents()
                .chain(m.outputs().contents())
                .map(|(r, n)| weight(r) * n)
                .sum::<u64>()
                + m.held_inputs()
                    .iter()
                    .map(|a| weight(&a.resource) * a.quantity)
                    .sum::<u64>()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scenarios_created_in_the_same_clock_tick_never_share_a_world() {
        let paths = (0..16)
            .map(|_| world_root(42).unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(paths.len(), 16);
    }
    #[test]
    fn factory_replay_and_real_reopen_conserve_materials_and_energy() {
        let first = run(128).unwrap();
        let second = run(128).unwrap();
        assert_eq!(first.report["status"], "PASS");
        assert_eq!(first.final_state, second.final_state);
        assert_eq!(first.report["final_ingots"], 12);
        assert_eq!(first.report["energy_consumed"], 1824);
        assert_eq!(first.report["trace_hash"], second.report["trace_hash"]);
        assert!(first.report["unloaded_pause_checks"].as_u64().unwrap() > 0);
        assert!(first.report["render_only_pause_checks"].as_u64().unwrap() > 0);
        assert!(first.report["store_commits"].as_u64().unwrap() > 0);
    }
    #[cfg(feature = "paged")]
    #[test]
    fn paged_and_journal_backends_produce_identical_factory_states() {
        let journal = run(128).unwrap();
        let paged = run_with_backend(128, Backend::Paged, |_, _, _| Ok(())).unwrap();
        assert_eq!(journal.final_state, paged.final_state);
        assert_eq!(journal.report["trace_hash"], paged.report["trace_hash"]);
    }
}
