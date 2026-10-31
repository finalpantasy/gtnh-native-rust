//! Fresh-process growth measurements with a handshake for OS memory sampling.
use crate::world_storage::{Backend, WorldStore};
use serde_json::json;
use std::{
    fs,
    io::{self, Write},
    path::Path,
    time::Instant,
};
use voxel_core::{coords::CubePos, record::CubeRecord, storage::CubeStorage};
type Error = Box<dyn std::error::Error>;
fn position(index: u64) -> CubePos {
    CubePos::new(
        (index % 64) as i64 - 32,
        if (index / 64).is_multiple_of(2) {
            1_000_000
        } else {
            -1_000_000
        },
        (index / 128) as i64 - 16,
    )
}
fn payload(index: u64, epoch: u64) -> Vec<u8> {
    index
        .to_le_bytes()
        .into_iter()
        .chain(epoch.to_le_bytes())
        .collect()
}
fn disk(root: &Path) -> Result<(u64, u64), Error> {
    let (mut bytes, mut files) = (0, 0);
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            bytes += entry.metadata()?.len();
            files += 1;
        }
    }
    Ok((bytes, files))
}
fn sample(root: &Path, phase: &str, count: u64, milliseconds: f64) -> Result<(), Error> {
    let (bytes, files) = disk(root)?;
    println!(
        "{}",
        json!({"phase":phase,"cubes":count,"milliseconds":milliseconds,"disk_bytes":bytes,"files":files})
    );
    io::stdout().flush()?;
    let mut ack = String::new();
    if io::stdin().read_line(&mut ack)? == 0 || ack.trim() != "sampled" {
        return Err("measurement handshake missing".into());
    }
    Ok(())
}
pub fn execute(args: &[String]) -> Result<(), Error> {
    if args.len() != 2 {
        return Err(
            "storage-bench journal|paged NEW_FIXTURE_DIR (use tools/measure_storage.py)".into(),
        );
    }
    let backend = match args[0].as_str() {
        "journal" => Backend::Journal,
        "paged" => Backend::Paged,
        _ => return Err("invalid benchmark backend".into()),
    };
    let root = Path::new(&args[1]);
    if root.exists() {
        return Err("benchmark requires a new private fixture".into());
    }
    let mut store = WorldStore::open(root, backend)?;
    sample(root, "empty", 0, 0.)?;
    let (mut previous, mut epoch, mut assertions) = (0, 0, 0u64);
    for count in [128, 1024, 4096] {
        let start = Instant::now();
        epoch += 1;
        for begin in (previous..count).step_by(64) {
            let mut records = Vec::new();
            for index in begin..count.min(begin + 64) {
                let mut cube = CubeRecord::new(position(index))?;
                cube.set(0, 1)?;
                cube.replace_payload(payload(index, epoch))?;
                records.push(cube);
            }
            store.save_batch(&records.iter().collect::<Vec<_>>())?;
        }
        for _ in 0..2 {
            epoch += 1;
            for begin in (0..count).step_by(64) {
                let mut records = Vec::new();
                for index in begin..count.min(begin + 64) {
                    let mut cube = store
                        .load(position(index))?
                        .ok_or("benchmark cube missing")?;
                    cube.replace_payload(payload(index, epoch))?;
                    records.push(cube);
                }
                store.save_batch(&records.iter().collect::<Vec<_>>())?;
            }
        }
        sample(
            root,
            "updated",
            count,
            start.elapsed().as_secs_f64() * 1000.,
        )?;
        drop(store);
        let reopen = Instant::now();
        store = WorldStore::open(root, backend)?;
        sample(
            root,
            "reopened",
            count,
            reopen.elapsed().as_secs_f64() * 1000.,
        )?;
        let read = Instant::now();
        for index in (0..count).rev() {
            let cube = store
                .load(position(index))?
                .ok_or("reopened benchmark cube missing")?;
            assert_eq!(cube.payload(), payload(index, epoch));
            assert_eq!(cube.blocks()[0], 1);
            assertions += 2;
        }
        sample(
            root,
            "validated",
            count,
            read.elapsed().as_secs_f64() * 1000.,
        )?;
        previous = count;
    }
    println!(
        "{}",
        json!({"phase":"done","status":"PASS","backend":backend.name(),"assertions":assertions,"commits":store.sequence()?})
    );
    Ok(())
}
