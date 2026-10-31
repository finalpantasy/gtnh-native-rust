//! Atomic visibility of multi-cube snapshots through an immutable commit record.
//! File contents are synced before installation. Directory power-loss durability is platform dependent.

use crate::{
    coords::CubePos,
    record::{CubeRecord, RecordError, checksum},
    storage::{CubeStorage, StoreError, read_bounded},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const FORMAT: &[u8] = b"native-cube-journal-v1\n";
const MAX_MEMBERS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SnapshotRef {
    pos: CubePos,
    revision: u64,
    hash: u64,
}

pub struct PreparedBatch {
    owner: u64,
    base: u64,
    sequence: u64,
    members: Vec<SnapshotRef>,
}

pub struct JournalStorage {
    root: PathBuf,
    index: BTreeMap<CubePos, SnapshotRef>,
    sequence: u64,
    owner: u64,
    _lock: fs::File,
}

impl JournalStorage {
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
        static OWNER: AtomicU64 = AtomicU64::new(0);
        let owner = OWNER
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| StoreError::SequenceExhausted)?;
        let format = root.join(".world-format");
        if format.exists() {
            if read_bounded(&format)? != FORMAT {
                return Err(RecordError::Invalid("world format").into());
            }
        } else {
            for entry in fs::read_dir(&root)? {
                let path = entry?.path();
                if path.file_name().and_then(|s| s.to_str()) != Some(".owner.lock")
                    && path.extension().and_then(|s| s.to_str()) != Some("partial")
                {
                    return Err(RecordError::Invalid(
                        "nonempty world requires explicit format migration",
                    )
                    .into());
                }
            }
            install(&root, ".world-format", FORMAT)?;
        }
        let mut commits = BTreeMap::new();
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("ncommit") {
                continue;
            }
            if !entry.file_type()?.is_file() {
                return Err(RecordError::Invalid("nonregular commit file").into());
            }
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or(RecordError::Invalid("commit filename"))?;
            let sequence: u64 = name
                .strip_prefix("t.")
                .and_then(|s| s.strip_suffix(".ncommit"))
                .ok_or(RecordError::Invalid("commit filename"))?
                .parse()
                .map_err(|_| RecordError::Invalid("commit filename"))?;
            if commits.insert(sequence, path).is_some() {
                return Err(StoreError::Conflict);
            }
        }
        let mut index: BTreeMap<CubePos, SnapshotRef> = BTreeMap::new();
        let mut sequence = 0u64;
        for (number, path) in commits {
            if number
                != sequence
                    .checked_add(1)
                    .ok_or(StoreError::SequenceExhausted)?
            {
                return Err(RecordError::Invalid("commit sequence gap").into());
            }
            let (encoded, members) = decode_commit(&read_bounded(&path)?)?;
            if encoded != number {
                return Err(RecordError::Invalid("commit filename sequence mismatch").into());
            }
            for member in members {
                if let Some(old) = index.get(&member.pos)
                    && (old.revision > member.revision
                        || (old.revision == member.revision && old.hash != member.hash))
                {
                    return Err(StoreError::Conflict);
                }
                index.insert(member.pos, member);
            }
            sequence = number;
        }
        // Refuse the entire store if any current committed member is damaged.
        for reference in index.values() {
            read_snapshot(&root, *reference)?;
        }
        Ok(Self {
            root,
            index,
            sequence,
            owner,
            _lock: lock,
        })
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn prepare_batch(&mut self, records: &[&CubeRecord]) -> Result<PreparedBatch, StoreError> {
        if records.is_empty() || records.len() > MAX_MEMBERS {
            return Err(RecordError::Invalid("batch size").into());
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(StoreError::SequenceExhausted)?;
        let mut positions = BTreeSet::new();
        let mut encoded = Vec::with_capacity(records.len());
        for record in records {
            if !positions.insert(record.pos()) {
                return Err(StoreError::Conflict);
            }
            let bytes = record.encode();
            let reference = SnapshotRef {
                pos: record.pos(),
                revision: record.revision(),
                hash: checksum(&bytes),
            };
            if let Some(old) = self.index.get(&record.pos()) {
                if old.revision > record.revision() {
                    return Err(StoreError::Conflict);
                }
                if old.revision == record.revision()
                    && (old.hash != reference.hash || read_snapshot(&self.root, *old)? != **record)
                {
                    return Err(StoreError::Conflict);
                }
            }
            encoded.push((reference, bytes));
        }
        let mut members = Vec::with_capacity(encoded.len());
        for (reference, bytes) in encoded {
            install(&self.root, &snapshot_name(reference), &bytes)?;
            members.push(reference);
        }
        members.sort_by_key(|r| r.pos);
        Ok(PreparedBatch {
            owner: self.owner,
            base: self.sequence,
            sequence,
            members,
        })
    }
    pub fn commit(&mut self, batch: PreparedBatch) -> Result<(), StoreError> {
        if batch.owner != self.owner
            || batch.base != self.sequence
            || self.sequence.checked_add(1) != Some(batch.sequence)
        {
            return Err(StoreError::Conflict);
        }
        for member in &batch.members {
            read_snapshot(&self.root, *member)?;
        }
        let bytes = encode_commit(batch.sequence, &batch.members);
        install(&self.root, &format!("t.{}.ncommit", batch.sequence), &bytes)?;
        for member in batch.members {
            self.index.insert(member.pos, member);
        }
        self.sequence = batch.sequence;
        Ok(())
    }
}

impl CubeStorage for JournalStorage {
    fn load(&mut self, pos: CubePos) -> Result<Option<CubeRecord>, StoreError> {
        self.index
            .get(&pos)
            .map(|r| read_snapshot(&self.root, *r))
            .transpose()
    }
    fn save(&mut self, record: &CubeRecord) -> Result<(), StoreError> {
        self.save_batch(&[record])
    }
    fn save_batch(&mut self, records: &[&CubeRecord]) -> Result<(), StoreError> {
        let batch = self.prepare_batch(records)?;
        self.commit(batch)
    }
}

fn snapshot_name(reference: SnapshotRef) -> String {
    format!(
        "c.{}.{}.{}.r{}.h{:016x}.ncub",
        reference.pos.x, reference.pos.y, reference.pos.z, reference.revision, reference.hash
    )
}

fn read_snapshot(root: &Path, reference: SnapshotRef) -> Result<CubeRecord, StoreError> {
    let path = root.join(snapshot_name(reference));
    if !fs::symlink_metadata(&path)?.file_type().is_file() {
        return Err(RecordError::Invalid("nonregular snapshot").into());
    }
    let bytes = read_bounded(&path)?;
    if checksum(&bytes) != reference.hash {
        return Err(RecordError::ChecksumMismatch.into());
    }
    let record = CubeRecord::decode(&bytes)?;
    if record.pos() != reference.pos {
        return Err(StoreError::WrongPosition);
    }
    if record.revision() != reference.revision {
        return Err(RecordError::Invalid("snapshot revision mismatch").into());
    }
    Ok(record)
}

fn encode_commit(sequence: u64, members: &[SnapshotRef]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(28 + members.len() * 40);
    bytes.extend_from_slice(b"NCMT");
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&sequence.to_le_bytes());
    bytes.extend_from_slice(&(members.len() as u32).to_le_bytes());
    for member in members {
        for coordinate in [member.pos.x, member.pos.y, member.pos.z] {
            bytes.extend_from_slice(&coordinate.to_le_bytes());
        }
        bytes.extend_from_slice(&member.revision.to_le_bytes());
        bytes.extend_from_slice(&member.hash.to_le_bytes());
    }
    let sum = checksum(&bytes);
    bytes.extend_from_slice(&sum.to_le_bytes());
    bytes
}

fn decode_commit(bytes: &[u8]) -> Result<(u64, Vec<SnapshotRef>), StoreError> {
    if bytes.len() < 28 || bytes.len() > 28 + MAX_MEMBERS * 40 {
        return Err(RecordError::Invalid("commit length").into());
    }
    if &bytes[..4] != b"NCMT" {
        return Err(RecordError::Invalid("commit magic").into());
    }
    let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
    if version != 1 {
        return Err(RecordError::UnsupportedVersion(version).into());
    }
    if bytes[6..8] != [0, 0] {
        return Err(RecordError::Invalid("commit flags").into());
    }
    let tail = bytes.len() - 8;
    if checksum(&bytes[..tail]) != u64::from_le_bytes(bytes[tail..].try_into().unwrap()) {
        return Err(RecordError::ChecksumMismatch.into());
    }
    let sequence = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let count = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as usize;
    if count == 0 || count > MAX_MEMBERS || 28 + count * 40 != bytes.len() {
        return Err(RecordError::Invalid("commit member count").into());
    }
    let mut members = Vec::with_capacity(count);
    let mut positions = BTreeSet::new();
    for part in bytes[20..tail].chunks_exact(40) {
        let coord = |i| i64::from_le_bytes(part[i..i + 8].try_into().unwrap());
        let pos = CubePos::new(coord(0), coord(8), coord(16));
        if pos.block_at(0).is_none() || !positions.insert(pos) {
            return Err(RecordError::Invalid("commit positions").into());
        }
        members.push(SnapshotRef {
            pos,
            revision: u64::from_le_bytes(part[24..32].try_into().unwrap()),
            hash: u64::from_le_bytes(part[32..40].try_into().unwrap()),
        });
    }
    Ok((sequence, members))
}

fn install(root: &Path, name: &str, bytes: &[u8]) -> Result<(), StoreError> {
    let path = root.join(name);
    if path.exists() {
        return if read_bounded(&path)? == bytes {
            Ok(())
        } else {
            Err(StoreError::Conflict)
        };
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temporary = root.join(format!(
        ".{name}.{}.{}.partial",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let outcome = (|| -> Result<(), StoreError> {
        file.write_all(bytes)?;
        file.sync_all()?;
        match fs::hard_link(&temporary, &path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if read_bounded(&path)? == bytes {
                    Ok(())
                } else {
                    Err(StoreError::Conflict)
                }
            }
            Err(error) => Err(error.into()),
        }
    })();
    drop(file);
    let _ = fs::remove_file(&temporary);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "gtnh-journal-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }
    fn pair() -> (CubeRecord, CubeRecord) {
        let mut a = CubeRecord::new(CubePos::new(-1, 1_000_000, 0)).unwrap();
        let mut b = CubeRecord::new(CubePos::new(-1, 1_000_001, 0)).unwrap();
        a.replace_payload(vec![10]).unwrap();
        b.replace_payload(vec![0]).unwrap();
        (a, b)
    }
    #[test]
    fn both_sides_of_a_transfer_reappear_after_reopen() {
        let root = root("pair");
        let (mut a, mut b) = pair();
        {
            let mut store = JournalStorage::open(&root).unwrap();
            store.save_batch(&[&a, &b]).unwrap();
            a.replace_payload(vec![7]).unwrap();
            b.replace_payload(vec![3]).unwrap();
            store.save_batch(&[&a, &b]).unwrap();
        }
        let mut store = JournalStorage::open(&root).unwrap();
        assert_eq!(store.sequence(), 2);
        assert_eq!(store.load(a.pos()).unwrap(), Some(a));
        assert_eq!(store.load(b.pos()).unwrap(), Some(b));
    }
    #[test]
    fn prepared_but_uncommitted_data_is_never_visible_after_restart() {
        let root = root("interrupted");
        let (mut a, mut b) = pair();
        let old = (a.clone(), b.clone());
        {
            let mut store = JournalStorage::open(&root).unwrap();
            store.save_batch(&[&a, &b]).unwrap();
            a.replace_payload(vec![6]).unwrap();
            b.replace_payload(vec![4]).unwrap();
            let _abandoned = store.prepare_batch(&[&a, &b]).unwrap();
        }
        let mut store = JournalStorage::open(&root).unwrap();
        assert_eq!(store.sequence(), 1);
        assert_eq!(store.load(a.pos()).unwrap(), Some(old.0));
        assert_eq!(store.load(b.pos()).unwrap(), Some(old.1));
        // An orphan at the same revision must not block a different later valid edit.
        a.replace_payload(vec![8]).unwrap();
        b.replace_payload(vec![2]).unwrap();
        store.save_batch(&[&a, &b]).unwrap();
        assert_eq!(store.load(a.pos()).unwrap(), Some(a));
    }
    #[test]
    fn a_prepared_batch_cannot_commit_after_another_commit_or_owner_change() {
        let root = root("stale");
        let (a, b) = pair();
        let stale;
        {
            let mut store = JournalStorage::open(&root).unwrap();
            stale = store.prepare_batch(&[&a, &b]).unwrap();
            store.save(&a).unwrap();
        }
        let mut reopened = JournalStorage::open(&root).unwrap();
        assert!(matches!(reopened.commit(stale), Err(StoreError::Conflict)));
        assert_eq!(reopened.sequence(), 1);
    }
    #[test]
    fn duplicate_members_and_corrupt_commit_records_are_rejected() {
        let root = root("corrupt");
        let (a, b) = pair();
        {
            let mut store = JournalStorage::open(&root).unwrap();
            assert!(store.save_batch(&[&a, &a]).is_err());
            store.save_batch(&[&a, &b]).unwrap();
        }
        fs::write(root.join("t.1.ncommit"), b"damaged commit").unwrap();
        assert!(JournalStorage::open(&root).is_err());
    }
}
