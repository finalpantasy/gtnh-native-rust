//! Cube persistence. Filesystem records are immutable snapshots installed atomically.

use crate::{
    coords::CubePos,
    record::{CubeRecord, MAX_ENCODED, RecordError},
};
use std::{
    collections::BTreeMap,
    fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Record(RecordError),
    Conflict,
    WrongPosition,
    AtomicBatchUnsupported,
    SequenceExhausted,
    Backend(String),
}
impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for StoreError {}
impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<RecordError> for StoreError {
    fn from(e: RecordError) -> Self {
        Self::Record(e)
    }
}

pub trait CubeStorage {
    fn load(&mut self, pos: CubePos) -> Result<Option<CubeRecord>, StoreError>;
    fn save(&mut self, record: &CubeRecord) -> Result<(), StoreError>;
    fn save_batch(&mut self, records: &[&CubeRecord]) -> Result<(), StoreError> {
        if records.len() == 1 {
            self.save(records[0])
        } else {
            Err(StoreError::AtomicBatchUnsupported)
        }
    }
}

#[derive(Default)]
pub struct MemoryStorage {
    records: BTreeMap<CubePos, Vec<u8>>,
    pub reject_writes: bool,
}
impl CubeStorage for MemoryStorage {
    fn load(&mut self, pos: CubePos) -> Result<Option<CubeRecord>, StoreError> {
        self.records
            .get(&pos)
            .map(|b| CubeRecord::decode(b).map_err(StoreError::from))
            .transpose()
    }
    fn save(&mut self, record: &CubeRecord) -> Result<(), StoreError> {
        self.save_batch(&[record])
    }
    fn save_batch(&mut self, records: &[&CubeRecord]) -> Result<(), StoreError> {
        if self.reject_writes {
            return Err(StoreError::Io(io::Error::other("injected write failure")));
        }
        if records.is_empty() || records.len() > 4096 {
            return Err(RecordError::Invalid("batch size").into());
        }
        let mut staged = BTreeMap::new();
        for record in records {
            let bytes = record.encode();
            if let Some(existing) = self.records.get(&record.pos()) {
                let old = CubeRecord::decode(existing)?;
                if old.revision() > record.revision()
                    || (old.revision() == record.revision() && existing != &bytes)
                {
                    return Err(StoreError::Conflict);
                }
            }
            if staged.insert(record.pos(), bytes).is_some() {
                return Err(StoreError::Conflict);
            }
        }
        self.records.extend(staged);
        Ok(())
    }
}

pub struct SnapshotStorage {
    root: PathBuf,
    index: BTreeMap<CubePos, (u64, PathBuf)>,
    _lock: fs::File,
}

impl SnapshotStorage {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(".owner.lock"))?;
        lock.try_lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        let mut index = BTreeMap::new();
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("ncub") {
                continue;
            }
            if !entry.file_type()?.is_file() {
                return Err(io::Error::other("snapshot is not a regular file").into());
            }
            let name = entry.file_name();
            let parts: Vec<&str> = name
                .to_str()
                .ok_or_else(|| io::Error::other("snapshot filename is not UTF-8"))?
                .split('.')
                .collect();
            let invalid = || io::Error::other("invalid cube snapshot filename");
            if parts.len() != 6 || parts[0] != "c" || parts[5] != "ncub" {
                return Err(invalid().into());
            }
            let pos = CubePos::new(
                parts[1].parse().map_err(|_| invalid())?,
                parts[2].parse().map_err(|_| invalid())?,
                parts[3].parse().map_err(|_| invalid())?,
            );
            let revision: u64 = parts[4]
                .strip_prefix('r')
                .ok_or_else(invalid)?
                .parse()
                .map_err(|_| invalid())?;
            if pos.block_at(0).is_none() {
                return Err(invalid().into());
            }
            let value = index.entry(pos).or_insert((revision, path.clone()));
            if revision > value.0 {
                *value = (revision, path);
            }
        }
        Ok(Self {
            root,
            index,
            _lock: lock,
        })
    }
}

impl CubeStorage for SnapshotStorage {
    fn load(&mut self, pos: CubePos) -> Result<Option<CubeRecord>, StoreError> {
        let Some((revision, path)) = self.index.get(&pos) else {
            return Ok(None);
        };
        let record = CubeRecord::decode(&read_bounded(path)?)?;
        if record.pos() != pos {
            return Err(StoreError::WrongPosition);
        }
        if record.revision() != *revision {
            return Err(RecordError::Invalid("filename revision mismatch").into());
        }
        Ok(Some(record))
    }
    fn save(&mut self, record: &CubeRecord) -> Result<(), StoreError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let pos = record.pos();
        if self
            .index
            .get(&pos)
            .is_some_and(|entry| entry.0 > record.revision())
        {
            return Err(StoreError::Conflict);
        }
        let bytes = record.encode();
        let name = format!(
            "c.{}.{}.{}.r{}.ncub",
            pos.x,
            pos.y,
            pos.z,
            record.revision()
        );
        let destination = self.root.join(&name);
        if destination.exists() {
            if read_bounded(&destination)? != bytes {
                return Err(StoreError::Conflict);
            }
            self.index.insert(pos, (record.revision(), destination));
            return Ok(());
        }
        let temporary = self.root.join(format!(
            ".{name}.{}.{}.partial",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let installed = (|| -> Result<(), StoreError> {
            file.write_all(&bytes)?;
            file.sync_all()?;
            // A hard link installs a complete file without replacing an existing snapshot.
            match fs::hard_link(&temporary, &destination) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if read_bounded(&destination)? == bytes {
                        Ok(())
                    } else {
                        Err(StoreError::Conflict)
                    }
                }
                Err(error) => Err(error.into()),
            }
        })();
        drop(file);
        // This path was created exclusively by this invocation, inside this store.
        let _ = fs::remove_file(&temporary);
        installed?;
        self.index.insert(pos, (record.revision(), destination));
        Ok(())
    }
}

pub(crate) fn read_bounded(path: &Path) -> Result<Vec<u8>, StoreError> {
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > MAX_ENCODED as u64 {
        return Err(RecordError::TooLarge.into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_ENCODED as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_ENCODED {
        return Err(RecordError::TooLarge.into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn directory(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "gtnh-cubes-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn snapshots_survive_close_reopen_and_keep_coordinates() {
        let root = directory("roundtrip");
        let mut cube = CubeRecord::new(CubePos::new(-99, 1_000_000, -2)).unwrap();
        cube.set(8, 1234).unwrap();
        cube.replace_payload(vec![1, 3, 7]).unwrap();
        {
            let mut store = SnapshotStorage::open(&root).unwrap();
            store.save(&cube).unwrap();
        }
        let mut reopened = SnapshotStorage::open(&root).unwrap();
        assert_eq!(reopened.load(cube.pos()).unwrap(), Some(cube));
        assert!(reopened.load(CubePos::new(0, 0, 0)).unwrap().is_none());
    }

    #[test]
    fn incomplete_temporary_write_cannot_hide_last_complete_snapshot() {
        let root = directory("interrupted");
        let cube = CubeRecord::new(CubePos::new(0, -2, 0)).unwrap();
        {
            let mut store = SnapshotStorage::open(&root).unwrap();
            store.save(&cube).unwrap();
        }
        fs::write(root.join("interrupted.partial"), b"NCUB incomplete").unwrap();
        let mut reopened = SnapshotStorage::open(&root).unwrap();
        assert_eq!(reopened.load(cube.pos()).unwrap(), Some(cube));
    }

    #[test]
    fn repeated_save_is_idempotent_and_latest_revision_wins() {
        let root = directory("latest");
        let mut cube = CubeRecord::new(CubePos::new(1, 2, 3)).unwrap();
        {
            let mut store = SnapshotStorage::open(&root).unwrap();
            store.save(&cube).unwrap();
            store.save(&cube).unwrap();
            cube.set(0, 9).unwrap();
            store.save(&cube).unwrap();
        }
        let mut reopened = SnapshotStorage::open(&root).unwrap();
        assert_eq!(reopened.load(cube.pos()).unwrap(), Some(cube));
    }

    #[test]
    fn second_writer_is_rejected_without_replacing_the_lock() {
        let root = directory("locking");
        let first = SnapshotStorage::open(&root).unwrap();
        assert!(SnapshotStorage::open(&root).is_err());
        drop(first);
        assert!(SnapshotStorage::open(&root).is_ok());
    }

    #[test]
    fn conflicting_revision_cannot_replace_existing_data() {
        let root = directory("conflict");
        let mut original = CubeRecord::new(CubePos::new(0, 0, 0)).unwrap();
        let mut conflicting = original.clone();
        original.set(0, 1).unwrap();
        conflicting.set(0, 2).unwrap();
        let mut store = SnapshotStorage::open(&root).unwrap();
        store.save(&original).unwrap();
        assert!(matches!(
            store.save(&conflicting),
            Err(StoreError::Conflict)
        ));
        assert_eq!(store.load(original.pos()).unwrap(), Some(original));
    }

    #[test]
    fn corrupt_latest_snapshot_is_reported_instead_of_silently_rolling_back() {
        let root = directory("corrupt");
        let mut cube = CubeRecord::new(CubePos::new(0, 0, 0)).unwrap();
        {
            let mut store = SnapshotStorage::open(&root).unwrap();
            store.save(&cube).unwrap();
            cube.set(0, 5).unwrap();
            store.save(&cube).unwrap();
        }
        fs::write(root.join("c.0.0.0.r1.ncub"), b"truncated").unwrap();
        let mut reopened = SnapshotStorage::open(&root).unwrap();
        assert!(matches!(
            reopened.load(cube.pos()),
            Err(StoreError::Record(_))
        ));
    }
}
