//! Bounded, generation-tagged streaming independent in all three axes.

use crate::{
    coords::CubePos,
    record::{CubeRecord, RecordError},
    storage::{CubeStorage, StoreError},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

pub const MAX_TICKETS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub render: bool,
    pub simulate: bool,
    pub distance: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Tickets(BTreeMap<CubePos, Ticket>);

impl Tickets {
    pub fn around(
        center: CubePos,
        render: [u16; 3],
        simulate: [u16; 3],
    ) -> Result<Self, StreamError> {
        let radius = std::array::from_fn::<_, 3, _>(|i| render[i].max(simulate[i]));
        let volume = radius
            .iter()
            .try_fold(1usize, |v, r| v.checked_mul(usize::from(*r) * 2 + 1))
            .ok_or(StreamError::Capacity)?;
        if volume > MAX_TICKETS || center.block_at(0).is_none() {
            return Err(StreamError::Capacity);
        }
        let mut map = BTreeMap::new();
        for y in -i64::from(radius[1])..=i64::from(radius[1]) {
            for z in -i64::from(radius[2])..=i64::from(radius[2]) {
                for x in -i64::from(radius[0])..=i64::from(radius[0]) {
                    let delta = [x, y, z];
                    let Some(pos) = center.offset(delta).filter(|p| p.block_at(0).is_some()) else {
                        continue;
                    };
                    let inside = |r: [u16; 3]| {
                        delta
                            .iter()
                            .zip(r)
                            .all(|(d, r)| d.unsigned_abs() <= u64::from(r))
                    };
                    let ticket = Ticket {
                        render: inside(render),
                        simulate: inside(simulate),
                        distance: delta.iter().map(|v| v.unsigned_abs()).max().unwrap(),
                    };
                    if ticket.render || ticket.simulate {
                        map.insert(pos, ticket);
                    }
                }
            }
        }
        Ok(Self(map))
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn get(&self, pos: CubePos) -> Option<Ticket> {
        self.0.get(&pos).copied()
    }
    pub fn positions(&self) -> impl Iterator<Item = CubePos> + '_ {
        self.0.keys().copied()
    }
    fn preferred(&self, count: usize) -> Vec<CubePos> {
        let mut rows: Vec<_> = self.0.iter().collect();
        rows.sort_by_key(|(pos, t)| (!t.simulate, t.distance, **pos));
        rows.into_iter().take(count).map(|(p, _)| *p).collect()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_cubes: usize,
    pub max_resident_bytes: usize,
    pub max_cube_bytes: usize,
    pub max_pending_jobs: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadJob {
    pub id: u64,
    pub pos: CubePos,
}

#[derive(Debug)]
pub enum StreamError {
    Capacity,
    Missing,
    InvalidJob,
    PinExhausted,
    SequenceExhausted,
    Store(StoreError),
    Record(RecordError),
}
impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for StreamError {}
impl From<StoreError> for StreamError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}
impl From<RecordError> for StreamError {
    fn from(e: RecordError) -> Self {
        Self::Record(e)
    }
}

struct Resident {
    cube: CubeRecord,
    dirty: bool,
    pins: u32,
}
struct Pending {
    job: LoadJob,
    active: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub loaded: u64,
    pub unloaded: u64,
    pub canceled: u64,
    pub discarded: u64,
    pub save_failures: u64,
    pub peak_cubes: usize,
    pub peak_bytes: usize,
    pub peak_jobs: usize,
    pub transactions: u64,
    pub transaction_failures: u64,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub loads: Vec<LoadJob>,
    pub unloaded: Vec<CubePos>,
}

pub struct Streamer<S> {
    storage: S,
    limits: Limits,
    tickets: Tickets,
    resident: BTreeMap<CubePos, Resident>,
    pending: BTreeMap<u64, Pending>,
    next_job: u64,
    resident_bytes: usize,
    stats: Stats,
}

impl<S: CubeStorage> Streamer<S> {
    pub fn new(storage: S, limits: Limits) -> Result<Self, StreamError> {
        let minimum = CubeRecord::new(CubePos::new(0, 0, 0))?.resident_bytes();
        if limits.max_cubes == 0
            || limits.max_cubes > MAX_TICKETS
            || limits.max_pending_jobs == 0
            || limits.max_pending_jobs > MAX_TICKETS
            || limits.max_cube_bytes < minimum
            || limits.max_resident_bytes < limits.max_cube_bytes
            || limits.max_cube_bytes > crate::record::MAX_ENCODED + 256
            || limits
                .max_cubes
                .checked_mul(limits.max_cube_bytes)
                .is_none()
            || limits
                .max_pending_jobs
                .checked_mul(limits.max_cube_bytes)
                .is_none()
        {
            return Err(StreamError::Capacity);
        }
        Ok(Self {
            storage,
            limits,
            tickets: Tickets::default(),
            resident: BTreeMap::new(),
            pending: BTreeMap::new(),
            next_job: 0,
            resident_bytes: 0,
            stats: Stats::default(),
        })
    }
    pub fn set_tickets(&mut self, tickets: Tickets) -> Vec<LoadJob> {
        self.tickets = tickets;
        let preferred: BTreeSet<_> = self
            .tickets
            .preferred(self.limits.max_cubes)
            .into_iter()
            .collect();
        let mut canceled = Vec::new();
        for pending in self.pending.values_mut() {
            if pending.active && !preferred.contains(&pending.job.pos) {
                pending.active = false;
                canceled.push(pending.job);
                self.stats.canceled += 1;
            }
        }
        canceled
    }
    pub fn record(&self, pos: CubePos) -> Option<&CubeRecord> {
        self.resident.get(&pos).map(|r| &r.cube)
    }
    pub fn resident_positions(&self) -> impl Iterator<Item = CubePos> + '_ {
        self.resident.keys().copied()
    }
    pub fn pending_jobs(&self) -> usize {
        self.pending.len()
    }
    pub fn resident_bytes(&self) -> usize {
        self.resident_bytes
    }
    pub fn charged_bytes(&self) -> usize {
        self.resident_bytes + self.pending.len() * self.limits.max_cube_bytes
    }
    pub fn stats(&self) -> &Stats {
        &self.stats
    }
    pub fn storage_mut(&mut self) -> &mut S {
        &mut self.storage
    }
    pub fn simulates(&self, pos: CubePos) -> bool {
        self.resident.contains_key(&pos) && self.tickets.get(pos).is_some_and(|t| t.simulate)
    }
    pub fn renders(&self, pos: CubePos) -> bool {
        self.resident.contains_key(&pos) && self.tickets.get(pos).is_some_and(|t| t.render)
    }
    pub fn pin(&mut self, pos: CubePos) -> Result<(), StreamError> {
        let resident = self.resident.get_mut(&pos).ok_or(StreamError::Missing)?;
        resident.pins = resident
            .pins
            .checked_add(1)
            .ok_or(StreamError::PinExhausted)?;
        Ok(())
    }
    pub fn unpin(&mut self, pos: CubePos) -> Result<(), StreamError> {
        let resident = self.resident.get_mut(&pos).ok_or(StreamError::Missing)?;
        resident.pins = resident.pins.checked_sub(1).ok_or(StreamError::Missing)?;
        Ok(())
    }
    pub fn plan(&mut self, loads: usize, evictions: usize) -> Result<Plan, StreamError> {
        let preferred = self.tickets.preferred(self.limits.max_cubes);
        let wanted: BTreeSet<_> = preferred.iter().copied().collect();
        let removable: Vec<_> = self
            .resident
            .iter()
            .filter(|(p, r)| !wanted.contains(p) && r.pins == 0)
            .map(|(p, _)| *p)
            .take(evictions)
            .collect();
        let mut plan = Plan::default();
        for pos in removable {
            let resident = &self.resident[&pos];
            if resident.dirty
                && let Err(error) = self.storage.save(&resident.cube)
            {
                self.stats.save_failures += 1;
                return Err(error.into());
            }
            let removed = self.resident.remove(&pos).unwrap();
            self.resident_bytes -= removed.cube.resident_bytes();
            self.stats.unloaded += 1;
            plan.unloaded.push(pos);
        }
        for pos in preferred {
            if plan.loads.len() >= loads
                || self.pending.len() >= self.limits.max_pending_jobs
                || self.resident.len() + self.pending.len() >= self.limits.max_cubes
            {
                break;
            }
            if self.resident.contains_key(&pos)
                || self.pending.values().any(|p| p.active && p.job.pos == pos)
            {
                continue;
            }
            if self
                .charged_bytes()
                .checked_add(self.limits.max_cube_bytes)
                .is_none_or(|bytes| bytes > self.limits.max_resident_bytes)
            {
                break;
            }
            self.next_job = self
                .next_job
                .checked_add(1)
                .ok_or(StreamError::SequenceExhausted)?;
            let job = LoadJob {
                id: self.next_job,
                pos,
            };
            self.pending.insert(job.id, Pending { job, active: true });
            plan.loads.push(job);
        }
        self.observe();
        Ok(plan)
    }
    pub fn complete(&mut self, job: LoadJob, record: CubeRecord) -> Result<bool, StreamError> {
        if !self.pending.get(&job.id).is_some_and(|p| p.job == job) {
            self.stats.discarded += 1;
            return Err(StreamError::InvalidJob);
        }
        let pending = self.pending.remove(&job.id).unwrap();
        if !pending.active {
            self.stats.discarded += 1;
            return Ok(false);
        }
        let bytes = record.resident_bytes();
        if record.pos() != job.pos || self.resident.contains_key(&job.pos) {
            self.stats.discarded += 1;
            return Err(StreamError::InvalidJob);
        }
        if bytes > self.limits.max_cube_bytes
            || self.charged_bytes() + bytes > self.limits.max_resident_bytes
        {
            self.stats.discarded += 1;
            return Err(StreamError::Capacity);
        }
        self.resident_bytes += bytes;
        self.resident.insert(
            job.pos,
            Resident {
                cube: record,
                dirty: false,
                pins: 0,
            },
        );
        self.stats.loaded += 1;
        self.observe();
        Ok(true)
    }
    pub fn acknowledge_canceled(&mut self, job: LoadJob) -> bool {
        let matches = self
            .pending
            .get(&job.id)
            .is_some_and(|p| p.job == job && !p.active);
        if matches {
            self.pending.remove(&job.id);
        }
        matches
    }
    pub fn read_or_generate(
        &mut self,
        job: LoadJob,
        generate: impl FnOnce(CubePos) -> CubeRecord,
    ) -> Result<CubeRecord, StreamError> {
        if !self
            .pending
            .get(&job.id)
            .is_some_and(|p| p.job == job && p.active)
        {
            return Err(StreamError::InvalidJob);
        }
        Ok(self
            .storage
            .load(job.pos)?
            .unwrap_or_else(|| generate(job.pos)))
    }
    pub fn edit(
        &mut self,
        pos: CubePos,
        edit: impl FnOnce(&mut CubeRecord) -> Result<(), RecordError>,
    ) -> Result<(), StreamError> {
        let current = self.resident.get(&pos).ok_or(StreamError::Missing)?;
        let mut candidate = current.cube.clone();
        edit(&mut candidate)?;
        if candidate.pos() != pos
            || (candidate != current.cube && candidate.revision() <= current.cube.revision())
        {
            return Err(RecordError::Invalid("nonmonotonic or misaddressed edit").into());
        }
        let bytes = candidate.resident_bytes();
        if bytes > self.limits.max_cube_bytes
            || self.charged_bytes() - current.cube.resident_bytes() + bytes
                > self.limits.max_resident_bytes
        {
            return Err(StreamError::Capacity);
        }
        let resident = self.resident.get_mut(&pos).unwrap();
        self.resident_bytes = self.resident_bytes - resident.cube.resident_bytes() + bytes;
        resident.dirty |= candidate.revision() != resident.cube.revision();
        resident.cube = candidate;
        self.observe();
        Ok(())
    }
    /// Persist all changed participants atomically before exposing the in-memory result.
    pub fn transact<T>(
        &mut self,
        positions: &[CubePos],
        edit: impl FnOnce(&mut BTreeMap<CubePos, CubeRecord>) -> Result<T, RecordError>,
    ) -> Result<T, StreamError> {
        if positions.is_empty() || positions.len() > self.limits.max_cubes {
            return Err(RecordError::Invalid("transaction size").into());
        }
        let keys: BTreeSet<_> = positions.iter().copied().collect();
        if keys.len() != positions.len() {
            return Err(RecordError::Invalid("duplicate transaction members").into());
        }
        let mut candidates = BTreeMap::new();
        let mut before_bytes = 0usize;
        for pos in &keys {
            let resident = self.resident.get(pos).ok_or(StreamError::Missing)?;
            before_bytes += resident.cube.resident_bytes();
            candidates.insert(*pos, resident.cube.clone());
        }
        let value = edit(&mut candidates)?;
        if candidates.len() != keys.len() || !candidates.keys().copied().eq(keys.iter().copied()) {
            return Err(RecordError::Invalid("transaction members changed").into());
        }
        let mut after_bytes = 0usize;
        let mut changed = false;
        for (pos, candidate) in &candidates {
            let old = &self.resident[pos].cube;
            if candidate.pos() != *pos
                || (candidate != old && candidate.revision() <= old.revision())
            {
                return Err(
                    RecordError::Invalid("nonmonotonic or misaddressed transaction").into(),
                );
            }
            if candidate.resident_bytes() > self.limits.max_cube_bytes {
                return Err(StreamError::Capacity);
            }
            after_bytes += candidate.resident_bytes();
            changed |= candidate != old;
        }
        if self.charged_bytes() - before_bytes + after_bytes > self.limits.max_resident_bytes {
            return Err(StreamError::Capacity);
        }
        if !changed {
            return Ok(value);
        }
        let references: Vec<_> = candidates.values().collect();
        if let Err(error) = self.storage.save_batch(&references) {
            self.stats.transaction_failures += 1;
            return Err(error.into());
        }
        self.resident_bytes = self.resident_bytes - before_bytes + after_bytes;
        for (pos, cube) in candidates {
            let resident = self.resident.get_mut(&pos).unwrap();
            resident.cube = cube;
            resident.dirty = false;
        }
        self.stats.transactions += 1;
        self.observe();
        Ok(value)
    }
    pub fn flush_all(&mut self) -> Result<usize, StreamError> {
        let records: Vec<_> = self
            .resident
            .values()
            .filter(|r| r.dirty)
            .map(|r| &r.cube)
            .collect();
        let count = records.len();
        if count == 0 {
            return Ok(0);
        }
        self.storage.save_batch(&records)?;
        for resident in self.resident.values_mut() {
            resident.dirty = false;
        }
        Ok(count)
    }
    fn observe(&mut self) {
        self.stats.peak_cubes = self.stats.peak_cubes.max(self.resident.len());
        self.stats.peak_bytes = self.stats.peak_bytes.max(self.charged_bytes());
        self.stats.peak_jobs = self.stats.peak_jobs.max(self.pending.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MemoryStorage;

    fn streamer(cubes: usize) -> Streamer<MemoryStorage> {
        Streamer::new(
            MemoryStorage::default(),
            Limits {
                max_cubes: cubes,
                max_resident_bytes: cubes * 32768,
                max_cube_bytes: 32768,
                max_pending_jobs: 4,
            },
        )
        .unwrap()
    }
    fn ticket(pos: CubePos) -> Tickets {
        Tickets::around(pos, [0; 3], [0; 3]).unwrap()
    }
    fn fill(stream: &mut Streamer<MemoryStorage>) {
        let jobs = stream.plan(99, 99).unwrap().loads;
        for job in jobs {
            let record = stream
                .read_or_generate(job, |p| CubeRecord::new(p).unwrap())
                .unwrap();
            assert!(stream.complete(job, record).unwrap());
        }
    }

    #[test]
    fn vertical_only_movement_replaces_cubes_and_preserves_dirty_payload() {
        let low = CubePos::new(-1, -1000, 2);
        let high = CubePos::new(-1, 1_000_000, 2);
        let mut stream = streamer(1);
        stream.set_tickets(ticket(low));
        fill(&mut stream);
        stream
            .edit(low, |c| {
                c.replace_payload(vec![7, 8, 9])?;
                Ok(())
            })
            .unwrap();
        stream.set_tickets(ticket(high));
        fill(&mut stream);
        assert!(stream.record(low).is_none());
        assert!(stream.record(high).is_some());
        stream.set_tickets(ticket(low));
        fill(&mut stream);
        assert_eq!(stream.record(low).unwrap().payload(), &[7, 8, 9]);
        assert_eq!(stream.stats().unloaded, 2);
    }

    #[test]
    fn failed_save_keeps_the_dirty_resident_cube() {
        let pos = CubePos::new(0, 0, 0);
        let mut stream = streamer(1);
        stream.set_tickets(ticket(pos));
        fill(&mut stream);
        stream
            .edit(pos, |c| {
                c.set(0, 42)?;
                Ok(())
            })
            .unwrap();
        stream.storage_mut().reject_writes = true;
        stream.set_tickets(Tickets::default());
        assert!(stream.plan(1, 1).is_err());
        assert_eq!(stream.record(pos).unwrap().blocks()[0], 42);
        stream.storage_mut().reject_writes = false;
        assert_eq!(stream.plan(1, 1).unwrap().unloaded, vec![pos]);
    }

    #[test]
    fn canceled_jobs_keep_their_reservation_until_acknowledged() {
        let mut stream = streamer(1);
        let old = CubePos::new(0, 0, 0);
        let new = CubePos::new(0, 1, 0);
        stream.set_tickets(ticket(old));
        let job = stream.plan(1, 1).unwrap().loads[0];
        assert_eq!(stream.set_tickets(ticket(new)), vec![job]);
        assert!(stream.plan(1, 1).unwrap().loads.is_empty());
        assert!(!stream.complete(job, CubeRecord::new(old).unwrap()).unwrap());
        fill(&mut stream);
        assert!(stream.record(old).is_none() && stream.record(new).is_some());
    }

    #[test]
    fn pinning_blocks_eviction_and_simulation_does_not_follow_pins() {
        let mut stream = streamer(1);
        let old = CubePos::new(0, 0, 0);
        stream.set_tickets(ticket(old));
        fill(&mut stream);
        stream.pin(old).unwrap();
        stream.set_tickets(ticket(CubePos::new(0, 4, 0)));
        assert!(stream.plan(1, 1).unwrap().loads.is_empty());
        assert!(stream.record(old).is_some() && !stream.simulates(old));
        stream.unpin(old).unwrap();
        fill(&mut stream);
        assert!(stream.record(old).is_none());
    }

    #[test]
    fn interest_volume_and_work_per_plan_are_bounded() {
        assert!(Tickets::around(CubePos::new(0, 0, 0), [100; 3], [1; 3]).is_err());
        let tickets = Tickets::around(CubePos::new(0, 0, 0), [2, 1, 2], [0; 3]).unwrap();
        assert_eq!(tickets.len(), 75);
        let mut stream = streamer(20);
        stream.set_tickets(tickets);
        assert_eq!(stream.plan(2, 1).unwrap().loads.len(), 2);
        assert_eq!(stream.plan(99, 1).unwrap().loads.len(), 2);
        assert_eq!(stream.pending_jobs(), 4);
        assert!(stream.charged_bytes() <= 20 * 32768);
    }

    #[test]
    fn durable_transaction_is_all_or_nothing_even_when_storage_rejects_writes() {
        let a = CubePos::new(0, 0, 0);
        let b = CubePos::new(0, 1, 0);
        let mut stream = streamer(3);
        stream.set_tickets(Tickets::around(a, [0, 1, 0], [0, 1, 0]).unwrap());
        fill(&mut stream);
        stream
            .edit(a, |c| {
                c.replace_payload(vec![10])?;
                Ok(())
            })
            .unwrap();
        stream.storage_mut().reject_writes = true;
        let result = stream.transact(&[a, b], |cubes| {
            cubes.get_mut(&a).unwrap().replace_payload(vec![7])?;
            cubes.get_mut(&b).unwrap().replace_payload(vec![3])?;
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(stream.record(a).unwrap().payload(), &[10]);
        assert!(stream.record(b).unwrap().payload().is_empty());
        stream.storage_mut().reject_writes = false;
        stream
            .transact(&[a, b], |cubes| {
                cubes.get_mut(&a).unwrap().replace_payload(vec![7])?;
                cubes.get_mut(&b).unwrap().replace_payload(vec![3])?;
                Ok(())
            })
            .unwrap();
        assert_eq!(stream.record(a).unwrap().payload(), &[7]);
        assert_eq!(stream.record(b).unwrap().payload(), &[3]);
        assert_eq!(
            stream.storage_mut().load(a).unwrap().unwrap().payload(),
            &[7]
        );
        assert_eq!(
            stream.storage_mut().load(b).unwrap().unwrap().payload(),
            &[3]
        );
    }

    #[test]
    fn transaction_cannot_invent_members_or_rewind_a_record_revision() {
        let a = CubePos::new(0, 0, 0);
        let mut stream = streamer(1);
        stream.set_tickets(ticket(a));
        fill(&mut stream);
        stream
            .edit(a, |c| {
                c.set(0, 99)?;
                Ok(())
            })
            .unwrap();
        let before = stream.record(a).unwrap().clone();
        assert!(
            stream
                .transact(&[a], |cubes| {
                    cubes.insert(a, CubeRecord::new(a)?);
                    Ok(())
                })
                .is_err()
        );
        assert_eq!(stream.record(a), Some(&before));
        assert!(
            stream
                .transact(&[a], |cubes| {
                    cubes.clear();
                    Ok(())
                })
                .is_err()
        );
        assert_eq!(stream.record(a), Some(&before));
    }
}
