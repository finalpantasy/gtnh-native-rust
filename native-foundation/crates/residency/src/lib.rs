//! Budgeted asset admission. Backends own allocations and acknowledge their retirement.
//! Catalog/map overhead is separate from the CPU/GPU payload budgets.

use std::{collections::BTreeMap, fmt};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    pub id: AssetId,
    pub cpu_bytes: u64,
    pub gpu_bytes: u64,
    pub transient_cpu_bytes: u64,
}
impl Spec {
    pub fn loading_cpu(self) -> Option<u64> {
        self.cpu_bytes.checked_add(self.transient_cpu_bytes)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub cpu_bytes: u64,
    pub gpu_bytes: u64,
    pub max_entries: usize,
    pub max_loading: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Background,
    World,
    Visible,
    Critical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Handle {
    pub id: AssetId,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadToken(pub Handle);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetireToken(pub Handle);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deferred {
    Loading,
    Retiring,
    Capacity,
    Pinned,
    LoadingLimit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Ready(Handle),
    Load(LoadToken),
    Deferred {
        reason: Deferred,
        retire: Vec<RetireToken>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Published {
    Ready(Handle),
    Retire(RetireToken),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheError {
    InvalidBudget,
    InvalidSpec,
    Duplicate,
    UnknownAsset,
    TooLarge,
    Stale,
    WrongSize,
    SequenceExhausted,
    PinExhausted,
    Pinned,
}
impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for CacheError {}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub resident_cpu: u64,
    pub resident_gpu: u64,
    pub loading_cpu: u64,
    pub loading_gpu: u64,
    pub retiring_cpu: u64,
    pub retiring_gpu: u64,
    pub ready: usize,
    pub loading: usize,
    pub retiring: usize,
}
impl Usage {
    pub fn cpu(self) -> u64 {
        self.resident_cpu + self.loading_cpu + self.retiring_cpu
    }
    pub fn gpu(self) -> u64 {
        self.resident_gpu + self.loading_gpu + self.retiring_gpu
    }
    pub fn entries(self) -> usize {
        self.ready + self.loading + self.retiring
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub requests: u64,
    pub hits: u64,
    pub admitted: u64,
    pub evicted: u64,
    pub canceled: u64,
    pub failures: u64,
    pub deferred: u64,
    pub peak_cpu: u64,
    pub peak_gpu: u64,
    pub peak_entries: usize,
}

#[derive(Clone, Copy, Debug)]
enum Phase {
    Loading { canceled: bool },
    Ready,
    Retiring { cpu: u64, gpu: u64 },
}
struct Entry {
    spec: Spec,
    handle: Handle,
    priority: Priority,
    touched: u64,
    pins: u32,
    phase: Phase,
}

pub struct Cache {
    catalog: BTreeMap<AssetId, Spec>,
    entries: BTreeMap<AssetId, Entry>,
    budget: Budget,
    sequence: u64,
    stats: Stats,
}

impl Cache {
    pub fn new(budget: Budget) -> Result<Self, CacheError> {
        if budget.max_entries == 0
            || budget.max_entries > 1_000_000
            || budget.max_loading == 0
            || budget.max_loading > budget.max_entries
        {
            return Err(CacheError::InvalidBudget);
        }
        Ok(Self {
            catalog: BTreeMap::new(),
            entries: BTreeMap::new(),
            budget,
            sequence: 0,
            stats: Stats::default(),
        })
    }
    pub fn register(&mut self, spec: Spec) -> Result<(), CacheError> {
        if spec.loading_cpu().is_none()
            || (spec.cpu_bytes == 0 && spec.gpu_bytes == 0 && spec.transient_cpu_bytes == 0)
            || self.catalog.len() >= 1_000_000
        {
            return Err(CacheError::InvalidSpec);
        }
        if self.catalog.contains_key(&spec.id) {
            return Err(CacheError::Duplicate);
        }
        self.catalog.insert(spec.id, spec);
        Ok(())
    }
    pub fn spec(&self, id: AssetId) -> Option<Spec> {
        self.catalog.get(&id).copied()
    }
    pub fn stats(&self) -> Stats {
        self.stats
    }
    pub fn budget(&self) -> Budget {
        self.budget
    }
    pub fn usage(&self) -> Usage {
        let mut usage = Usage::default();
        for entry in self.entries.values() {
            match entry.phase {
                Phase::Ready => {
                    usage.resident_cpu += entry.spec.cpu_bytes;
                    usage.resident_gpu += entry.spec.gpu_bytes;
                    usage.ready += 1;
                }
                Phase::Loading { .. } => {
                    usage.loading_cpu += entry.spec.loading_cpu().unwrap();
                    usage.loading_gpu += entry.spec.gpu_bytes;
                    usage.loading += 1;
                }
                Phase::Retiring { cpu, gpu } => {
                    usage.retiring_cpu += cpu;
                    usage.retiring_gpu += gpu;
                    usage.retiring += 1;
                }
            }
        }
        usage
    }
    pub fn request(&mut self, id: AssetId, priority: Priority) -> Result<Request, CacheError> {
        self.stats.requests += 1;
        let spec = self.spec(id).ok_or(CacheError::UnknownAsset)?;
        let cpu = spec.loading_cpu().unwrap();
        if cpu > self.budget.cpu_bytes || spec.gpu_bytes > self.budget.gpu_bytes {
            return Err(CacheError::TooLarge);
        }
        if let Some(entry) = self.entries.get(&id) {
            match entry.phase {
                Phase::Ready => {
                    let handle = entry.handle;
                    self.touch(handle, priority)?;
                    self.stats.hits += 1;
                    return Ok(Request::Ready(handle));
                }
                Phase::Loading { canceled } => {
                    return Ok(self.defer(
                        if canceled {
                            Deferred::Retiring
                        } else {
                            Deferred::Loading
                        },
                        Vec::new(),
                    ));
                }
                Phase::Retiring { .. } => return Ok(self.defer(Deferred::Retiring, Vec::new())),
            }
        }
        let usage = self.usage();
        if usage.loading >= self.budget.max_loading {
            return Ok(self.defer(Deferred::LoadingLimit, Vec::new()));
        }
        if self.fits(usage.cpu(), usage.gpu(), usage.entries(), spec) {
            let generation = self.stamp()?;
            let handle = Handle { id, generation };
            self.entries.insert(
                id,
                Entry {
                    spec,
                    handle,
                    priority,
                    touched: generation,
                    pins: 0,
                    phase: Phase::Loading { canceled: false },
                },
            );
            self.stats.admitted += 1;
            self.observe();
            return Ok(Request::Load(LoadToken(handle)));
        }
        let mut projected_cpu = usage.cpu() - usage.retiring_cpu;
        let mut projected_gpu = usage.gpu() - usage.retiring_gpu;
        let mut projected_entries = usage.entries() - usage.retiring;
        if self.fits(projected_cpu, projected_gpu, projected_entries, spec) {
            return Ok(self.defer(Deferred::Retiring, Vec::new()));
        }
        let mut candidates: Vec<_> = self
            .entries
            .values()
            .filter(|e| matches!(e.phase, Phase::Ready) && e.pins == 0 && e.priority <= priority)
            .map(|e| (e.priority, e.touched, e.handle.id))
            .collect();
        candidates.sort();
        let mut chosen = Vec::new();
        for (_, _, victim) in candidates {
            let entry = &self.entries[&victim];
            projected_cpu -= entry.spec.cpu_bytes;
            projected_gpu -= entry.spec.gpu_bytes;
            projected_entries -= 1;
            chosen.push(victim);
            if self.fits(projected_cpu, projected_gpu, projected_entries, spec) {
                break;
            }
        }
        if !self.fits(projected_cpu, projected_gpu, projected_entries, spec) {
            let reason = if self.entries.values().any(|e| e.pins > 0) {
                Deferred::Pinned
            } else {
                Deferred::Capacity
            };
            return Ok(self.defer(reason, Vec::new()));
        }
        let mut retire = Vec::new();
        for victim in chosen {
            let entry = self.entries.get_mut(&victim).unwrap();
            entry.phase = Phase::Retiring {
                cpu: entry.spec.cpu_bytes,
                gpu: entry.spec.gpu_bytes,
            };
            retire.push(RetireToken(entry.handle));
            self.stats.evicted += 1;
        }
        self.observe();
        Ok(self.defer(Deferred::Retiring, retire))
    }
    /// Call only after the backend has finished uploading and freed transient CPU staging.
    pub fn publish(
        &mut self,
        token: LoadToken,
        cpu: u64,
        gpu: u64,
    ) -> Result<Published, CacheError> {
        let entry = self
            .entries
            .get_mut(&token.0.id)
            .filter(|e| e.handle == token.0)
            .ok_or(CacheError::Stale)?;
        let Phase::Loading { canceled } = entry.phase else {
            return Err(CacheError::Stale);
        };
        if cpu != entry.spec.cpu_bytes || gpu != entry.spec.gpu_bytes {
            return Err(CacheError::WrongSize);
        }
        let outcome = if canceled {
            entry.phase = Phase::Retiring {
                cpu: entry.spec.loading_cpu().unwrap(),
                gpu: entry.spec.gpu_bytes,
            };
            Published::Retire(RetireToken(entry.handle))
        } else {
            entry.phase = Phase::Ready;
            Published::Ready(entry.handle)
        };
        self.observe();
        Ok(outcome)
    }
    pub fn cancel(&mut self, token: LoadToken) -> Result<(), CacheError> {
        let entry = self
            .entries
            .get_mut(&token.0.id)
            .filter(|e| e.handle == token.0)
            .ok_or(CacheError::Stale)?;
        let Phase::Loading { canceled } = &mut entry.phase else {
            return Err(CacheError::Stale);
        };
        if !*canceled {
            self.stats.canceled += 1;
            *canceled = true;
        }
        Ok(())
    }
    /// The backend must drop any allocations, wait for GPU completion, then acknowledge.
    pub fn fail(&mut self, token: LoadToken) -> Result<RetireToken, CacheError> {
        let entry = self
            .entries
            .get_mut(&token.0.id)
            .filter(|e| e.handle == token.0)
            .ok_or(CacheError::Stale)?;
        if !matches!(entry.phase, Phase::Loading { .. }) {
            return Err(CacheError::Stale);
        }
        entry.phase = Phase::Retiring {
            cpu: entry.spec.loading_cpu().unwrap(),
            gpu: entry.spec.gpu_bytes,
        };
        self.stats.failures += 1;
        Ok(RetireToken(entry.handle))
    }
    pub fn retire(&mut self, handle: Handle) -> Result<RetireToken, CacheError> {
        let entry = self
            .entries
            .get_mut(&handle.id)
            .filter(|e| e.handle == handle && matches!(e.phase, Phase::Ready))
            .ok_or(CacheError::Stale)?;
        if entry.pins > 0 {
            return Err(CacheError::Pinned);
        }
        entry.phase = Phase::Retiring {
            cpu: entry.spec.cpu_bytes,
            gpu: entry.spec.gpu_bytes,
        };
        self.stats.evicted += 1;
        Ok(RetireToken(handle))
    }
    pub fn acknowledge_retired(&mut self, token: RetireToken) -> bool {
        let valid = self
            .entries
            .get(&token.0.id)
            .is_some_and(|e| e.handle == token.0 && matches!(e.phase, Phase::Retiring { .. }));
        if valid {
            self.entries.remove(&token.0.id);
        }
        valid
    }
    pub fn pin(&mut self, handle: Handle) -> Result<(), CacheError> {
        let entry = self.ready_mut(handle)?;
        entry.pins = entry.pins.checked_add(1).ok_or(CacheError::PinExhausted)?;
        Ok(())
    }
    pub fn unpin(&mut self, handle: Handle) -> Result<(), CacheError> {
        let entry = self.ready_mut(handle)?;
        entry.pins = entry.pins.checked_sub(1).ok_or(CacheError::Stale)?;
        Ok(())
    }
    pub fn touch(&mut self, handle: Handle, priority: Priority) -> Result<(), CacheError> {
        if !self.is_ready(handle) {
            return Err(CacheError::Stale);
        }
        let stamp = self.stamp()?;
        let entry = self.entries.get_mut(&handle.id).unwrap();
        entry.touched = stamp;
        entry.priority = priority;
        Ok(())
    }
    pub fn is_ready(&self, handle: Handle) -> bool {
        self.entries
            .get(&handle.id)
            .is_some_and(|e| e.handle == handle && matches!(e.phase, Phase::Ready))
    }
    pub fn ready_handles(&self) -> impl Iterator<Item = Handle> + '_ {
        self.entries
            .values()
            .filter(|e| matches!(e.phase, Phase::Ready))
            .map(|e| e.handle)
    }
    fn ready_mut(&mut self, handle: Handle) -> Result<&mut Entry, CacheError> {
        self.entries
            .get_mut(&handle.id)
            .filter(|e| e.handle == handle && matches!(e.phase, Phase::Ready))
            .ok_or(CacheError::Stale)
    }
    fn stamp(&mut self) -> Result<u64, CacheError> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(CacheError::SequenceExhausted)?;
        Ok(self.sequence)
    }
    fn fits(&self, cpu: u64, gpu: u64, entries: usize, spec: Spec) -> bool {
        cpu.checked_add(spec.loading_cpu().unwrap())
            .is_some_and(|v| v <= self.budget.cpu_bytes)
            && gpu
                .checked_add(spec.gpu_bytes)
                .is_some_and(|v| v <= self.budget.gpu_bytes)
            && entries < self.budget.max_entries
    }
    fn defer(&mut self, reason: Deferred, retire: Vec<RetireToken>) -> Request {
        self.stats.deferred += 1;
        Request::Deferred { reason, retire }
    }
    fn observe(&mut self) {
        let usage = self.usage();
        debug_assert!(
            usage.cpu() <= self.budget.cpu_bytes
                && usage.gpu() <= self.budget.gpu_bytes
                && usage.entries() <= self.budget.max_entries
        );
        self.stats.peak_cpu = self.stats.peak_cpu.max(usage.cpu());
        self.stats.peak_gpu = self.stats.peak_gpu.max(usage.gpu());
        self.stats.peak_entries = self.stats.peak_entries.max(usage.entries());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(cpu: u64, gpu: u64) -> Cache {
        Cache::new(Budget {
            cpu_bytes: cpu,
            gpu_bytes: gpu,
            max_entries: 8,
            max_loading: 4,
        })
        .unwrap()
    }
    fn spec(id: u64, cpu: u64, gpu: u64, transient: u64) -> Spec {
        Spec {
            id: AssetId(id),
            cpu_bytes: cpu,
            gpu_bytes: gpu,
            transient_cpu_bytes: transient,
        }
    }
    fn load(cache: &mut Cache, id: u64, priority: Priority) -> LoadToken {
        match cache.request(AssetId(id), priority).unwrap() {
            Request::Load(token) => token,
            other => panic!("expected load: {other:?}"),
        }
    }
    fn ready(cache: &mut Cache, id: u64, priority: Priority) -> Handle {
        let token = load(cache, id, priority);
        let spec = cache.spec(AssetId(id)).unwrap();
        match cache
            .publish(token, spec.cpu_bytes, spec.gpu_bytes)
            .unwrap()
        {
            Published::Ready(h) => h,
            _ => panic!("unexpected retirement"),
        }
    }

    #[test]
    fn pending_work_reserves_cpu_gpu_and_transient_bytes() {
        let mut c = cache(100, 100);
        c.register(spec(1, 60, 60, 20)).unwrap();
        c.register(spec(2, 60, 60, 20)).unwrap();
        let token = load(&mut c, 1, Priority::Visible);
        assert_eq!(
            (c.usage().cpu(), c.usage().gpu(), c.usage().loading),
            (80, 60, 1)
        );
        assert!(matches!(
            c.request(AssetId(2), Priority::Visible).unwrap(),
            Request::Deferred { .. }
        ));
        assert_eq!(c.usage().loading, 1);
        c.publish(token, 60, 60).unwrap();
        assert_eq!((c.usage().cpu(), c.usage().gpu()), (60, 60));
    }

    #[test]
    fn eviction_does_not_free_budget_until_backend_acknowledges() {
        let mut c = cache(100, 100);
        for id in [1, 2] {
            c.register(spec(id, 60, 60, 20)).unwrap();
        }
        let first = ready(&mut c, 1, Priority::World);
        let Request::Deferred { retire, .. } = c.request(AssetId(2), Priority::Visible).unwrap()
        else {
            panic!("must wait for retirement")
        };
        assert_eq!(retire, vec![RetireToken(first)]);
        assert_eq!((c.usage().cpu(), c.usage().gpu()), (60, 60));
        assert_eq!(c.usage().retiring, 1);
        assert!(
            matches!(c.request(AssetId(2),Priority::Visible).unwrap(),Request::Deferred{retire,..} if retire.is_empty())
        );
        assert!(c.acknowledge_retired(retire[0]));
        assert!(!c.acknowledge_retired(retire[0]));
        load(&mut c, 2, Priority::Visible);
        assert_eq!(c.usage().cpu(), 80);
    }

    #[test]
    fn pinned_assets_block_eviction_and_old_handles_never_pin_replacements() {
        let mut c = cache(60, 60);
        for id in [1, 2] {
            c.register(spec(id, 60, 60, 0)).unwrap();
        }
        let first = ready(&mut c, 1, Priority::World);
        c.pin(first).unwrap();
        assert!(
            matches!(c.request(AssetId(2),Priority::Critical).unwrap(),Request::Deferred{reason:Deferred::Pinned,ref retire} if retire.is_empty())
        );
        assert_eq!(c.usage().ready, 1);
        c.unpin(first).unwrap();
        let Request::Deferred { retire, .. } = c.request(AssetId(2), Priority::Visible).unwrap()
        else {
            panic!()
        };
        c.acknowledge_retired(retire[0]);
        let second = ready(&mut c, 2, Priority::Visible);
        assert_eq!(c.pin(first), Err(CacheError::Stale));
        c.pin(second).unwrap();
    }

    #[test]
    fn canceled_load_keeps_its_reservation_and_late_completion_is_retired() {
        let mut c = cache(100, 100);
        c.register(spec(1, 50, 50, 10)).unwrap();
        let old = load(&mut c, 1, Priority::World);
        c.cancel(old).unwrap();
        assert_eq!(c.usage().cpu(), 60);
        assert!(matches!(
            c.request(AssetId(1), Priority::World).unwrap(),
            Request::Deferred { .. }
        ));
        assert_eq!(
            c.publish(old, 50, 50).unwrap(),
            Published::Retire(RetireToken(old.0))
        );
        assert_eq!(c.usage().cpu(), 60);
        assert!(c.acknowledge_retired(RetireToken(old.0)));
        let new = load(&mut c, 1, Priority::Visible);
        assert_ne!(old, new);
        assert_eq!(c.publish(old, 50, 50), Err(CacheError::Stale));
        assert_eq!(c.usage().loading, 1);
    }

    #[test]
    fn oversized_assets_cannot_purge_existing_healthy_assets() {
        let mut c = cache(100, 100);
        c.register(spec(1, 30, 30, 0)).unwrap();
        c.register(spec(2, 101, 20, 0)).unwrap();
        let first = ready(&mut c, 1, Priority::World);
        let before = c.usage();
        assert_eq!(
            c.request(AssetId(2), Priority::Critical),
            Err(CacheError::TooLarge)
        );
        assert_eq!(c.usage(), before);
        c.pin(first).unwrap();
    }

    #[test]
    fn priorities_then_lru_choose_the_eviction_victim() {
        let mut c = cache(120, 120);
        for id in 1..=4 {
            c.register(spec(id, 40, 40, 0)).unwrap();
        }
        ready(&mut c, 1, Priority::Critical);
        let second = ready(&mut c, 2, Priority::Background);
        ready(&mut c, 3, Priority::World);
        let Request::Deferred { retire, .. } = c.request(AssetId(4), Priority::Visible).unwrap()
        else {
            panic!()
        };
        assert_eq!(retire, vec![RetireToken(second)]);
        assert_eq!(c.usage().cpu(), 120);
    }

    #[test]
    fn failed_or_wrong_sized_loads_never_become_ready() {
        let mut c = cache(100, 100);
        c.register(spec(1, 50, 50, 10)).unwrap();
        let token = load(&mut c, 1, Priority::World);
        assert_eq!(c.publish(token, 51, 50), Err(CacheError::WrongSize));
        assert_eq!((c.usage().loading, c.usage().ready), (1, 0));
        let retire = c.fail(token).unwrap();
        assert_eq!(c.usage().cpu(), 60);
        c.acknowledge_retired(retire);
        assert_eq!(c.usage().entries(), 0);
    }
}
