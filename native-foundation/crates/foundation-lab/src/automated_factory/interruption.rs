//! Marked subprocess fixtures only. This is not a manual-play resume interface.
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Write},
    path::Path,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    version: u32,
    backend: String,
    phase: String,
    before_sha256: String,
    after_sha256: String,
}
fn digest(records: &BTreeMap<CubePos, CubeRecord>) -> String {
    let mut hash = Sha256::new();
    for record in records.values() {
        hash.update(record.encode());
    }
    format!("{:x}", hash.finalize())
}
fn read_records(store: &mut WorldStore) -> Result<BTreeMap<CubePos, CubeRecord>, Error> {
    POSITIONS
        .into_iter()
        .map(|pos| Ok((pos, store.load(pos)?.ok_or("missing interruption record")?)))
        .collect()
}
fn init(
    root: &Path,
    backend: Backend,
    phase: &str,
    items: &Catalog,
    quests: &Quests,
) -> Result<(), Error> {
    if root.exists() {
        return Err("interruption fixture must be new".into());
    }
    let parent = root
        .parent()
        .ok_or("missing fixture parent")?
        .canonicalize()?;
    if !parent.starts_with(factory::worlds_base()?) {
        return Err("fixture outside private world root".into());
    }
    let mut records: BTreeMap<_, _> = initial(items, quests)?
        .into_iter()
        .map(|r| (r.pos(), r))
        .collect();
    if phase == "working" {
        for _ in 0..17 {
            tick(&mut records, items, quests, false)?;
        }
        assert!(
            State::decode(&records, items, quests)?
                .machines
                .iter()
                .any(|m| m.progress().is_some())
        );
    } else {
        let mut ready = false;
        for _ in 0..400 {
            let state = State::decode(&records, items, quests)?;
            let n = state.submitted.amount(&id("iron_ingot"));
            if n == 11 && state.hopper.amount(&id("iron_ingot")) == 1 {
                ready = true;
                break;
            }
            tick(&mut records, items, quests, n < 11)?;
        }
        assert!(ready, "final-claim fixture failed to reach its bound");
    }
    State::decode(&records, items, quests)?.audit(items)?;
    let before_sha256 = digest(&records);
    let mut next = records.clone();
    tick(&mut next, items, quests, true)?;
    let after_sha256 = digest(&next);
    assert_ne!(before_sha256, after_sha256);
    let mut store = WorldStore::open(root, backend)?;
    store.save_batch(&records.values().collect::<Vec<_>>())?;
    let marker = Marker {
        version: 1,
        backend: backend.name().into(),
        phase: phase.into(),
        before_sha256,
        after_sha256,
    };
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(".automation-interruption"))?;
    output.write_all(&serde_json::to_vec(&marker)?)?;
    output.sync_all()?;
    println!("{}", serde_json::to_string(&marker)?);
    Ok(())
}
pub fn execute(args: &[String]) -> Result<(), Error> {
    if args.len() < 2 {
        return Err("automation-probe MODE PRIVATE_FIXTURE_DIR [RECEIPT]".into());
    }
    let root = Path::new(&args[1]);
    #[cfg(feature = "paged")]
    let (mode, backend) = args[0]
        .strip_prefix("paged-")
        .map(|m| (m, Backend::Paged))
        .unwrap_or((&args[0], Backend::Journal));
    #[cfg(not(feature = "paged"))]
    let (mode, backend) = (args[0].as_str(), Backend::Journal);
    let items = factory::catalog()?;
    let quests = quests()?;
    if let Some(phase) = mode.strip_prefix("init-") {
        if !matches!(phase, "working" | "claim") {
            return Err("invalid initialization phase".into());
        }
        return init(root, backend, phase, &items, &quests);
    }
    if !matches!(mode, "inspect" | "prepare-exit" | "commit-exit" | "loop") {
        return Err("invalid automation interruption mode".into());
    }
    if !root.canonicalize()?.starts_with(factory::worlds_base()?) {
        return Err("fixture outside private world root".into());
    }
    let marker_path = root.join(".automation-interruption");
    if fs::metadata(&marker_path)?.len() > 16384 {
        return Err("interruption marker exceeds bound".into());
    }
    let marker: Marker = serde_json::from_slice(&fs::read(&marker_path)?)?;
    if marker.version != 1
        || marker.backend != backend.name()
        || !matches!(marker.phase.as_str(), "working" | "claim")
    {
        return Err("not an owned automation interruption fixture".into());
    }
    let mut store = WorldStore::open(root, backend)?;
    let mut records = read_records(&mut store)?;
    let state = State::decode(&records, &items, &quests)?;
    let (submitted, energy, tokens) = state.audit(&items)?;
    if mode == "inspect" {
        let report = json!({"status":"PASS","backend":backend.name(),"phase":marker.phase,"sequence":store.sequence()?,
            "record_sha256":digest(&records),"expected_before_sha256":marker.before_sha256,"expected_after_sha256":marker.after_sha256,
            "records":records.len(),"weighted_material_units":24,"energy_consumed":energy,"submitted_ingots":submitted,
            "reward_tokens":tokens,"claimed":state.book.has_claimed(&qid(2)),"scheduler_ticks":state.network.ticks()});
        if let Some(receipt) = args.get(2) {
            fs::write(receipt, serde_json::to_vec_pretty(&report)?)?;
        }
        println!("{report}");
        return Ok(());
    }
    if digest(&records) != marker.before_sha256 {
        return Err("writer requires its exact initial fixture state".into());
    }
    println!("READY");
    io::stdout().flush()?;
    for _ in 0..10000 {
        tick(&mut records, &items, &quests, true)?;
        let refs = records.values().collect::<Vec<_>>();
        match &mut store {
            WorldStore::Journal(store) => {
                let prepared = store.prepare_batch(&refs)?;
                if mode == "prepare-exit" {
                    std::process::exit(71);
                }
                store.commit(prepared)?;
            }
            #[cfg(feature = "paged")]
            WorldStore::Paged(store) => {
                let prepared = store.begin_batch(&refs)?;
                if mode == "prepare-exit" {
                    std::process::exit(71);
                }
                prepared.commit()?;
            }
        }
        if mode == "commit-exit" {
            std::process::exit(72);
        }
    }
    Err("interruption writer reached its bound without being stopped".into())
}
