//! Inventory/count atomicity fixture. Native core is original/MIT; pack input contains only facts.
use crate::{
    factory,
    world_storage::{Backend, WorldStore},
};
use quest_core::{
    item_match::{DEFAULT_BUDGET, Stack},
    retrieval::{Definition, Progress, Slot, validate_slots},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use voxel_core::{
    coords::CubePos,
    record::{CubeRecord, RecordError},
    storage::CubeStorage,
    streaming::{Limits, Streamer, Tickets},
};
type Error = Box<dyn std::error::Error>;
const PLAYER: CubePos = CubePos::new(-1, 1_000_000, 0);
const CHEST: CubePos = CubePos::new(0, 1_000_000, 0);
const CONSUME: CubePos = CubePos::new(-1, 1_000_001, 0);
const DETECT: CubePos = CubePos::new(-1, 999_999, 0);
const PARTICIPANTS: [CubePos; 4] = [PLAYER, CHEST, CONSUME, DETECT];
const ARCHIVE: &str = "0def2484e84f0b8f16e765d5f79e3f54fc61943c78d758d506cd3e5195c29321";
#[derive(Clone)]
struct Fixture {
    consume: Definition,
    detect: Definition,
}
impl Fixture {
    fn validate(&self) -> Result<(), Error> {
        for def in [&self.consume, &self.detect] {
            def.fingerprint()?;
            if def.requirements.len() != 1
                || def.requirements[0].stack != cobblestone()
                || def.requirements[0].damageable
            {
                return Err(
                    "fixture requires a single exact non-damageable cobblestone requirement".into(),
                );
            }
        }
        let (c, d) = (
            self.consume.requirements[0].quantity,
            self.detect.requirements[0].quantity,
        );
        if !self.consume.consume
            || self.detect.consume
            || !(2..=64).contains(&d)
            || c <= d
            || c + d > 512
        {
            return Err("fixture count/mode bounds exceeded".into());
        }
        Ok(())
    }
    fn total(&self) -> u32 {
        self.consume.requirements[0].quantity + self.detect.requirements[0].quantity
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    schema_version: u32,
    archive_sha256: String,
    consume_task: String,
    detect_task: String,
    consume_definition: Definition,
    detect_definition: Definition,
}
fn profile(path: &Path) -> Result<(Fixture, String, String, String), Error> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(65537)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err("retrieval profile too large".into());
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let p: Profile = serde_json::from_slice(&bytes)?;
    if p.schema_version != 1 || p.archive_sha256 != ARCHIVE {
        return Err("retrieval profile source is not the pinned evaluation archive".into());
    }
    for (task, definition) in [
        (&p.consume_task, &p.consume_definition),
        (&p.detect_task, &p.detect_definition),
    ] {
        if definition.key != format!("gtnh:2.9.0/{ARCHIVE}/{task}") {
            return Err("retrieval task/source binding mismatch".into());
        }
    }
    let fixture = Fixture {
        consume: p.consume_definition,
        detect: p.detect_definition,
    };
    fixture.validate()?;
    Ok((fixture, p.consume_task, p.detect_task, digest))
}
fn cobblestone() -> Stack {
    Stack {
        registry: "minecraft:cobblestone".into(),
        damage: 0,
        nbt: None,
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Inventory {
    schema_version: u32,
    slots: Vec<Option<Slot>>,
}
impl Inventory {
    fn validate(&self) -> Result<(), RecordError> {
        if self.schema_version != 1 || self.slots.len() != 8 {
            return Err(RecordError::Invalid("retrieval inventory shape"));
        }
        validate_slots(&self.slots).map_err(|_| RecordError::Invalid("retrieval slot state"))?;
        if self
            .slots
            .iter()
            .flatten()
            .any(|s| s.quantity > 64 || s.stack != cobblestone())
        {
            return Err(RecordError::Invalid("fixture cobblestone slot/capacity"));
        }
        Ok(())
    }
    fn total(&self) -> u32 {
        self.slots.iter().flatten().map(|s| s.quantity).sum()
    }
    fn add(&mut self, quantity: u32) -> Result<(), RecordError> {
        self.validate()?;
        if quantity > 512 - self.total() {
            return Err(RecordError::Invalid("fixture inventory full"));
        }
        let mut left = quantity;
        for slot in self.slots.iter_mut().flatten() {
            let moved = (64 - slot.quantity).min(left);
            slot.quantity += moved;
            left -= moved;
        }
        for slot in &mut self.slots {
            if left == 0 {
                break;
            }
            if slot.is_none() {
                let moved = left.min(64);
                *slot = Some(Slot {
                    stack: cobblestone(),
                    quantity: moved,
                });
                left -= moved;
            }
        }
        assert_eq!(left, 0);
        Ok(())
    }
    fn take(&mut self, quantity: u32) -> Result<(), RecordError> {
        self.validate()?;
        if quantity > self.total() {
            return Err(RecordError::Invalid("fixture insufficient items"));
        }
        let mut left = quantity;
        for slot in &mut self.slots {
            if let Some(value) = slot {
                let moved = value.quantity.min(left);
                value.quantity -= moved;
                left -= moved;
                if value.quantity == 0 {
                    *slot = None;
                }
            }
        }
        assert_eq!(left, 0);
        Ok(())
    }
    fn new(quantity: u32) -> Result<Self, RecordError> {
        let mut inventory = Self {
            schema_version: 1,
            slots: vec![None; 8],
        };
        inventory.add(quantity)?;
        Ok(inventory)
    }
    fn encode(&self) -> Result<Vec<u8>, RecordError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|_| RecordError::Invalid("retrieval inventory encode"))
    }
    fn decode(record: &CubeRecord) -> Result<Self, RecordError> {
        if record.payload().len() > 32768 {
            return Err(RecordError::Invalid("retrieval inventory decode bounds"));
        }
        let value: Self = serde_json::from_slice(record.payload())
            .map_err(|_| RecordError::Invalid("retrieval inventory encoding"))?;
        value.validate()?;
        Ok(value)
    }
}
fn initial(fixture: &Fixture) -> Result<Vec<CubeRecord>, Error> {
    fixture.validate()?;
    let half = fixture.detect.requirements[0].quantity / 2;
    let payloads = [
        Inventory::new(half)?.encode()?,
        Inventory::new(fixture.total() - half)?.encode()?,
        Progress::new(&fixture.consume)?.encode(&fixture.consume)?,
        Progress::new(&fixture.detect)?.encode(&fixture.detect)?,
    ];
    PARTICIPANTS
        .into_iter()
        .zip(payloads)
        .map(|(pos, payload)| {
            let mut cube = factory::generated(pos);
            cube.replace_payload(payload)?;
            Ok(cube)
        })
        .collect()
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
    simulate: [u16; 3],
) -> Result<(), Error> {
    for job in world.set_tickets(Tickets::around(center, [1; 3], simulate)?) {
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
#[derive(Clone, Copy)]
enum Action {
    Detect,
    Submit,
    PlayerToChest(u32),
    ChestToPlayer(u32),
}
#[derive(Clone, Copy, Debug, Default)]
struct Step {
    consumed: u64,
    moved: u32,
    satisfied: bool,
}
fn operate<S: CubeStorage>(
    world: &mut Streamer<S>,
    fixture: &Fixture,
    action: Action,
) -> Result<Step, Error> {
    fixture.validate()?;
    let participants = match action {
        Action::Detect => [PLAYER, DETECT],
        Action::Submit => [PLAYER, CONSUME],
        _ => [PLAYER, CHEST],
    };
    if participants.iter().any(|p| !world.simulates(*p)) {
        return Err("retrieval participants unavailable for simulation".into());
    }
    Ok(world.transact(&participants, |records| match action {
        Action::Detect | Action::Submit => {
            let mut inventory = Inventory::decode(&records[&PLAYER])?;
            let (pos, definition) = if matches!(action, Action::Submit) {
                (CONSUME, &fixture.consume)
            } else {
                (DETECT, &fixture.detect)
            };
            let mut progress = Progress::decode(records[&pos].payload(), definition)
                .map_err(|_| RecordError::Invalid("retrieval progress binding"))?;
            let report = if matches!(action, Action::Submit) {
                progress.submit(definition, &mut inventory.slots, DEFAULT_BUDGET)
            } else {
                progress.detect(definition, &inventory.slots, DEFAULT_BUDGET)
            }
            .map_err(|_| RecordError::Invalid("retrieval operation"))?;
            let state = progress
                .encode(definition)
                .map_err(|_| RecordError::Invalid("retrieval progress encode"))?;
            records
                .get_mut(&PLAYER)
                .unwrap()
                .replace_payload(inventory.encode()?)?;
            records.get_mut(&pos).unwrap().replace_payload(state)?;
            Ok(Step {
                consumed: report.consumed,
                moved: 0,
                satisfied: report.satisfied,
            })
        }
        Action::PlayerToChest(limit) | Action::ChestToPlayer(limit) => {
            let (from, to) = if matches!(action, Action::PlayerToChest(_)) {
                (PLAYER, CHEST)
            } else {
                (CHEST, PLAYER)
            };
            let mut source = Inventory::decode(&records[&from])?;
            let mut target = Inventory::decode(&records[&to])?;
            let moved = limit.min(source.total()).min(512 - target.total());
            source.take(moved)?;
            target.add(moved)?;
            records
                .get_mut(&from)
                .unwrap()
                .replace_payload(source.encode()?)?;
            records
                .get_mut(&to)
                .unwrap()
                .replace_payload(target.encode()?)?;
            Ok(Step {
                moved,
                ..Step::default()
            })
        }
    })?)
}
fn snapshot<S: CubeStorage>(
    world: &mut Streamer<S>,
) -> Result<BTreeMap<CubePos, CubeRecord>, Error> {
    let mut records = BTreeMap::new();
    for pos in PARTICIPANTS {
        let cube = if let Some(cube) = world.record(pos) {
            cube.clone()
        } else {
            world
                .storage_mut()
                .load(pos)?
                .ok_or("missing retrieval fixture cube")?
        };
        records.insert(pos, cube);
    }
    Ok(records)
}
fn audit(
    records: &BTreeMap<CubePos, CubeRecord>,
    fixture: &Fixture,
) -> Result<(u32, u32, u32), Error> {
    let player = Inventory::decode(&records[&PLAYER])?;
    let chest = Inventory::decode(&records[&CHEST])?;
    let consume = Progress::decode(records[&CONSUME].payload(), &fixture.consume)?;
    let detect = Progress::decode(records[&DETECT].payload(), &fixture.detect)?;
    let consumed = consume.values()[0];
    assert_eq!(player.total() + chest.total() + consumed, fixture.total());
    Ok((player.total() + chest.total(), consumed, detect.values()[0]))
}
fn unload<S: CubeStorage>(world: &mut Streamer<S>) -> Result<(), Error> {
    for job in world.set_tickets(Tickets::default()) {
        assert!(world.acknowledge_canceled(job));
    }
    world.plan(0, 27)?;
    assert_eq!(world.resident_positions().count(), 0);
    Ok(())
}
fn checked_step(
    world: &mut Streamer<WorldStore>,
    fixture: &Fixture,
    action: Action,
    trace: &mut u64,
    checks: &mut u64,
) -> Result<Step, Error> {
    let result = operate(world, fixture, action)?;
    let records = snapshot(world)?;
    audit(&records, fixture)?;
    *checks += 1;
    assert!(world.resident_positions().count() <= 27 && world.charged_bytes() <= 27 * 32768);
    for cube in records.values() {
        for byte in cube.encode() {
            *trace = (*trace ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    Ok(result)
}
pub fn run(path: &Path, backend: Backend) -> Result<Value, Error> {
    let (fixture, consume_task, detect_task, profile_sha256) = profile(path)?;
    let (c, d) = (
        fixture.consume.requirements[0].quantity,
        fixture.detect.requirements[0].quantity,
    );
    let half = d / 2;
    let root = factory::world_root(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos())?;
    let mut store = WorldStore::open(&root, backend)?;
    let records = initial(&fixture)?;
    store.save_batch(&records.iter().collect::<Vec<_>>())?;
    let mut world = stream(store)?;
    focus(&mut world, PLAYER, [1; 3])?;
    let (mut trace, mut ledger_checks) = (0xcbf29ce484222325u64, 0u64);
    checked_step(
        &mut world,
        &fixture,
        Action::Detect,
        &mut trace,
        &mut ledger_checks,
    )?;
    let before = snapshot(&mut world)?;
    assert_eq!(audit(&before, &fixture)?, (fixture.total(), 0, half));
    unload(&mut world)?;
    drop(world);
    world = stream(WorldStore::open(&root, backend)?)?;
    assert_eq!(snapshot(&mut world)?, before);
    focus(&mut world, PLAYER, [1; 3])?;
    assert_eq!(
        checked_step(
            &mut world,
            &fixture,
            Action::PlayerToChest(u32::MAX),
            &mut trace,
            &mut ledger_checks
        )?
        .moved,
        half
    );
    checked_step(
        &mut world,
        &fixture,
        Action::Detect,
        &mut trace,
        &mut ledger_checks,
    )?;
    assert_eq!(audit(&snapshot(&mut world)?, &fixture)?.2, 0);
    assert_eq!(
        checked_step(
            &mut world,
            &fixture,
            Action::ChestToPlayer(d),
            &mut trace,
            &mut ledger_checks
        )?
        .moved,
        d
    );
    assert!(
        checked_step(
            &mut world,
            &fixture,
            Action::Detect,
            &mut trace,
            &mut ledger_checks
        )?
        .satisfied
    );
    let mut availability_rejections = 0;
    for center in [PLAYER, CubePos::new(-1, -1_000_000, 0)] {
        focus(&mut world, center, [0; 3])?;
        let before = snapshot(&mut world)?;
        for action in [Action::Detect, Action::Submit] {
            assert!(operate(&mut world, &fixture, action).is_err());
            assert_eq!(snapshot(&mut world)?, before);
            availability_rejections += 1;
        }
    }
    focus(&mut world, PLAYER, [1; 3])?;
    let (mut donated, mut donations, mut mid_donation, mut latched_checks) =
        (0u64, 0u32, 0u32, 0u32);
    while donated < u64::from(c) {
        if donations >= 16 {
            return Err("bounded retrieval donation route failed to finish".into());
        }
        if Inventory::decode(world.record(PLAYER).ok_or("player cube unavailable")?)?.total() == 0 {
            assert!(
                checked_step(
                    &mut world,
                    &fixture,
                    Action::ChestToPlayer(64),
                    &mut trace,
                    &mut ledger_checks
                )?
                .moved
                    > 0
            );
        }
        let result = checked_step(
            &mut world,
            &fixture,
            Action::Submit,
            &mut trace,
            &mut ledger_checks,
        )?;
        assert!(result.consumed > 0);
        donated += result.consumed;
        donations += 1;
        assert!(
            checked_step(
                &mut world,
                &fixture,
                Action::Detect,
                &mut trace,
                &mut ledger_checks
            )?
            .satisfied
        );
        latched_checks += 1;
        if donations == 1 {
            let before = snapshot(&mut world)?;
            mid_donation = audit(&before, &fixture)?.1;
            assert_eq!(mid_donation, d);
            unload(&mut world)?;
            drop(world);
            world = stream(WorldStore::open(&root, backend)?)?;
            assert_eq!(snapshot(&mut world)?, before);
            focus(&mut world, PLAYER, [1; 3])?;
        }
    }
    let sequence = world.storage_mut().sequence()?;
    for _ in 0..20 {
        let result = checked_step(
            &mut world,
            &fixture,
            Action::Submit,
            &mut trace,
            &mut ledger_checks,
        )?;
        assert_eq!(result.consumed, 0);
        assert!(result.satisfied);
    }
    assert_eq!(
        world.storage_mut().sequence()?,
        sequence,
        "repeated satisfied donation wrote changed state"
    );
    checked_step(
        &mut world,
        &fixture,
        Action::ChestToPlayer(u32::MAX),
        &mut trace,
        &mut ledger_checks,
    )?;
    let final_state = snapshot(&mut world)?;
    assert_eq!(audit(&final_state, &fixture)?, (d, c, d));
    assert_eq!(Inventory::decode(&final_state[&PLAYER])?.total(), d);
    assert_eq!(Inventory::decode(&final_state[&CHEST])?.total(), 0);
    assert!(
        Progress::decode(final_state[&CONSUME].payload(), &fixture.consume)?
            .satisfied(&fixture.consume)?
    );
    let stats = world.stats().clone();
    unload(&mut world)?;
    drop(world);
    let mut reopened = WorldStore::open(&root, backend)?;
    for (pos, cube) in &final_state {
        assert_eq!(reopened.load(*pos)?.as_ref(), Some(cube));
    }
    Ok(
        json!({"status":"PASS","backend":backend.name(),"world_directory":root,"profile_sha256":profile_sha256,"source_archive_sha256":ARCHIVE,
        "source_consume_task":consume_task,"source_detect_task":detect_task,"initial_cobblestone":fixture.total(),"donated_cobblestone":donated,"remaining_cobblestone":d,
        "detected_requirement":d,"donation_requirement":c,"mid_detection_reopen_count":half,"mid_donation_reopen_count":mid_donation,"availability_rejections":availability_rejections,
        "donation_commands":donations,"latched_empty_inventory_checks":latched_checks,"repeated_submission_checks":20,"ledger_checks":ledger_checks,"final_reopen_records":4,"trace_hash":format!("{trace:016x}"),
        "streaming_since_mid_donation_reopen":{"loaded":stats.loaded,"peak_cubes":stats.peak_cubes,"peak_bytes":stats.peak_bytes,"transactions":stats.transactions},
        "scope":"Private factual projection of two provisional GTNH 2.9.0 cobblestone task predicates. Real inventory/progress atomic commits and reopen; no quest unlock/completion/reward/party/full-registry parity. Other fixture geometry and container layout are synthetic."}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use quest_core::retrieval::Requirement;
    use voxel_core::storage::{MemoryStorage, StoreError};
    fn fixture() -> Fixture {
        let def = |key: &str, consume, quantity| Definition {
            key: key.into(),
            requirements: vec![Requirement {
                stack: cobblestone(),
                quantity,
                damageable: false,
                ore_alias: String::new(),
            }],
            consume,
            ignore_nbt: false,
            partial: true,
            only_one: false,
            group_detect: false,
            auto_consume: false,
        };
        Fixture {
            consume: def("synthetic:consume", true, 8),
            detect: def("synthetic:detect", false, 4),
        }
    }
    fn seeded() -> (Fixture, Streamer<MemoryStorage>) {
        let f = fixture();
        let mut store = MemoryStorage::default();
        let records = initial(&f).unwrap();
        store
            .save_batch(&records.iter().collect::<Vec<_>>())
            .unwrap();
        let mut world = stream(store).unwrap();
        focus(&mut world, PLAYER, [1; 3]).unwrap();
        (f, world)
    }
    #[test]
    fn rejected_storage_write_keeps_inventory_and_requirement_progress_unchanged() {
        let (f, mut world) = seeded();
        let before = snapshot(&mut world).unwrap();
        world.storage_mut().reject_writes = true;
        assert!(operate(&mut world, &f, Action::Submit).is_err());
        assert_eq!(snapshot(&mut world).unwrap(), before);
        for (pos, cube) in &before {
            assert_eq!(world.storage_mut().load(*pos).unwrap().as_ref(), Some(cube));
        }
        world.storage_mut().reject_writes = false;
        assert_eq!(operate(&mut world, &f, Action::Submit).unwrap().consumed, 2);
        assert_eq!(
            audit(&snapshot(&mut world).unwrap(), &f).unwrap(),
            (10, 2, 0)
        );
    }
    #[test]
    fn render_only_or_unloaded_progress_cannot_detect_or_consume_items() {
        let (f, mut world) = seeded();
        for center in [PLAYER, CubePos::new(-1, -1_000_000, 0)] {
            focus(&mut world, center, [0; 3]).unwrap();
            let before = snapshot(&mut world).unwrap();
            for action in [Action::Detect, Action::Submit] {
                assert!(operate(&mut world, &f, action).is_err());
                assert_eq!(snapshot(&mut world).unwrap(), before);
            }
        }
        focus(&mut world, PLAYER, [1; 3]).unwrap();
        operate(&mut world, &f, Action::Detect).unwrap();
        assert_eq!(
            audit(&snapshot(&mut world).unwrap(), &f).unwrap(),
            (12, 0, 2)
        );
    }
    struct Reject {
        inner: WorldStore,
        fail: bool,
    }
    impl CubeStorage for Reject {
        fn load(&mut self, pos: CubePos) -> Result<Option<CubeRecord>, StoreError> {
            self.inner.load(pos)
        }
        fn save(&mut self, record: &CubeRecord) -> Result<(), StoreError> {
            self.save_batch(&[record])
        }
        fn save_batch(&mut self, records: &[&CubeRecord]) -> Result<(), StoreError> {
            if self.fail {
                Err(StoreError::Io(std::io::Error::other(
                    "injected retrieval write rejection",
                )))
            } else {
                self.inner.save_batch(records)
            }
        }
    }
    fn filesystem_failure(backend: Backend) {
        let f = fixture();
        let root = factory::world_root(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        )
        .unwrap();
        let mut store = WorldStore::open(&root, backend).unwrap();
        let records = initial(&f).unwrap();
        store
            .save_batch(&records.iter().collect::<Vec<_>>())
            .unwrap();
        let sequence = store.sequence().unwrap();
        let mut world = stream(Reject {
            inner: store,
            fail: true,
        })
        .unwrap();
        focus(&mut world, PLAYER, [1; 3]).unwrap();
        let before = snapshot(&mut world).unwrap();
        assert!(operate(&mut world, &f, Action::Submit).is_err());
        assert_eq!(snapshot(&mut world).unwrap(), before);
        assert_eq!(world.storage_mut().inner.sequence().unwrap(), sequence);
        drop(world);
        let mut store = WorldStore::open(&root, backend).unwrap();
        for (pos, cube) in &before {
            assert_eq!(store.load(*pos).unwrap().as_ref(), Some(cube));
        }
        let mut world = stream(store).unwrap();
        focus(&mut world, PLAYER, [1; 3]).unwrap();
        assert_eq!(operate(&mut world, &f, Action::Submit).unwrap().consumed, 2);
        let after = snapshot(&mut world).unwrap();
        drop(world);
        let mut reopened = WorldStore::open(&root, backend).unwrap();
        for (pos, cube) in after {
            assert_eq!(reopened.load(pos).unwrap(), Some(cube));
        }
    }
    #[test]
    fn journal_rejection_and_retry_reopen_the_same_inventory_progress_ledger() {
        filesystem_failure(Backend::Journal);
    }
    #[cfg(feature = "paged")]
    #[test]
    fn paged_rejection_and_retry_reopen_the_same_inventory_progress_ledger() {
        filesystem_failure(Backend::Paged);
    }
}
