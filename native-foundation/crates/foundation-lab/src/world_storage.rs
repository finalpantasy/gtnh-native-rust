//! Explicit lab backend selection; this does not select the game's production store.
use std::path::Path;
#[cfg(feature = "paged")]
use voxel_core::paged::{Config, PagedStorage};
use voxel_core::{
    coords::CubePos,
    journal::JournalStorage,
    record::CubeRecord,
    storage::{CubeStorage, StoreError},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Journal,
    #[cfg(feature = "paged")]
    Paged,
}
impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Self::Journal => "immutable-journal-v1",
            #[cfg(feature = "paged")]
            Self::Paged => "redb-4.4.0-paged-v1",
        }
    }
    pub fn from_name(name: &str) -> Result<Self, StoreError> {
        match name {
            "immutable-journal-v1" => Ok(Self::Journal),
            #[cfg(feature = "paged")]
            "redb-4.4.0-paged-v1" => Ok(Self::Paged),
            _ => Err(StoreError::Backend(
                "unsupported lab storage backend".into(),
            )),
        }
    }
}
pub enum WorldStore {
    Journal(JournalStorage),
    #[cfg(feature = "paged")]
    Paged(PagedStorage),
}
impl WorldStore {
    pub fn open(root: &Path, backend: Backend) -> Result<Self, StoreError> {
        Ok(match backend {
            Backend::Journal => Self::Journal(JournalStorage::open(root)?),
            #[cfg(feature = "paged")]
            Backend::Paged => Self::Paged(PagedStorage::open(root, Config::default())?),
        })
    }
    pub fn sequence(&self) -> Result<u64, StoreError> {
        match self {
            Self::Journal(s) => Ok(s.sequence()),
            #[cfg(feature = "paged")]
            Self::Paged(s) => s.sequence(),
        }
    }
}
impl CubeStorage for WorldStore {
    fn load(&mut self, pos: CubePos) -> Result<Option<CubeRecord>, StoreError> {
        match self {
            Self::Journal(s) => s.load(pos),
            #[cfg(feature = "paged")]
            Self::Paged(s) => s.load(pos),
        }
    }
    fn save(&mut self, record: &CubeRecord) -> Result<(), StoreError> {
        self.save_batch(&[record])
    }
    fn save_batch(&mut self, records: &[&CubeRecord]) -> Result<(), StoreError> {
        match self {
            Self::Journal(s) => s.save_batch(records),
            #[cfg(feature = "paged")]
            Self::Paged(s) => s.save_batch(records),
        }
    }
}
