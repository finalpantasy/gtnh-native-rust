//! Separate synthetic logistics world. No manual-play world or visible factory changes.
use crate::{
    factory,
    world_storage::{Backend, WorldStore},
};
use industrial_sim::{
    content::{Amount, Catalog, Id},
    inventory::{Inventory, Limits as PortLimits},
    logistics::{Definition, Edge, Network, Report},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};
use voxel_core::{
    coords::CubePos,
    record::{CubeRecord, RecordError},
    storage::CubeStorage,
    streaming::{Limits, Streamer, Tickets},
};
type Error = Box<dyn std::error::Error>;
const SCHEDULER: CubePos = CubePos::new(-1, 1_000_000, 0);
const PORTS: [CubePos; 6] = [
    CubePos::new(-2, 1_000_000, 0),
    CubePos::new(0, 1_000_000, 0),
    CubePos::new(-1, 1_000_001, 0),
    CubePos::new(-1, 999_999, 0),
    CubePos::new(-1, 1_000_000, 1),
    CubePos::new(-1, 1_000_000, -1),
];
fn id(name: &str) -> Id {
    Id::new(format!("lab:{name}")).unwrap()
}
fn definition() -> Definition {
    let edges = [
        (0, 1, "iron_ore", 17),
        (1, 2, "iron_ore", 5),
        (1, 3, "iron_ore", 5),
        (0, 4, "water", 100),
        (4, 5, "water", 50),
    ]
    .into_iter()
    .enumerate()
    .map(|(n, (source, target, resource, limit))| Edge {
        id: n as u16,
        source,
        target,
        resource: id(resource),
        limit,
    })
    .collect();
    Definition {
        edges,
        max_attempts: 2,
        max_units: 64,
    }
}
fn port_limits(port: usize) -> PortLimits {
    let (items, fluid_mb) = match port {
        0 => (1000, 10000),
        1 => (32, 0),
        2 | 3 => (1000, 0),
        4 => (0, 512),
        5 => (0, 10000),
        _ => unreachable!(),
    };
    PortLimits {
        items,
        fluid_mb,
        kinds: 2,
    }
}
fn initial(catalog: &Catalog) -> Result<Vec<CubeRecord>, Error> {
    let mut scheduler = factory::generated(SCHEDULER);
    scheduler.replace_payload(Network::new(definition(), catalog)?.encode(catalog)?)?;
    let mut records = vec![scheduler];
    for (index, pos) in PORTS.into_iter().enumerate() {
        let mut inventory = Inventory::new(port_limits(index))?;
        if index == 0 {
            inventory.apply(
                catalog,
                &[],
                &[
                    Amount {
                        resource: id("iron_ore"),
                        quantity: 1000,
                    },
                    Amount {
                        resource: id("water"),
                        quantity: 10000,
                    },
                ],
            )?;
        }
        let mut cube = factory::generated(pos);
        cube.replace_payload(serde_json::to_vec(&inventory)?)?;
        records.push(cube);
    }
    Ok(records)
}
fn stream<S: CubeStorage>(storage: S) -> Result<Streamer<S>, Error> {
    Ok(Streamer::new(
        storage,
        Limits {
            max_cubes: 27,
            max_resident_bytes: 27 * 32768,
            max_cube_bytes: 32768,
            max_pending_jobs: 4,
        },
    )?)
}
fn focus<S: CubeStorage>(
    world: &mut Streamer<S>,
    center: CubePos,
    render: [u16; 3],
    simulate: [u16; 3],
) -> Result<(), Error> {
    for job in world.set_tickets(Tickets::around(center, render, simulate)?) {
        assert!(world.acknowledge_canceled(job));
    }
    for _ in 0..16 {
        let jobs = world.plan(4, 27)?.loads;
        if jobs.is_empty() {
            break;
        }
        for job in jobs {
            let cube = world.read_or_generate(job, factory::generated)?;
            assert!(world.complete(job, cube)?);
        }
    }
    Ok(())
}
fn decode_port(
    record: &CubeRecord,
    index: usize,
    catalog: &Catalog,
) -> Result<Inventory, RecordError> {
    let inventory: Inventory = serde_json::from_slice(record.payload())
        .map_err(|_| RecordError::Invalid("logistics port encoding"))?;
    inventory
        .validate(catalog)
        .map_err(|_| RecordError::Invalid("logistics inventory"))?;
    if inventory.limits() != port_limits(index) {
        return Err(RecordError::Invalid("logistics fixture capacity changed"));
    }
    Ok(inventory)
}
fn advance<S: CubeStorage>(
    world: &mut Streamer<S>,
    catalog: &Catalog,
) -> Result<Option<Report>, Error> {
    if !world.simulates(SCHEDULER) {
        return Ok(None);
    }
    let active: Vec<_> = PORTS
        .into_iter()
        .enumerate()
        .filter(|(_, pos)| world.simulates(*pos))
        .collect();
    let participants: Vec<_> = std::iter::once(SCHEDULER)
        .chain(active.iter().map(|(_, pos)| *pos))
        .collect();
    let report = world.transact(&participants, |records| {
        let mut network = Network::decode(records[&SCHEDULER].payload(), &definition(), catalog)
            .map_err(|_| RecordError::Invalid("logistics scheduler"))?;
        let mut ports = BTreeMap::new();
        for &(index, pos) in &active {
            ports.insert(index as u8, decode_port(&records[&pos], index, catalog)?);
        }
        let report = network
            .step(catalog, &mut ports)
            .map_err(|_| RecordError::Invalid("logistics tick"))?;
        for &(index, pos) in &active {
            let bytes = serde_json::to_vec(&ports[&(index as u8)])
                .map_err(|_| RecordError::Invalid("logistics port encode"))?;
            records.get_mut(&pos).unwrap().replace_payload(bytes)?;
        }
        let bytes = network
            .encode(catalog)
            .map_err(|_| RecordError::Invalid("logistics scheduler encode"))?;
        records
            .get_mut(&SCHEDULER)
            .unwrap()
            .replace_payload(bytes)?;
        Ok(report)
    })?;
    Ok(Some(report))
}
fn snapshot<S: CubeStorage>(
    world: &mut Streamer<S>,
) -> Result<BTreeMap<CubePos, CubeRecord>, Error> {
    let mut records = BTreeMap::new();
    for pos in std::iter::once(SCHEDULER).chain(PORTS) {
        let cube = if let Some(cube) = world.record(pos) {
            cube.clone()
        } else {
            world
                .storage_mut()
                .load(pos)?
                .ok_or("missing logistics fixture record")?
        };
        records.insert(pos, cube);
    }
    Ok(records)
}

fn audit(records: &BTreeMap<CubePos, CubeRecord>, catalog: &Catalog) -> Result<[u128; 2], Error> {
    Network::decode(records[&SCHEDULER].payload(), &definition(), catalog)?;
    let mut totals = [0u128; 2];
    for (index, pos) in PORTS.into_iter().enumerate() {
        let inventory = decode_port(&records[&pos], index, catalog)?;
        totals[0] += u128::from(inventory.amount(&id("iron_ore")));
        totals[1] += u128::from(inventory.amount(&id("water")));
    }
    assert_eq!(totals, [1000, 10000]);
    Ok(totals)
}
struct Checks {
    ticks: u64,
    active_ticks: u64,
    paused_ticks: u64,
    inactive_port_checks: u64,
    unavailable: u64,
    transfers: u64,
    moved_units: u64,
    trace: u64,
    assertions: u64,
}
impl Default for Checks {
    fn default() -> Self {
        Self {
            ticks: 0,
            active_ticks: 0,
            paused_ticks: 0,
            inactive_port_checks: 0,
            unavailable: 0,
            transfers: 0,
            moved_units: 0,
            trace: 0xcbf29ce484222325,
            assertions: 0,
        }
    }
}
fn checked_tick<S: CubeStorage>(
    world: &mut Streamer<S>,
    catalog: &Catalog,
    checks: &mut Checks,
) -> Result<(), Error> {
    let before = snapshot(world)?;
    let active: Vec<_> = PORTS.into_iter().filter(|p| world.simulates(*p)).collect();
    let report = advance(world, catalog)?;
    let after = snapshot(world)?;
    audit(&after, catalog)?;
    if let Some(report) = report {
        assert!(report.attempts <= 2 && report.moved_units <= 64);
        checks.active_ticks += 1;
        checks.unavailable += u64::from(report.unavailable);
        checks.transfers += u64::from(report.transfers);
        checks.moved_units += report.moved_units;
    } else {
        assert_eq!(before, after);
        checks.paused_ticks += 1;
    }
    for pos in PORTS {
        if !active.contains(&pos) {
            assert_eq!(before[&pos], after[&pos]);
            checks.inactive_port_checks += 1;
        }
    }
    assert!(world.resident_positions().count() <= 27 && world.charged_bytes() <= 27 * 32768);
    for cube in after.values() {
        for byte in cube.encode() {
            checks.trace = (checks.trace ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    checks.ticks += 1;
    checks.assertions += 4;
    Ok(())
}
fn unload<S: CubeStorage>(world: &mut Streamer<S>) -> Result<(), Error> {
    for job in world.set_tickets(Tickets::default()) {
        assert!(world.acknowledge_canceled(job));
    }
    world.plan(0, 27)?;
    assert_eq!(world.resident_positions().count(), 0);
    Ok(())
}
pub fn run(backend: Backend) -> Result<Value, Error> {
    let catalog = factory::catalog()?;
    let root = factory::world_root(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos())?;
    let records = initial(&catalog)?;
    let mut store = WorldStore::open(&root, backend)?;
    store.save_batch(&records.iter().collect::<Vec<_>>())?;
    let mut world = stream(store)?;
    let mut checks = Checks::default();
    let mut mid_reopen_ticks = 0;
    for step in 0..128 {
        let (center, render, simulate) = match step % 8 {
            1 => (SCHEDULER, [1; 3], [1, 0, 0]),
            2 => (CubePos::new(-1, -1_000_000, 0), [1; 3], [1; 3]),
            3 => (SCHEDULER, [1; 3], [0; 3]),
            5 => (SCHEDULER, [0; 3], [1; 3]),
            6 => (PORTS[1], [1; 3], [0; 3]),
            _ => (SCHEDULER, [1; 3], [1; 3]),
        };
        focus(&mut world, center, render, simulate)?;
        for _ in 0..4 {
            checked_tick(&mut world, &catalog, &mut checks)?;
        }
        if step == 63 {
            let before = snapshot(&mut world)?;
            mid_reopen_ticks =
                Network::decode(before[&SCHEDULER].payload(), &definition(), &catalog)?.ticks();
            unload(&mut world)?;
            drop(world);
            world = stream(WorldStore::open(&root, backend)?)?;
            assert_eq!(before, snapshot(&mut world)?);
        }
    }
    focus(&mut world, SCHEDULER, [1; 3], [1; 3])?;
    let mut drained = false;
    for _ in 0..1000 {
        let state = snapshot(&mut world)?;
        let ore = decode_port(&state[&PORTS[0]], 0, &catalog)?.amount(&id("iron_ore"))
            + decode_port(&state[&PORTS[1]], 1, &catalog)?.amount(&id("iron_ore"));
        let water = decode_port(&state[&PORTS[0]], 0, &catalog)?.amount(&id("water"))
            + decode_port(&state[&PORTS[4]], 4, &catalog)?.amount(&id("water"));
        if ore == 0 && water == 0 {
            drained = true;
            break;
        }
        checked_tick(&mut world, &catalog, &mut checks)?;
    }
    assert!(
        drained,
        "bounded logistics route never delivered its original resources"
    );
    let final_state = snapshot(&mut world)?;
    audit(&final_state, &catalog)?;
    let sink_a = decode_port(&final_state[&PORTS[2]], 2, &catalog)?.amount(&id("iron_ore"));
    let sink_b = decode_port(&final_state[&PORTS[3]], 3, &catalog)?.amount(&id("iron_ore"));
    assert!(sink_a > 0 && sink_b > 0);
    assert_eq!(sink_a + sink_b, 1000);
    assert_eq!(
        decode_port(&final_state[&PORTS[5]], 5, &catalog)?.amount(&id("water")),
        10000
    );
    assert!(
        checks.paused_ticks > 0
            && checks.unavailable > 0
            && checks.inactive_port_checks > 0
            && checks.transfers > 0
            && mid_reopen_ticks > 0
    );
    let network = Network::decode(final_state[&SCHEDULER].payload(), &definition(), &catalog)?;
    let stats = world.stats().clone();
    unload(&mut world)?;
    drop(world);
    let mut reopened = WorldStore::open(&root, backend)?;
    for (pos, cube) in &final_state {
        assert_eq!(reopened.load(*pos)?.as_ref(), Some(cube));
        checks.assertions += 1;
    }
    Ok(
        json!({"status":"PASS","backend":backend.name(),"world_directory":root,"scenario_ticks":checks.ticks,"active_ticks":checks.active_ticks,"scheduler_ticks":network.ticks(),"paused_ticks":checks.paused_ticks,"inactive_port_checks":checks.inactive_port_checks,
        "unavailable_edges":checks.unavailable,"transfers":checks.transfers,"moved_units":checks.moved_units,"item_sinks":[sink_a,sink_b],"water_sink_mb":10000,"conserved_totals":[1000,10000],"mid_reopen_scheduler_ticks":mid_reopen_ticks,"trace_hash":format!("{:016x}",checks.trace),"assertions":checks.assertions,
        "streaming_since_mid_reopen":{"loaded":stats.loaded,"unloaded":stats.unloaded,"peak_cubes":stats.peak_cubes,"peak_bytes":stats.peak_bytes,"transactions":stats.transactions},"final_reopen_records":final_state.len(),"scope":"synthetic filtered aggregate ports; atomic endpoint/cursor commits; audit reads persisted unavailable endpoints explicitly; no GTNH pipe or slot parity"}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use voxel_core::{
        storage::{MemoryStorage, StoreError},
        streaming::StreamError,
    };
    struct Rejecting {
        inner: WorldStore,
        reject: bool,
    }
    impl CubeStorage for Rejecting {
        fn load(&mut self, pos: CubePos) -> Result<Option<CubeRecord>, StoreError> {
            self.inner.load(pos)
        }
        fn save(&mut self, cube: &CubeRecord) -> Result<(), StoreError> {
            self.save_batch(&[cube])
        }
        fn save_batch(&mut self, cubes: &[&CubeRecord]) -> Result<(), StoreError> {
            if self.reject {
                Err(StoreError::Io(std::io::Error::other(
                    "injected logistics write rejection",
                )))
            } else {
                self.inner.save_batch(cubes)
            }
        }
    }
    fn filesystem_rollback(backend: Backend) {
        let c = factory::catalog().unwrap();
        let root = factory::world_root(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        )
        .unwrap();
        let records = initial(&c).unwrap();
        let mut store = WorldStore::open(&root, backend).unwrap();
        store
            .save_batch(&records.iter().collect::<Vec<_>>())
            .unwrap();
        let sequence = store.sequence().unwrap();
        let mut world = stream(Rejecting {
            inner: store,
            reject: true,
        })
        .unwrap();
        focus(&mut world, SCHEDULER, [1; 3], [1; 3]).unwrap();
        let before = snapshot(&mut world).unwrap();
        assert!(advance(&mut world, &c).is_err());
        assert_eq!(snapshot(&mut world).unwrap(), before);
        assert_eq!(world.storage_mut().inner.sequence().unwrap(), sequence);
        drop(world);
        let mut store = WorldStore::open(&root, backend).unwrap();
        for (pos, cube) in &before {
            assert_eq!(store.load(*pos).unwrap().as_ref(), Some(cube));
        }
        let mut world = stream(Rejecting {
            inner: store,
            reject: false,
        })
        .unwrap();
        focus(&mut world, SCHEDULER, [1; 3], [1; 3]).unwrap();
        assert_eq!(advance(&mut world, &c).unwrap().unwrap().moved_units, 17);
        let after = snapshot(&mut world).unwrap();
        drop(world);
        let mut reopened = WorldStore::open(&root, backend).unwrap();
        for (pos, cube) in after {
            assert_eq!(reopened.load(pos).unwrap(), Some(cube));
        }
    }
    #[test]
    fn journal_failure_reopens_the_same_scheduler_and_endpoint_snapshots() {
        filesystem_rollback(Backend::Journal);
    }
    #[cfg(feature = "paged")]
    #[test]
    fn paged_failure_reopens_the_same_scheduler_and_endpoint_snapshots() {
        filesystem_rollback(Backend::Paged);
    }
    #[test]
    fn failed_multicube_commit_keeps_scheduler_and_all_ports_unchanged() {
        let c = factory::catalog().unwrap();
        let mut storage = MemoryStorage::default();
        let records = initial(&c).unwrap();
        storage
            .save_batch(&records.iter().collect::<Vec<_>>())
            .unwrap();
        let mut world = stream(storage).unwrap();
        focus(&mut world, SCHEDULER, [1; 3], [1; 3]).unwrap();
        let before = snapshot(&mut world).unwrap();
        world.storage_mut().reject_writes = true;
        let failure = advance(&mut world, &c).unwrap_err();
        assert!(
            failure
                .downcast_ref::<StreamError>()
                .is_some_and(|e| matches!(e, StreamError::Store(StoreError::Io(_))))
        );
        assert_eq!(snapshot(&mut world).unwrap(), before);
        for (pos, cube) in &before {
            assert_eq!(world.storage_mut().load(*pos).unwrap().as_ref(), Some(cube));
        }
        world.storage_mut().reject_writes = false;
        let r = advance(&mut world, &c).unwrap().unwrap();
        assert_eq!(r.moved_units, 17);
        let after = snapshot(&mut world).unwrap();
        let n = Network::decode(after[&SCHEDULER].payload(), &definition(), &c).unwrap();
        assert_eq!(n.ticks(), 1);
        assert_eq!(
            decode_port(&after[&PORTS[0]], 0, &c)
                .unwrap()
                .amount(&id("iron_ore")),
            983
        );
        assert_eq!(
            decode_port(&after[&PORTS[1]], 1, &c)
                .unwrap()
                .amount(&id("iron_ore")),
            17
        );
        for (pos, cube) in &after {
            assert_eq!(world.storage_mut().load(*pos).unwrap().as_ref(), Some(cube));
        }
    }
    #[test]
    fn scheduler_and_unavailable_endpoints_never_advance_outside_simulation() {
        let c = factory::catalog().unwrap();
        let mut storage = MemoryStorage::default();
        let records = initial(&c).unwrap();
        storage
            .save_batch(&records.iter().collect::<Vec<_>>())
            .unwrap();
        let mut world = stream(storage).unwrap();
        focus(&mut world, SCHEDULER, [1; 3], [0; 3]).unwrap();
        let before = snapshot(&mut world).unwrap();
        let r = advance(&mut world, &c).unwrap().unwrap();
        assert_eq!(r.unavailable, 2);
        assert_eq!(r.moved_units, 0);
        let after = snapshot(&mut world).unwrap();
        for pos in PORTS {
            assert_eq!(before[&pos], after[&pos]);
        }
        focus(&mut world, PORTS[1], [1; 3], [0; 3]).unwrap();
        let before = snapshot(&mut world).unwrap();
        assert!(advance(&mut world, &c).unwrap().is_none());
        assert_eq!(before, snapshot(&mut world).unwrap());
        focus(&mut world, CubePos::new(-1, -1_000_000, 0), [1; 3], [1; 3]).unwrap();
        let before = snapshot(&mut world).unwrap();
        assert!(advance(&mut world, &c).unwrap().is_none());
        assert_eq!(before, snapshot(&mut world).unwrap());
    }
}
