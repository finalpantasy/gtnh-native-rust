//! Subprocess-only journal crash fixture. Never points at player saves.

use crate::world_storage::{Backend, WorldStore};
use serde_json::json;
use std::{
    fs,
    io::{self, Write},
    path::Path,
};
use voxel_core::{coords::CubePos, record::CubeRecord, storage::CubeStorage};

type Error = Box<dyn std::error::Error>;
const A: CubePos = CubePos::new(-1, 1_000_000, 0);
const B: CubePos = CubePos::new(-1, 1_000_001, 0);
fn value(cube: &CubeRecord) -> Result<u64, Error> {
    Ok(u64::from_le_bytes(cube.payload().try_into()?))
}

pub fn execute(args: &[String]) -> Result<(), Error> {
    if args.len() < 2 {
        return Err("journal-probe MODE PRIVATE_FIXTURE_DIR [RECEIPT]".into());
    }
    let mode = args[0].as_str();
    let root = Path::new(&args[1]);
    #[cfg(feature = "paged")]
    let (mode, backend) = mode
        .strip_prefix("paged-")
        .map(|mode| (mode, Backend::Paged))
        .unwrap_or((mode, Backend::Journal));
    #[cfg(not(feature = "paged"))]
    let backend = Backend::Journal;
    // A dedicated marker prevents accidental use of ordinary worlds.
    let marker = root.join(".crash-fixture");
    if mode == "init" {
        if root.exists() {
            return Err("crash fixture directory must be new".into());
        }
        let mut store = WorldStore::open(root, backend)?;
        let mut a = CubeRecord::new(A)?;
        let mut b = CubeRecord::new(B)?;
        a.replace_payload(1000u64.to_le_bytes().to_vec())?;
        b.replace_payload(0u64.to_le_bytes().to_vec())?;
        store.save_batch(&[&a, &b])?;
        fs::write(
            marker,
            format!("native-crash-fixture-v2:{}", backend.name()),
        )?;
        return Ok(());
    }
    if fs::read(&marker)? != format!("native-crash-fixture-v2:{}", backend.name()).as_bytes() {
        return Err("not an owned crash fixture".into());
    }
    let mut store = WorldStore::open(root, backend)?;
    if mode == "inspect" {
        let a = store.load(A)?.ok_or("missing source")?;
        let b = store.load(B)?.ok_or("missing target")?;
        let (left, right) = (value(&a)?, value(&b)?);
        assert_eq!(left + right, 1000);
        assert_eq!(a.revision(), b.revision());
        let report = json!({"status":"PASS","backend":backend.name(),"sequence":store.sequence()?,"source":left,"target":right,"total":left+right,"revision":a.revision()});
        if let Some(output) = args.get(2) {
            fs::write(output, serde_json::to_vec_pretty(&report)?)?;
        }
        println!("{report}");
        return Ok(());
    }
    if !matches!(mode, "prepare-exit" | "commit-exit" | "loop") {
        return Err("invalid crash mode".into());
    }
    println!("READY");
    io::stdout().flush()?;
    for _ in 0..10000 {
        let mut a = store.load(A)?.ok_or("missing source")?;
        let mut b = store.load(B)?.ok_or("missing target")?;
        let mut left = value(&a)?;
        let mut right = value(&b)?;
        if left > 0 {
            left -= 1;
            right += 1;
        } else {
            left = 1000;
            right = 0;
        }
        a.replace_payload(left.to_le_bytes().to_vec())?;
        b.replace_payload(right.to_le_bytes().to_vec())?;
        match &mut store {
            WorldStore::Journal(store) => {
                let prepared = store.prepare_batch(&[&a, &b])?;
                if mode == "prepare-exit" {
                    std::process::exit(71);
                }
                store.commit(prepared)?;
            }
            #[cfg(feature = "paged")]
            WorldStore::Paged(store) => {
                let prepared = store.begin_batch(&[&a, &b])?;
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
    Err("crash loop reached its bound without being stopped".into())
}
