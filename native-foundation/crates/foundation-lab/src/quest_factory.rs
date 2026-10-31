//! Synthetic item submission/reward ledger attached to the verified two-machine world.
use crate::{
    factory,
    world_storage::{Backend, WorldStore},
};
use industrial_sim::{
    content::{Amount, Catalog as Items, Id},
    inventory::{Inventory, Limits as PortLimits},
    machine::Machine,
};
use quest_core::{
    catalog::{Catalog as Quests, QuestId},
    progress::Book,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};
use voxel_core::{
    coords::CubePos,
    record::{CubeRecord, RecordError},
    storage::CubeStorage,
    streaming::{Limits, Streamer, Tickets},
};
type Error = Box<dyn std::error::Error>;
pub(crate) const CRUSHER: CubePos = CubePos::new(-1, 1_000_000, 0);
pub(crate) const SMELTER: CubePos = CubePos::new(-1, 1_000_001, 0);
pub(crate) const LEDGER: CubePos = CubePos::new(0, 1_000_000, 0);
pub(crate) const PLAYER: CubePos = CubePos::new(0, 1_000_001, 0);
pub(crate) const PARTICIPANTS: [CubePos; 4] = [CRUSHER, SMELTER, LEDGER, PLAYER];
fn id(name: &str) -> Id {
    Id::new(format!("lab:{name}")).unwrap()
}
fn qid(n: u128) -> QuestId {
    QuestId::from_u128(n)
}
fn ports() -> PortLimits {
    PortLimits {
        items: 16,
        fluid_mb: 0,
        kinds: 2,
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    schema_version: u32,
    book_json: String,
    submitted: Inventory,
}
pub(crate) fn encode_ledger(
    book: &Book,
    submitted: Inventory,
    quests: &Quests,
) -> Result<Vec<u8>, Error> {
    let ledger = Ledger {
        schema_version: 1,
        book_json: String::from_utf8(book.encode(quests)?)?,
        submitted,
    };
    Ok(serde_json::to_vec(&ledger)?)
}
pub(crate) fn decode_ledger(
    bytes: &[u8],
    items: &Items,
    quests: &Quests,
) -> Result<(Book, Inventory), Error> {
    if bytes.len() > 32768 {
        return Err("lab ledger exceeds cube payload budget".into());
    }
    let ledger: Ledger = serde_json::from_slice(bytes)?;
    if ledger.schema_version != 1 {
        return Err("unknown quest ledger schema".into());
    }
    ledger.submitted.validate(items)?;
    Ok((
        Book::decode(ledger.book_json.as_bytes(), quests)?,
        ledger.submitted,
    ))
}
fn invalid(_: impl std::fmt::Display) -> RecordError {
    RecordError::Invalid("quest factory state or reward pressure")
}

pub(crate) fn submit(
    records: &mut BTreeMap<CubePos, CubeRecord>,
    items: &Items,
    quests: &Quests,
) -> Result<u64, RecordError> {
    let crusher = Machine::decode(
        records
            .get(&CRUSHER)
            .ok_or(RecordError::Invalid("missing quest source"))?
            .payload(),
        items,
    )
    .map_err(invalid)?;
    let mut smelter = Machine::decode(
        records
            .get(&SMELTER)
            .ok_or(RecordError::Invalid("missing quest source"))?
            .payload(),
        items,
    )
    .map_err(invalid)?;
    let (mut book, mut submitted) = decode_ledger(
        records
            .get(&LEDGER)
            .ok_or(RecordError::Invalid("missing quest ledger"))?
            .payload(),
        items,
        quests,
    )
    .map_err(invalid)?;
    let mut player: Inventory = serde_json::from_slice(
        records
            .get(&PLAYER)
            .ok_or(RecordError::Invalid("missing player reward port"))?
            .payload(),
    )
    .map_err(invalid)?;
    player.validate(items).map_err(invalid)?;
    if !book.is_complete(&qid(1)) {
        book.record_total(quests, &qid(1), 0, crusher.completed())
            .map_err(invalid)?;
        if book.task_progress(&qid(1), 0) >= 1 {
            book.complete(quests, &qid(1)).map_err(invalid)?;
        }
    }
    if !book.is_complete(&qid(2)) && book.unlocked(quests, &qid(2)).map_err(invalid)? {
        smelter
            .outputs_mut()
            .transfer(&mut submitted, items, &id("iron_ingot"), 1)
            .map_err(invalid)?;
        book.record_total(quests, &qid(2), 0, submitted.amount(&id("iron_ingot")))
            .map_err(invalid)?;
        if book.task_progress(&qid(2), 0) >= 12 {
            book.complete(quests, &qid(2)).map_err(invalid)?;
        }
    }
    let mut granted = 0;
    if book.is_complete(&qid(2)) && book.claim(quests, &qid(2)).map_err(invalid)? {
        player
            .apply(
                items,
                &[],
                &[Amount {
                    resource: id("reward_token"),
                    quantity: 2,
                }],
            )
            .map_err(invalid)?;
        granted = 2;
    }
    records
        .get_mut(&SMELTER)
        .unwrap()
        .replace_payload(smelter.encode(items).map_err(invalid)?)?;
    records
        .get_mut(&LEDGER)
        .unwrap()
        .replace_payload(encode_ledger(&book, submitted, quests).map_err(invalid)?)?;
    records
        .get_mut(&PLAYER)
        .unwrap()
        .replace_payload(serde_json::to_vec(&player).map_err(invalid)?)?;
    Ok(granted)
}

pub(crate) fn initial_records(quests: &Quests) -> Result<Vec<CubeRecord>, Error> {
    let mut ledger = factory::generated(LEDGER);
    ledger.set(256 + 8 * 16 + 8, 7)?;
    ledger.replace_payload(encode_ledger(
        &Book::new(quests),
        Inventory::new(ports())?,
        quests,
    )?)?;
    let mut player = factory::generated(PLAYER);
    player.set(256 + 8 * 16 + 8, 8)?;
    player.replace_payload(serde_json::to_vec(&Inventory::new(PortLimits {
        items: 4,
        fluid_mb: 0,
        kinds: 1,
    })?)?)?;
    Ok(vec![ledger, player])
}

fn load<S: CubeStorage>(store: S) -> Result<Streamer<S>, Error> {
    let mut stream = Streamer::new(
        store,
        Limits {
            max_cubes: 27,
            max_resident_bytes: 27 * 32768,
            max_cube_bytes: 32768,
            max_pending_jobs: 4,
        },
    )?;
    stream.set_tickets(Tickets::around(CRUSHER, [1; 3], [1; 3])?);
    for _ in 0..32 {
        let plan = stream.plan(4, 27)?;
        if plan.loads.is_empty() {
            break;
        }
        for job in plan.loads {
            let cube = stream.read_or_generate(job, factory::generated)?;
            assert!(stream.complete(job, cube)?);
        }
    }
    for pos in PARTICIPANTS {
        assert!(stream.simulates(pos));
    }
    Ok(stream)
}
fn unload<S: CubeStorage>(stream: &mut Streamer<S>) -> Result<(), Error> {
    for job in stream.set_tickets(Tickets::default()) {
        assert!(stream.acknowledge_canceled(job));
    }
    stream.plan(0, 27)?;
    assert_eq!(stream.resident_positions().count(), 0);
    Ok(())
}
fn player_edit(stream: &mut Streamer<WorldStore>, items: &Items, add: bool) -> Result<(), Error> {
    stream.edit(PLAYER, |cube| {
        let mut inv: Inventory = serde_json::from_slice(cube.payload()).map_err(invalid)?;
        let tokens = [Amount {
            resource: id("reward_token"),
            quantity: 4,
        }];
        inv.apply(
            items,
            if add { &[] } else { &tokens },
            if add { &tokens } else { &[] },
        )
        .map_err(invalid)?;
        cube.replace_payload(serde_json::to_vec(&inv).map_err(invalid)?)?;
        Ok(())
    })?;
    stream.flush_all()?;
    Ok(())
}
fn ledger_state(
    stream: &Streamer<WorldStore>,
    items: &Items,
    quests: &Quests,
) -> Result<(Book, Inventory, Inventory), Error> {
    let (book, submitted) = decode_ledger(
        stream.record(LEDGER).ok_or("ledger unavailable")?.payload(),
        items,
        quests,
    )?;
    let player: Inventory =
        serde_json::from_slice(stream.record(PLAYER).ok_or("player unavailable")?.payload())?;
    player.validate(items)?;
    Ok((book, submitted, player))
}

pub fn run(backend: Backend) -> Result<Value, Error> {
    let factory = factory::run_with_backend(128, backend, |_, _, _| Ok(()))?;
    let root = PathBuf::from(
        factory.report["world_directory"]
            .as_str()
            .ok_or("factory receipt missing world")?,
    );
    let items = Items::from_json(include_bytes!("../../../fixtures/industrial-v1.json"))?;
    let quests = Quests::from_json(include_bytes!("../../../fixtures/quests-v1.json"))?;
    let mut store = WorldStore::open(&root, backend)?;
    let initial = initial_records(&quests)?;
    store.save_batch(&initial.iter().collect::<Vec<_>>())?;
    let mut stream = load(store)?;
    let (mut grants, mut rollback_checks, mut assertions, mut mid_progress) =
        (0u64, 0u64, 0u64, 0u64);
    for step in 0..12 {
        if step == 11 {
            // Four explicitly inserted fixture tokens fill the wallet; remove them after the failed attempt.
            player_edit(&mut stream, &items, true)?;
            let before: Vec<_> = PARTICIPANTS
                .iter()
                .map(|p| stream.record(*p).unwrap().clone())
                .collect();
            assert!(
                stream
                    .transact(&PARTICIPANTS, |records| submit(records, &items, &quests))
                    .is_err()
            );
            for (pos, expected) in PARTICIPANTS.iter().zip(before) {
                assert_eq!(stream.record(*pos), Some(&expected));
                assertions += 1;
            }
            let (book, submitted, player) = ledger_state(&stream, &items, &quests)?;
            assert_eq!(submitted.amount(&id("iron_ingot")), 11);
            assert!(!book.is_complete(&qid(2)));
            assert!(!book.has_claimed(&qid(2)));
            assert_eq!(player.amount(&id("reward_token")), 4);
            player_edit(&mut stream, &items, false)?;
            rollback_checks += 1;
        }
        let granted = stream.transact(&PARTICIPANTS, |records| submit(records, &items, &quests))?;
        grants += u64::from(granted > 0);
        let (book, submitted, player) = ledger_state(&stream, &items, &quests)?;
        let machine = Machine::decode(stream.record(SMELTER).unwrap().payload(), &items)?;
        assert_eq!(submitted.amount(&id("iron_ingot")), step + 1);
        assert_eq!(
            submitted.amount(&id("iron_ingot")) + machine.outputs().amount(&id("iron_ingot")),
            12
        );
        assert_eq!(book.task_progress(&qid(2), 0), step + 1);
        assert_eq!(player.amount(&id("reward_token")), grants * 2);
        assertions += 4;
        if step == 5 {
            unload(&mut stream)?;
            drop(stream);
            stream = load(WorldStore::open(&root, backend)?)?;
            let (book, submitted, _) = ledger_state(&stream, &items, &quests)?;
            mid_progress = book.task_progress(&qid(2), 0);
            assert_eq!(mid_progress, 6);
            assert_eq!(submitted.amount(&id("iron_ingot")), 6);
            assertions += 2;
        }
    }
    let sequence = stream.storage_mut().sequence()?;
    for _ in 0..20 {
        assert_eq!(
            stream.transact(&PARTICIPANTS, |records| submit(records, &items, &quests))?,
            0
        );
        assertions += 1;
    }
    assert_eq!(
        stream.storage_mut().sequence()?,
        sequence,
        "repeated claim should not write changed state"
    );
    let (book, submitted, player) = ledger_state(&stream, &items, &quests)?;
    assert!(book.is_complete(&qid(1)) && book.is_complete(&qid(2)) && book.has_claimed(&qid(2)));
    assert_eq!(player.amount(&id("reward_token")), 2);
    assert_eq!(submitted.amount(&id("iron_ingot")), 12);
    assert_eq!(grants, 1);
    let expected: Vec<_> = PARTICIPANTS
        .iter()
        .map(|p| stream.record(*p).unwrap().clone())
        .collect();
    unload(&mut stream)?;
    drop(stream);
    let mut reopened = WorldStore::open(&root, backend)?;
    for cube in &expected {
        assert_eq!(reopened.load(cube.pos())?, Some(cube.clone()));
        assertions += 1;
    }
    Ok(
        json!({"status":"PASS","storage_backend":backend.name(),"fixture":"synthetic quests/rewards; original counters, not GTNH task parity","factory":factory.report,"submitted_ingots":12,"remaining_machine_ingots":0,"reward_tokens":2,"reward_grants":grants,"mid_submission_reopen_progress":mid_progress,"full_reward_rollback_checks":rollback_checks,"final_reopen_checks":4,"duplicate_claim_attempts":20,"assertions":assertions,"world_directory":root,"scope":"item submission, quest progress and reward inventory share one four-cube transaction; no visible quest UI or multiplayer/repeat semantics"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_core::storage::StoreError;
    #[test]
    fn rewards_and_submitted_items_survive_restart_and_failed_reward_delivery() {
        let report = run(Backend::Journal).unwrap();
        assert_eq!(report["status"], "PASS");
        assert_eq!(report["submitted_ingots"], 12);
        assert_eq!(report["reward_tokens"], 2);
        assert_eq!(report["reward_grants"], 1);
        assert_eq!(report["mid_submission_reopen_progress"], 6);
        assert_eq!(report["full_reward_rollback_checks"], 1);
        assert_eq!(report["final_reopen_checks"], 4);
    }
    #[cfg(feature = "paged")]
    #[test]
    fn paged_reward_flow_has_the_same_conservation_result() {
        let report = run(Backend::Paged).unwrap();
        assert_eq!(report["status"], "PASS");
        assert_eq!(report["submitted_ingots"], 12);
        assert_eq!(report["reward_tokens"], 2);
        assert_eq!(report["reward_grants"], 1);
    }

    #[test]
    fn failed_storage_commit_keeps_the_claim_and_reward_inventory_unchanged() {
        let items =
            Items::from_json(include_bytes!("../../../fixtures/industrial-v1.json")).unwrap();
        let quests = Quests::from_json(include_bytes!("../../../fixtures/quests-v1.json")).unwrap();
        let mut book = Book::new(&quests);
        book.record_total(&quests, &qid(1), 0, 1).unwrap();
        book.complete(&quests, &qid(1)).unwrap();
        book.record_total(&quests, &qid(2), 0, 12).unwrap();
        book.complete(&quests, &qid(2)).unwrap();
        let mut submitted = Inventory::new(ports()).unwrap();
        submitted
            .apply(
                &items,
                &[],
                &[Amount {
                    resource: id("iron_ingot"),
                    quantity: 12,
                }],
            )
            .unwrap();
        let machine = |kind: &str| {
            Machine::new(
                id(kind),
                ports(),
                ports(),
                industrial_sim::machine::Energy::new(64, 32, 1).unwrap(),
            )
            .unwrap()
        };
        let payloads = [
            machine("crusher").encode(&items).unwrap(),
            machine("smelter").encode(&items).unwrap(),
            encode_ledger(&book, submitted, &quests).unwrap(),
            serde_json::to_vec(
                &Inventory::new(PortLimits {
                    items: 4,
                    fluid_mb: 0,
                    kinds: 1,
                })
                .unwrap(),
            )
            .unwrap(),
        ];
        let records: Vec<_> = PARTICIPANTS
            .into_iter()
            .zip(payloads)
            .map(|(pos, bytes)| {
                let mut cube = CubeRecord::new(pos).unwrap();
                cube.replace_payload(bytes).unwrap();
                cube
            })
            .collect();
        let mut memory = voxel_core::storage::MemoryStorage::default();
        memory
            .save_batch(&records.iter().collect::<Vec<_>>())
            .unwrap();
        let mut stream = load(memory).unwrap();
        stream.storage_mut().reject_writes = true;
        let result = stream.transact(&PARTICIPANTS, |records| submit(records, &items, &quests));
        assert!(matches!(
            result,
            Err(voxel_core::streaming::StreamError::Store(StoreError::Io(_)))
        ));
        assert_eq!(stream.stats().transaction_failures, 1);
        for record in &records {
            assert_eq!(stream.record(record.pos()), Some(record));
        }
        stream.storage_mut().reject_writes = false;
        assert_eq!(
            stream
                .transact(&PARTICIPANTS, |records| submit(records, &items, &quests))
                .unwrap(),
            2
        );
        let wallet = stream.storage_mut().load(PLAYER).unwrap().unwrap();
        let wallet: Inventory = serde_json::from_slice(wallet.payload()).unwrap();
        assert_eq!(wallet.amount(&id("reward_token")), 2);
        let ledger = stream.storage_mut().load(LEDGER).unwrap().unwrap();
        assert!(
            decode_ledger(ledger.payload(), &items, &quests)
                .unwrap()
                .0
                .has_claimed(&qid(2))
        );
        assert_eq!(
            stream
                .transact(&PARTICIPANTS, |records| submit(records, &items, &quests))
                .unwrap(),
            0
        );
    }
}
