//! Optional paged transactional backend. Cache configuration is not a whole-process RAM cap.

use crate::{
    coords::CubePos,
    record::{CubeRecord, RecordError},
    storage::{CubeStorage, StoreError},
};
use redb::{
    Database, Durability, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition,
    WriteTransaction,
};
use std::{
    collections::BTreeSet,
    fs,
    marker::PhantomData,
    path::{Path, PathBuf},
};

const CUBES: TableDefinition<(i64, i64, i64), &[u8]> = TableDefinition::new("native.cubes.v1");
const META: TableDefinition<&str, u64> = TableDefinition::new("native.meta.v1");
fn backend(error: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(error.to_string())
}
fn key(pos: CubePos) -> (i64, i64, i64) {
    (pos.x, pos.y, pos.z)
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub cache_bytes: usize,
    pub max_batch_bytes: usize,
    pub max_batch_cubes: usize,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            cache_bytes: 8 * 1024 * 1024,
            max_batch_bytes: 8 * 1024 * 1024,
            max_batch_cubes: 128,
        }
    }
}
pub struct PagedStorage {
    db: Database,
    path: PathBuf,
    config: Config,
}
/// Dropping a batch aborts it. The mutable owner borrow prevents a second writer or owner drop.
pub struct Batch<'a> {
    transaction: WriteTransaction,
    _owner: PhantomData<&'a mut PagedStorage>,
}
impl Batch<'_> {
    pub fn commit(self) -> Result<(), StoreError> {
        self.transaction.commit().map_err(backend)
    }
}
impl PagedStorage {
    pub fn open(root: impl AsRef<Path>, config: Config) -> Result<Self, StoreError> {
        if !(64 * 1024..=256 * 1024 * 1024).contains(&config.cache_bytes)
            || config.max_batch_bytes == 0
            || config.max_batch_bytes > 64 * 1024 * 1024
            || config.max_batch_cubes == 0
            || config.max_batch_cubes > 4096
        {
            return Err(RecordError::Invalid("paged storage bounds").into());
        }
        let root = root.as_ref();
        fs::create_dir_all(root)?;
        if root.join(".world-format").exists() {
            return Err(RecordError::Invalid("different world storage format").into());
        }
        let path = root.join("native-world-v1.redb");
        let existing = path.exists();
        let db = Database::builder()
            .set_cache_size(config.cache_bytes)
            .create(&path)
            .map_err(backend)?;
        if !existing {
            let mut write = db.begin_write().map_err(backend)?;
            write
                .set_durability(Durability::Immediate)
                .map_err(backend)?;
            {
                let mut meta = write.open_table(META).map_err(backend)?;
                meta.insert("format", 1).map_err(backend)?;
                meta.insert("sequence", 0).map_err(backend)?;
            }
            {
                let _cubes = write.open_table(CUBES).map_err(backend)?;
            }
            write.commit().map_err(backend)?;
        }
        {
            let read = db.begin_read().map_err(backend)?;
            let meta = read.open_table(META).map_err(backend)?;
            if meta.get("format").map_err(backend)?.map(|v| v.value()) != Some(1)
                || meta.get("sequence").map_err(backend)?.is_none()
            {
                return Err(RecordError::Invalid("paged world schema").into());
            }
            let _cubes = read.open_table(CUBES).map_err(backend)?;
        }
        Ok(Self { db, path, config })
    }
    pub fn begin_batch(&mut self, records: &[&CubeRecord]) -> Result<Batch<'_>, StoreError> {
        if records.is_empty() || records.len() > self.config.max_batch_cubes {
            return Err(RecordError::Invalid("paged batch count").into());
        }
        let mut positions = BTreeSet::new();
        let mut bytes = 0usize;
        for record in records {
            if !positions.insert(record.pos()) {
                return Err(StoreError::Conflict);
            }
            bytes = bytes
                .checked_add(crate::record::MIN_ENCODED + record.payload().len())
                .ok_or(RecordError::TooLarge)?;
            if bytes > self.config.max_batch_bytes {
                return Err(RecordError::TooLarge.into());
            }
        }
        let mut transaction = self.db.begin_write().map_err(backend)?;
        transaction
            .set_durability(Durability::Immediate)
            .map_err(backend)?;
        {
            let mut table = transaction.open_table(CUBES).map_err(backend)?;
            for record in records {
                let encoded = record.encode();
                {
                    if let Some(previous) = table.get(key(record.pos())).map_err(backend)? {
                        let old = CubeRecord::decode(previous.value())?;
                        if old.pos() != record.pos() {
                            return Err(StoreError::WrongPosition);
                        }
                        if old.revision() > record.revision()
                            || (old.revision() == record.revision() && previous.value() != encoded)
                        {
                            return Err(StoreError::Conflict);
                        }
                    }
                }
                table
                    .insert(key(record.pos()), encoded.as_slice())
                    .map_err(backend)?;
            }
        }
        {
            let mut meta = transaction.open_table(META).map_err(backend)?;
            let sequence = meta
                .get("sequence")
                .map_err(backend)?
                .ok_or(RecordError::Invalid("missing paged sequence"))?
                .value();
            meta.insert(
                "sequence",
                sequence
                    .checked_add(1)
                    .ok_or(StoreError::SequenceExhausted)?,
            )
            .map_err(backend)?;
        }
        Ok(Batch {
            transaction,
            _owner: PhantomData,
        })
    }
    pub fn sequence(&self) -> Result<u64, StoreError> {
        let read = self.db.begin_read().map_err(backend)?;
        let meta = read.open_table(META).map_err(backend)?;
        Ok(meta
            .get("sequence")
            .map_err(backend)?
            .ok_or(RecordError::Invalid("missing paged sequence"))?
            .value())
    }
    pub fn len(&self) -> Result<u64, StoreError> {
        self.db
            .begin_read()
            .map_err(backend)?
            .open_table(CUBES)
            .map_err(backend)?
            .len()
            .map_err(backend)
    }
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }
    pub fn file_bytes(&self) -> Result<u64, StoreError> {
        Ok(fs::metadata(&self.path)?.len())
    }
    pub fn config(&self) -> Config {
        self.config
    }
}
impl CubeStorage for PagedStorage {
    fn load(&mut self, pos: CubePos) -> Result<Option<CubeRecord>, StoreError> {
        if pos.block_at(0).is_none() {
            return Err(RecordError::Invalid("cube origin").into());
        }
        let read = self.db.begin_read().map_err(backend)?;
        let table = read.open_table(CUBES).map_err(backend)?;
        table
            .get(key(pos))
            .map_err(backend)?
            .map(|value| {
                if value.value().len() > crate::record::MAX_ENCODED {
                    return Err(RecordError::TooLarge.into());
                }
                let record = CubeRecord::decode(value.value())?;
                if record.pos() != pos {
                    return Err(StoreError::WrongPosition);
                }
                Ok(record)
            })
            .transpose()
    }
    fn save(&mut self, record: &CubeRecord) -> Result<(), StoreError> {
        self.save_batch(&[record])
    }
    fn save_batch(&mut self, records: &[&CubeRecord]) -> Result<(), StoreError> {
        self.begin_batch(records)?.commit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    fn root() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "gtnh-paged-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }
    fn pair() -> (CubeRecord, CubeRecord) {
        let mut a = CubeRecord::new(CubePos::new(i64::MIN / 16, i64::MAX / 16, -1)).unwrap();
        let mut b = CubeRecord::new(CubePos::new(-1, -1_000_000, 1)).unwrap();
        a.replace_payload(vec![10]).unwrap();
        b.replace_payload(vec![0]).unwrap();
        (a, b)
    }
    #[test]
    fn signed_extreme_keys_and_atomic_batches_survive_reopen() {
        let root = root();
        let (mut a, mut b) = pair();
        {
            let mut store = PagedStorage::open(&root, Config::default()).unwrap();
            store.save_batch(&[&a, &b]).unwrap();
            a.replace_payload(vec![7]).unwrap();
            b.replace_payload(vec![3]).unwrap();
            store.save_batch(&[&a, &b]).unwrap();
            assert_eq!(store.sequence().unwrap(), 2);
        }
        let mut store = PagedStorage::open(&root, Config::default()).unwrap();
        assert_eq!(store.load(a.pos()).unwrap(), Some(a));
        assert_eq!(store.load(b.pos()).unwrap(), Some(b));
        assert_eq!(store.sequence().unwrap(), 2);
    }
    #[test]
    fn dropped_preparation_rolls_back_and_does_not_hold_the_write_lock() {
        let root = root();
        let (mut a, mut b) = pair();
        let mut store = PagedStorage::open(&root, Config::default()).unwrap();
        store.save_batch(&[&a, &b]).unwrap();
        let old = (a.clone(), b.clone());
        a.replace_payload(vec![6]).unwrap();
        b.replace_payload(vec![4]).unwrap();
        {
            let _abandoned = store.begin_batch(&[&a, &b]).unwrap();
        }
        assert_eq!(store.sequence().unwrap(), 1);
        assert_eq!(store.load(a.pos()).unwrap(), Some(old.0));
        assert_eq!(store.load(b.pos()).unwrap(), Some(old.1));
        store.save_batch(&[&a, &b]).unwrap();
        assert_eq!(store.load(a.pos()).unwrap(), Some(a));
    }
    #[test]
    fn conflicts_duplicate_members_and_batch_bounds_do_not_change_either_cube() {
        let root = root();
        let (a, b) = pair();
        let mut store = PagedStorage::open(
            &root,
            Config {
                max_batch_cubes: 2,
                ..Config::default()
            },
        )
        .unwrap();
        store.save_batch(&[&a, &b]).unwrap();
        assert!(store.save_batch(&[&a, &a]).is_err());
        let mut stale = CubeRecord::new(a.pos()).unwrap();
        stale.replace_payload(vec![99]).unwrap();
        assert!(matches!(
            store.save_batch(&[&stale, &b]),
            Err(StoreError::Conflict)
        ));
        assert_eq!(store.load(a.pos()).unwrap(), Some(a.clone()));
        assert_eq!(store.load(b.pos()).unwrap(), Some(b.clone()));
        drop(store);
        let mut store = PagedStorage::open(
            &root,
            Config {
                max_batch_bytes: 16390,
                ..Config::default()
            },
        )
        .unwrap();
        assert!(store.save_batch(&[&a, &b]).is_err());
        assert_eq!(store.sequence().unwrap(), 1);
    }
    #[test]
    fn concurrent_owners_and_invalid_cache_configuration_are_rejected() {
        let root = root();
        let owner = PagedStorage::open(&root, Config::default()).unwrap();
        assert!(PagedStorage::open(&root, Config::default()).is_err());
        drop(owner);
        assert!(
            PagedStorage::open(
                &root,
                Config {
                    cache_bytes: 0,
                    ..Config::default()
                }
            )
            .is_err()
        );
        assert!(
            PagedStorage::open(
                &root,
                Config {
                    max_batch_cubes: 0,
                    ..Config::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn a_conflict_after_an_earlier_insert_rolls_back_the_entire_transaction() {
        let root = root();
        let (mut a, b) = pair();
        let mut store = PagedStorage::open(&root, Config::default()).unwrap();
        store.save_batch(&[&a, &b]).unwrap();
        let old = a.clone();
        a.replace_payload(vec![7]).unwrap();
        let mut conflict = CubeRecord::new(b.pos()).unwrap();
        conflict.replace_payload(vec![99]).unwrap();
        assert!(matches!(
            store.save_batch(&[&a, &conflict]),
            Err(StoreError::Conflict)
        ));
        assert_eq!(store.load(a.pos()).unwrap(), Some(old));
        assert_eq!(store.load(b.pos()).unwrap(), Some(b));
        assert_eq!(store.sequence().unwrap(), 1);
    }

    #[test]
    fn malformed_stored_cube_data_is_rejected_on_load() {
        let root = root();
        let (a, _) = pair();
        let mut store = PagedStorage::open(&root, Config::default()).unwrap();
        store.save(&a).unwrap();
        let write = store.db.begin_write().unwrap();
        {
            let mut table = write.open_table(CUBES).unwrap();
            table
                .insert(key(a.pos()), b"invalid cube".as_slice())
                .unwrap();
        }
        write.commit().unwrap();
        assert!(matches!(store.load(a.pos()), Err(StoreError::Record(_))));
    }
}
