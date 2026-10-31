//! Six-record synthetic automation fixture, separate from manual-play worlds.
pub mod interruption;
use crate::{
    factory, quest_factory,
    world_storage::{Backend, WorldStore},
};
use industrial_sim::{
    content::{Amount, Catalog, Id},
    inventory::{Inventory, Limits as PortLimits},
    logistics::{Definition, Edge, Network},
    machine::{Energy, Machine, State as MachineState},
};
use quest_core::{
    catalog::{Catalog as Quests, QuestId},
    progress::Book,
};
use serde::Serialize;
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
const CRUSHER: CubePos = quest_factory::CRUSHER;
const SMELTER: CubePos = quest_factory::SMELTER;
const LEDGER: CubePos = quest_factory::LEDGER;
const WALLET: CubePos = quest_factory::PLAYER;
const SCHEDULER: CubePos = CubePos::new(-2, 1_000_000, 0);
const HOPPER: CubePos = CubePos::new(-2, 1_000_001, 0);
const POSITIONS: [CubePos; 6] = [CRUSHER, SMELTER, LEDGER, WALLET, SCHEDULER, HOPPER];
fn id(name: &str) -> Id {
    Id::new(format!("lab:{name}")).unwrap()
}
fn qid(n: u128) -> QuestId {
    QuestId::from_u128(n)
}
fn quests() -> Result<Quests, Error> {
    Ok(Quests::from_json(include_bytes!(
        "../../../fixtures/quests-v1.json"
    ))?)
}
fn port_limits() -> PortLimits {
    PortLimits {
        items: 2,
        fluid_mb: 0,
        kinds: 1,
    }
}
fn input_limits(index: usize) -> PortLimits {
    if index == 0 {
        PortLimits {
            items: 12,
            fluid_mb: 0,
            kinds: 1,
        }
    } else {
        port_limits()
    }
}
fn contains_only(inventory: &Inventory, resource: &Id) -> bool {
    inventory.contents().all(|(id, _)| id == resource)
}
fn definition() -> Definition {
    Definition {
        edges: vec![
            Edge {
                id: 0,
                source: 0,
                target: 1,
                resource: id("iron_dust"),
                limit: 4,
            },
            Edge {
                id: 1,
                source: 2,
                target: 3,
                resource: id("iron_ingot"),
                limit: 1,
            },
        ],
        max_attempts: 2,
        max_units: 4,
    }
}
fn invalid(_: impl std::fmt::Display) -> RecordError {
    RecordError::Invalid("automated factory state or tick")
}
struct State {
    machines: [Machine; 2],
    network: Network,
    hopper: Inventory,
    book: Book,
    submitted: Inventory,
    wallet: Inventory,
}
impl State {
    fn decode(
        records: &BTreeMap<CubePos, CubeRecord>,
        items: &Catalog,
        quests: &Quests,
    ) -> Result<Self, Error> {
        let payload = |pos| {
            records
                .get(&pos)
                .map(CubeRecord::payload)
                .ok_or("missing automation participant")
        };
        let (book, submitted) = quest_factory::decode_ledger(payload(LEDGER)?, items, quests)?;
        let hopper: Inventory = serde_json::from_slice(payload(HOPPER)?)?;
        let wallet: Inventory = serde_json::from_slice(payload(WALLET)?)?;
        hopper.validate(items)?;
        wallet.validate(items)?;
        let machines = [
            Machine::decode(payload(CRUSHER)?, items)?,
            Machine::decode(payload(SMELTER)?, items)?,
        ];
        for (index, machine) in machines.iter().enumerate() {
            if machine.kind() != &id(if index == 0 { "crusher" } else { "smelter" })
                || machine.inputs().limits() != input_limits(index)
                || machine.outputs().limits() != port_limits()
                || machine.energy().capacity() != 4096
                || machine.energy().max_voltage() != 32
                || !contains_only(
                    machine.inputs(),
                    &id(if index == 0 { "iron_ore" } else { "iron_dust" }),
                )
                || !contains_only(
                    machine.outputs(),
                    &id(if index == 0 {
                        "iron_dust"
                    } else {
                        "iron_ingot"
                    }),
                )
            {
                return Err("changed automation machine definition or cargo".into());
            }
        }
        if hopper.limits() != port_limits()
            || submitted.limits()
                != (PortLimits {
                    items: 16,
                    fluid_mb: 0,
                    kinds: 2,
                })
            || wallet.limits()
                != (PortLimits {
                    items: 4,
                    fluid_mb: 0,
                    kinds: 1,
                })
            || !contains_only(&hopper, &id("iron_ingot"))
            || !contains_only(&submitted, &id("iron_ingot"))
            || !contains_only(&wallet, &id("reward_token"))
        {
            return Err("changed automation port definition or cargo".into());
        }
        Ok(Self {
            machines,
            network: Network::decode(payload(SCHEDULER)?, &definition(), items)?,
            hopper,
            book,
            submitted,
            wallet,
        })
    }
    fn write(
        self,
        records: &mut BTreeMap<CubePos, CubeRecord>,
        items: &Catalog,
        quests: &Quests,
    ) -> Result<(), RecordError> {
        let ledger =
            quest_factory::encode_ledger(&self.book, self.submitted, quests).map_err(invalid)?;
        let payloads = [
            self.machines[0].encode(items).map_err(invalid)?,
            self.machines[1].encode(items).map_err(invalid)?,
            ledger,
            serde_json::to_vec(&self.wallet).map_err(invalid)?,
            self.network.encode(items).map_err(invalid)?,
            serde_json::to_vec(&self.hopper).map_err(invalid)?,
        ];
        for (pos, bytes) in POSITIONS.into_iter().zip(payloads) {
            records
                .get_mut(&pos)
                .ok_or_else(|| invalid("missing participant"))?
                .replace_payload(bytes)?;
        }
        Ok(())
    }
    fn audit(&self, items: &Catalog) -> Result<(u64, u64, u64), Error> {
        let submitted = self.submitted.amount(&id("iron_ingot"));
        let mass =
            factory::mass(&self.machines) + 2 * (self.hopper.amount(&id("iron_ingot")) + submitted);
        let energy: u64 = self.machines.iter().map(|m| m.energy().stored()).sum();
        let tokens = self.wallet.amount(&id("reward_token"));
        self.wallet.validate(items)?;
        if mass != 24
            || energy > 8192
            || submitted > 12
            || tokens != if self.book.has_claimed(&qid(2)) { 2 } else { 0 }
        {
            return Err("automation material/energy/reward ledger mismatch".into());
        }
        if self.book.task_progress(&qid(2), 0) != submitted {
            return Err("submission progress mismatch".into());
        }
        let job_energy: u128 = self
            .machines
            .iter()
            .zip([(5u128, 8u128), (7, 16)])
            .map(|(m, (duration, eu))| {
                (u128::from(m.completed()) * duration + u128::from(m.progress().unwrap_or(0))) * eu
            })
            .sum();
        if job_energy != u128::from(8192 - energy) {
            return Err("job energy debit mismatch".into());
        }
        Ok((submitted, 8192 - energy, tokens))
    }
}
fn initial(items: &Catalog, quests: &Quests) -> Result<Vec<CubeRecord>, Error> {
    let mut records = quest_factory::initial_records(quests)?;
    for (index, pos) in [CRUSHER, SMELTER].into_iter().enumerate() {
        let input = input_limits(index);
        let mut machine = Machine::new(
            id(if index == 0 { "crusher" } else { "smelter" }),
            input,
            port_limits(),
            Energy::new(4096, 32, 2)?,
        )?;
        for _ in 0..64 {
            machine.energy_mut().receive(32, 2)?;
        }
        if index == 0 {
            machine.inputs_mut().apply(
                items,
                &[],
                &[Amount {
                    resource: id("iron_ore"),
                    quantity: 12,
                }],
            )?;
        }
        let mut cube = factory::generated(pos);
        cube.replace_payload(machine.encode(items)?)?;
        records.push(cube);
    }
    let mut network = factory::generated(SCHEDULER);
    network.replace_payload(Network::new(definition(), items)?.encode(items)?)?;
    records.push(network);
    let mut hopper = factory::generated(HOPPER);
    hopper.replace_payload(serde_json::to_vec(&Inventory::new(port_limits())?)?)?;
    records.push(hopper);
    Ok(records)
}
#[derive(Clone, Copy, Debug, Default, Serialize)]
struct Step {
    energy: u64,
    transferred: u64,
    blocked_machines: u64,
    rewards: u64,
}
fn tick(
    records: &mut BTreeMap<CubePos, CubeRecord>,
    items: &Catalog,
    quests: &Quests,
    submit: bool,
) -> Result<Step, RecordError> {
    let mut state = State::decode(records, items, quests).map_err(invalid)?;
    let mut result = Step::default();
    for machine in &mut state.machines {
        let report = machine.tick(items).map_err(invalid)?;
        result.energy += report.energy_used;
        result.blocked_machines += u64::from(report.state == MachineState::OutputBlocked);
    }
    // Port supplies are captured after machine ticks. An ingredient that arrives
    // through the network can first be processed on the following fixture tick.
    let mut ports = [
        (0, state.machines[0].outputs().clone()),
        (1, state.machines[1].inputs().clone()),
        (2, state.machines[1].outputs().clone()),
        (3, state.hopper),
    ]
    .into_iter()
    .collect();
    result.transferred = state
        .network
        .step(items, &mut ports)
        .map_err(invalid)?
        .moved_units;
    *state.machines[0].outputs_mut() = ports.remove(&0).unwrap();
    *state.machines[1].inputs_mut() = ports.remove(&1).unwrap();
    *state.machines[1].outputs_mut() = ports.remove(&2).unwrap();
    state.hopper = ports.remove(&3).unwrap();
    if !state.book.is_complete(&qid(1)) {
        state
            .book
            .record_total(quests, &qid(1), 0, state.machines[0].completed())
            .map_err(invalid)?;
        if state.book.task_progress(&qid(1), 0) >= 1 {
            state.book.complete(quests, &qid(1)).map_err(invalid)?;
        }
    }
    if submit
        && !state.book.is_complete(&qid(2))
        && state.book.unlocked(quests, &qid(2)).map_err(invalid)?
    {
        state
            .hopper
            .transfer(&mut state.submitted, items, &id("iron_ingot"), 1)
            .map_err(invalid)?;
        state
            .book
            .record_total(
                quests,
                &qid(2),
                0,
                state.submitted.amount(&id("iron_ingot")),
            )
            .map_err(invalid)?;
        if state.book.task_progress(&qid(2), 0) >= 12 {
            state.book.complete(quests, &qid(2)).map_err(invalid)?;
        }
    }
    if state.book.is_complete(&qid(2)) && state.book.claim(quests, &qid(2)).map_err(invalid)? {
        state
            .wallet
            .apply(
                items,
                &[],
                &[Amount {
                    resource: id("reward_token"),
                    quantity: 2,
                }],
            )
            .map_err(invalid)?;
        result.rewards = 2;
    }
    state.audit(items).map_err(invalid)?;
    state.write(records, items, quests)?;
    Ok(result)
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
fn focus<S: CubeStorage>(world: &mut Streamer<S>, mode: u8) -> Result<(), Error> {
    let center = if mode == 2 {
        CubePos::new(-1, 1_000_020, 0)
    } else {
        CRUSHER
    };
    let simulation = if mode == 1 { [0; 3] } else { [1; 3] };
    for job in world.set_tickets(Tickets::around(center, [1; 3], simulation)?) {
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
fn advance<S: CubeStorage>(
    world: &mut Streamer<S>,
    items: &Catalog,
    quests: &Quests,
    submit: bool,
) -> Result<Option<Step>, Error> {
    if POSITIONS.into_iter().any(|p| !world.simulates(p)) {
        return Ok(None);
    }
    Ok(Some(world.transact(&POSITIONS, |r| {
        tick(r, items, quests, submit)
    })?))
}
fn snapshot<S: CubeStorage>(
    world: &mut Streamer<S>,
) -> Result<BTreeMap<CubePos, CubeRecord>, Error> {
    POSITIONS
        .into_iter()
        .map(|p| {
            let record = if let Some(c) = world.record(p) {
                c.clone()
            } else {
                world
                    .storage_mut()
                    .load(p)?
                    .ok_or("missing automation record")?
            };
            Ok((p, record))
        })
        .collect()
}
fn reopen(
    mut world: Streamer<WorldStore>,
    root: &std::path::Path,
    backend: Backend,
) -> Result<Streamer<WorldStore>, Error> {
    world.flush_all()?;
    let expected = snapshot(&mut world)?;
    drop(world);
    let mut world = stream(WorldStore::open(root, backend)?)?;
    focus(&mut world, 0)?;
    assert_eq!(snapshot(&mut world)?, expected);
    Ok(world)
}
pub fn run(backend: Backend) -> Result<Value, Error> {
    let items = factory::catalog()?;
    let quests = quests()?;
    let root = factory::world_root(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos())?;
    let mut store = WorldStore::open(&root, backend)?;
    let records = initial(&items, &quests)?;
    store.save_batch(&records.iter().collect::<Vec<_>>())?;
    let mut world = stream(store)?;
    let (mut active, mut pauses, mut blocked, mut energy, mut rewards, mut moved) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut active_reopen, mut submission_reopen) = (false, false);
    let mut trace = 0xcbf29ce484222325u64;
    let mut peak_cubes = 0;
    let mut peak_bytes = 0;
    for step in 0..512 {
        let mode = match step % 32 {
            24..=27 => 1,
            28..=31 => 2,
            _ => 0,
        };
        focus(&mut world, mode)?;
        let before = snapshot(&mut world)?;
        let result = advance(&mut world, &items, &quests, active >= 160)?;
        if let Some(result) = result {
            active += 1;
            blocked += result.blocked_machines;
            energy += result.energy;
            rewards += result.rewards;
            moved += result.transferred;
        } else {
            pauses += 1;
            assert_eq!(snapshot(&mut world)?, before);
        }
        let records = snapshot(&mut world)?;
        let state = State::decode(&records, &items, &quests)?;
        state.audit(&items)?;
        assert_eq!(state.network.ticks(), active);
        for cube in records.values() {
            for byte in cube.encode() {
                trace = (trace ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        }
        peak_cubes = peak_cubes.max(world.stats().peak_cubes);
        peak_bytes = peak_bytes.max(world.stats().peak_bytes);
        if step == 17 {
            assert!(state.machines.iter().any(|m| m.progress().is_some()));
            world = reopen(world, &root, backend)?;
            active_reopen = true;
        } else if !submission_reopen && state.submitted.amount(&id("iron_ingot")) == 6 {
            assert!(!state.book.is_complete(&qid(2)) && !state.book.has_claimed(&qid(2)));
            world = reopen(world, &root, backend)?;
            submission_reopen = true;
        }
    }
    focus(&mut world, 0)?;
    for _ in 0..20 {
        let report = advance(&mut world, &items, &quests, true)?.ok_or("final fixture inactive")?;
        assert_eq!(
            (report.energy, report.rewards, report.transferred),
            (0, 0, 0)
        );
    }
    let final_records = snapshot(&mut world)?;
    let state = State::decode(&final_records, &items, &quests)?;
    assert_eq!(state.audit(&items)?, (12, 1824, 2));
    assert_eq!((energy, rewards), (1824, 2));
    assert!(active_reopen && submission_reopen && blocked > 0 && pauses == 128);
    assert!(peak_cubes <= 27 && peak_bytes <= 27 * 32768);
    assert!(state.machines.iter().all(|m| m.held_inputs().is_empty()));
    world.flush_all()?;
    drop(world);
    let mut store = WorldStore::open(&root, backend)?;
    for (pos, expected) in &final_records {
        assert_eq!(store.load(*pos)?.as_ref(), Some(expected));
    }
    Ok(
        json!({"status":"PASS","backend":backend.name(),"world_directory":root,
        "route_steps":512,"active_ticks":active,"paused_ticks":pauses,"blocked_machine_ticks":blocked,
        "network_moved_units":moved,"initial_ore":12,"submitted_ingots":12,"weighted_material_units":24,
        "energy_consumed":energy,"reward_tokens":rewards,"repeated_claim_checks":20,
        "active_job_reopen":active_reopen,"six_submission_reopen":submission_reopen,"final_reopen_records":6,
        "peak_cubes":peak_cubes,"peak_charged_cube_bytes":peak_bytes,"trace_hash":format!("{trace:016x}"),
        "scope":"Synthetic recipes and quests, automatic staged six-record tick, coarse all-participants activation. No visible controls, production pipe semantics or GTNH recipe parity claim."}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use voxel_core::storage::MemoryStorage;
    fn setup() -> (Catalog, Quests, Streamer<MemoryStorage>) {
        let items = factory::catalog().unwrap();
        let quests = quests().unwrap();
        let mut store = MemoryStorage::default();
        let records = initial(&items, &quests).unwrap();
        store
            .save_batch(&records.iter().collect::<Vec<_>>())
            .unwrap();
        let mut world = stream(store).unwrap();
        focus(&mut world, 0).unwrap();
        (items, quests, world)
    }
    #[test]
    fn bounded_transport_backpressure_and_automatic_claim_conserve_every_tick() {
        let (items, quests, mut world) = setup();
        let mut blocked = 0;
        let mut rewards = 0;
        for t in 0..400 {
            let report = advance(&mut world, &items, &quests, t >= 160)
                .unwrap()
                .unwrap();
            blocked += report.blocked_machines;
            rewards += report.rewards;
            let state = State::decode(&snapshot(&mut world).unwrap(), &items, &quests).unwrap();
            state.audit(&items).unwrap();
            assert_eq!(state.network.ticks(), t + 1);
            if t == 159 {
                assert_eq!(state.submitted.amount(&id("iron_ingot")), 0);
                assert!(blocked > 0);
            }
        }
        let state = State::decode(&snapshot(&mut world).unwrap(), &items, &quests).unwrap();
        assert_eq!(state.audit(&items).unwrap(), (12, 1824, 2));
        assert_eq!(rewards, 2);
        assert!(state.book.is_complete(&qid(2)) && state.book.has_claimed(&qid(2)));
    }
    #[test]
    fn job_progress_and_completed_counts_must_reconcile_with_energy_debits() {
        let (items, quests, mut world) = setup();
        advance(&mut world, &items, &quests, true).unwrap().unwrap();
        let original = snapshot(&mut world).unwrap();
        for case in 0..3 {
            let mut records = original.clone();
            let mut value: Value = serde_json::from_slice(records[&CRUSHER].payload()).unwrap();
            match case {
                0 => value["energy"]["stored"] = json!(4080),
                1 => value["energy"]["stored"] = json!(4096),
                _ => value["completed"] = json!(u64::MAX),
            }
            records
                .get_mut(&CRUSHER)
                .unwrap()
                .replace_payload(serde_json::to_vec(&value).unwrap())
                .unwrap();
            let state = State::decode(&records, &items, &quests).unwrap();
            assert!(
                state.audit(&items).is_err(),
                "unreconciled energy case {case} was accepted"
            );
        }
    }
    #[test]
    fn changed_fixture_capacities_kinds_and_foreign_cargo_fail_decode() {
        let (items, quests, mut world) = setup();
        let original = snapshot(&mut world).unwrap();
        for case in 0..5 {
            let mut records = original.clone();
            let pos = match case {
                0 | 1 => CRUSHER,
                2 => HOPPER,
                3 => LEDGER,
                _ => WALLET,
            };
            let mut value: Value = serde_json::from_slice(records[&pos].payload()).unwrap();
            match case {
                0 => value["inputs"]["limits"]["items"] = json!(13),
                1 => value["kind"] = json!("lab:smelter"),
                2 => value["contents"]["lab:reward_token"] = json!(1),
                3 => value["submitted"]["limits"]["items"] = json!(17),
                _ => value["limits"]["items"] = json!(5),
            }
            records
                .get_mut(&pos)
                .unwrap()
                .replace_payload(serde_json::to_vec(&value).unwrap())
                .unwrap();
            assert!(
                State::decode(&records, &items, &quests).is_err(),
                "case {case} was accepted"
            );
        }
        assert_eq!(snapshot(&mut world).unwrap(), original);
    }
    #[test]
    fn render_only_or_unloaded_participants_pause_the_entire_fixture() {
        let (items, quests, mut world) = setup();
        for mode in [1, 2] {
            focus(&mut world, mode).unwrap();
            let before = snapshot(&mut world).unwrap();
            assert!(
                advance(&mut world, &items, &quests, true)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(snapshot(&mut world).unwrap(), before);
        }
    }
    #[test]
    fn rejected_write_changes_no_machine_port_or_cursor_and_retry_advances_once() {
        let (items, quests, mut world) = setup();
        let before = snapshot(&mut world).unwrap();
        world.storage_mut().reject_writes = true;
        assert!(advance(&mut world, &items, &quests, true).is_err());
        assert_eq!(snapshot(&mut world).unwrap(), before);
        for (pos, cube) in &before {
            assert_eq!(world.storage_mut().load(*pos).unwrap().as_ref(), Some(cube));
        }
        world.storage_mut().reject_writes = false;
        let report = advance(&mut world, &items, &quests, true).unwrap().unwrap();
        // The smelter starts empty, so only the crusher uses power on tick one.
        assert_eq!(report.energy, 8);
        let state = State::decode(&snapshot(&mut world).unwrap(), &items, &quests).unwrap();
        assert_eq!(state.network.ticks(), 1);
        assert_eq!(state.machines[0].progress(), Some(1));
    }
    fn prepare_final_submission<S: CubeStorage>(
        world: &mut Streamer<S>,
        items: &Catalog,
        quests: &Quests,
    ) {
        for _ in 0..400 {
            let state = State::decode(&snapshot(world).unwrap(), items, quests).unwrap();
            let n = state.submitted.amount(&id("iron_ingot"));
            if n == 11 && state.hopper.amount(&id("iron_ingot")) == 1 {
                return;
            }
            advance(world, items, quests, n < 11).unwrap().unwrap();
        }
        panic!("failed to prepare final submission");
    }
    #[test]
    fn full_wallet_rolls_back_final_delivery_quest_completion_and_scheduler_together() {
        let (items, quests, mut world) = setup();
        prepare_final_submission(&mut world, &items, &quests);
        world
            .transact(&[WALLET], |records| {
                let mut wallet: Inventory =
                    serde_json::from_slice(records[&WALLET].payload()).map_err(invalid)?;
                wallet
                    .apply(
                        &items,
                        &[],
                        &[Amount {
                            resource: id("reward_token"),
                            quantity: 4,
                        }],
                    )
                    .map_err(invalid)?;
                records
                    .get_mut(&WALLET)
                    .unwrap()
                    .replace_payload(serde_json::to_vec(&wallet).map_err(invalid)?)?;
                Ok(())
            })
            .unwrap();
        let before = snapshot(&mut world).unwrap();
        assert!(advance(&mut world, &items, &quests, true).is_err());
        assert_eq!(snapshot(&mut world).unwrap(), before);
        for (pos, cube) in &before {
            assert_eq!(world.storage_mut().load(*pos).unwrap().as_ref(), Some(cube));
        }
        let state = State::decode(&before, &items, &quests).unwrap();
        assert!(!state.book.is_complete(&qid(2)) && !state.book.has_claimed(&qid(2)));
        assert_eq!(state.submitted.amount(&id("iron_ingot")), 11);
        assert_eq!(state.hopper.amount(&id("iron_ingot")), 1);
        world
            .transact(&[WALLET], |records| {
                let mut wallet: Inventory =
                    serde_json::from_slice(records[&WALLET].payload()).map_err(invalid)?;
                wallet
                    .apply(
                        &items,
                        &[Amount {
                            resource: id("reward_token"),
                            quantity: 4,
                        }],
                        &[],
                    )
                    .map_err(invalid)?;
                records
                    .get_mut(&WALLET)
                    .unwrap()
                    .replace_payload(serde_json::to_vec(&wallet).map_err(invalid)?)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            advance(&mut world, &items, &quests, true)
                .unwrap()
                .unwrap()
                .rewards,
            2
        );
        assert_eq!(
            State::decode(&snapshot(&mut world).unwrap(), &items, &quests)
                .unwrap()
                .audit(&items)
                .unwrap(),
            (12, 1824, 2)
        );
    }
    struct Rejecting<S> {
        inner: S,
        reject: bool,
    }
    impl<S: CubeStorage> CubeStorage for Rejecting<S> {
        fn load(
            &mut self,
            pos: CubePos,
        ) -> Result<Option<CubeRecord>, voxel_core::storage::StoreError> {
            self.inner.load(pos)
        }
        fn save(&mut self, record: &CubeRecord) -> Result<(), voxel_core::storage::StoreError> {
            self.save_batch(&[record])
        }
        fn save_batch(
            &mut self,
            records: &[&CubeRecord],
        ) -> Result<(), voxel_core::storage::StoreError> {
            if self.reject {
                return Err(std::io::Error::other("injected automation admission failure").into());
            }
            self.inner.save_batch(records)
        }
    }
    fn filesystem_claim_rollback(backend: Backend) {
        let items = factory::catalog().unwrap();
        let quests = quests().unwrap();
        let root = factory::world_root(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        )
        .unwrap();
        let mut store = WorldStore::open(&root, backend).unwrap();
        let initial = initial(&items, &quests).unwrap();
        store
            .save_batch(&initial.iter().collect::<Vec<_>>())
            .unwrap();
        let mut world = stream(Rejecting {
            inner: store,
            reject: false,
        })
        .unwrap();
        focus(&mut world, 0).unwrap();
        prepare_final_submission(&mut world, &items, &quests);
        let before = snapshot(&mut world).unwrap();
        let sequence = world.storage_mut().inner.sequence().unwrap();
        world.storage_mut().reject = true;
        assert!(advance(&mut world, &items, &quests, true).is_err());
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
        focus(&mut world, 0).unwrap();
        assert_eq!(
            advance(&mut world, &items, &quests, true)
                .unwrap()
                .unwrap()
                .rewards,
            2
        );
        assert_eq!(world.storage_mut().inner.sequence().unwrap(), sequence + 1);
        assert_eq!(
            advance(&mut world, &items, &quests, true)
                .unwrap()
                .unwrap()
                .rewards,
            0
        );
        let expected = snapshot(&mut world).unwrap();
        assert_eq!(
            State::decode(&expected, &items, &quests)
                .unwrap()
                .audit(&items)
                .unwrap(),
            (12, 1824, 2)
        );
        drop(world);
        let mut store = WorldStore::open(&root, backend).unwrap();
        for (pos, cube) in &expected {
            assert_eq!(store.load(*pos).unwrap().as_ref(), Some(cube));
        }
    }
    #[test]
    fn journal_rejected_final_claim_reopens_unchanged_and_retries_once() {
        filesystem_claim_rollback(Backend::Journal);
    }
    #[cfg(feature = "paged")]
    #[test]
    fn paged_rejected_final_claim_reopens_unchanged_and_retries_once() {
        filesystem_claim_rollback(Backend::Paged);
    }
}
