//! Bounded single-player requirement counts, not whole-task/party/reward parity.
use crate::item_match::{MatchError, Stack, compare_stacks};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt,
    io::{self, Write},
};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    pub stack: Stack,
    pub quantity: u32,
    pub damageable: bool,
    pub ore_alias: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub key: String,
    pub requirements: Vec<Requirement>,
    pub consume: bool,
    pub ignore_nbt: bool,
    pub partial: bool,
    pub only_one: bool,
    pub group_detect: bool,
    pub auto_consume: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slot {
    pub stack: Stack,
    pub quantity: u32,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    schema_version: u32,
    definition_sha256: String,
    progress: Vec<u32>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    pub satisfied: bool,
    pub changed: bool,
    pub consumed: u64,
    pub attempts: u32,
    pub operations: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidDefinition,
    UnsupportedDefinition,
    InvalidInventory,
    InvalidProgress,
    ChangedDefinition,
    WrongMode,
    EncodedLimit,
    InvalidEncoding,
    Match(MatchError),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
impl From<MatchError> for Error {
    fn from(e: MatchError) -> Self {
        Self::Match(e)
    }
}
struct Sink {
    bytes: usize,
    hash: Sha256,
}
impl Sink {
    fn new(prefix: &[u8]) -> Self {
        let mut hash = Sha256::new();
        hash.update(prefix);
        Self { bytes: 0, hash }
    }
}
impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > 1024 * 1024 - self.bytes {
            return Err(io::Error::other("retrieval encoding limit"));
        }
        self.bytes += bytes.len();
        self.hash.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Definition {
    pub fn validate(&self) -> Result<(), Error> {
        if self.key.is_empty()
            || self.key.len() > 256
            || !self.key.bytes().all(|b| b.is_ascii_graphic())
            || self.requirements.is_empty()
            || self.requirements.len() > 64
        {
            return Err(Error::InvalidDefinition);
        }
        if self.group_detect
            || self.auto_consume
            || self.requirements.iter().any(|r| !r.ore_alias.is_empty())
        {
            return Err(Error::UnsupportedDefinition);
        }
        let mut properties = BTreeMap::new();
        for req in &self.requirements {
            if !(1..=i32::MAX as u32).contains(&req.quantity) {
                return Err(Error::InvalidDefinition);
            }
            req.stack.validate()?;
            if let Some(existing) = properties.insert(&req.stack.registry, req.damageable)
                && existing != req.damageable
            {
                return Err(Error::InvalidDefinition);
            }
        }
        Ok(())
    }
    pub fn fingerprint(&self) -> Result<String, Error> {
        self.validate()?;
        let mut sink = Sink::new(b"retrieval-bq-3.8.92-v1");
        serde_json::to_writer(&mut sink, self).map_err(|_| Error::EncodedLimit)?;
        Ok(format!("{:x}", sink.hash.finalize()))
    }
    fn fulfilled(&self, progress: &[u32]) -> bool {
        let mut rows = self
            .requirements
            .iter()
            .zip(progress)
            .map(|(r, &p)| p >= r.quantity);
        if self.only_one {
            rows.any(|full| full)
        } else {
            rows.all(|full| full)
        }
    }
}
pub fn validate_slots(slots: &[Option<Slot>]) -> Result<(), Error> {
    if slots.len() > 128 {
        return Err(Error::InvalidInventory);
    }
    for slot in slots.iter().flatten() {
        if !(1..=i32::MAX as u32).contains(&slot.quantity) {
            return Err(Error::InvalidInventory);
        }
        slot.stack.validate()?;
    }
    serde_json::to_writer(Sink::new(b"retrieval-slots"), slots).map_err(|_| Error::EncodedLimit)?;
    Ok(())
}
struct Plan {
    next: Progress,
    counts: Vec<u32>,
    report: Report,
}
impl Progress {
    pub fn new(definition: &Definition) -> Result<Self, Error> {
        Ok(Self {
            schema_version: 1,
            definition_sha256: definition.fingerprint()?,
            progress: vec![0; definition.requirements.len()],
        })
    }
    pub fn values(&self) -> &[u32] {
        &self.progress
    }
    pub fn validate(&self, definition: &Definition) -> Result<(), Error> {
        if self.definition_sha256 != definition.fingerprint()? {
            return Err(Error::ChangedDefinition);
        }
        if self.schema_version != 1
            || self.progress.len() != definition.requirements.len()
            || self
                .progress
                .iter()
                .zip(&definition.requirements)
                .any(|(&p, r)| p > r.quantity)
        {
            return Err(Error::InvalidProgress);
        }
        Ok(())
    }
    pub fn satisfied(&self, definition: &Definition) -> Result<bool, Error> {
        self.validate(definition)?;
        Ok(definition.fulfilled(&self.progress))
    }
    fn plan(
        &self,
        definition: &Definition,
        slots: &[Option<Slot>],
        budget: u32,
    ) -> Result<Plan, Error> {
        if !(1..=1_000_000).contains(&budget) {
            return Err(MatchError::InvalidBudget.into());
        }
        self.validate(definition)?;
        validate_slots(slots)?;
        let mut next = self.clone();
        let mut counts = slots
            .iter()
            .map(|s| s.as_ref().map_or(0, |s| s.quantity))
            .collect::<Vec<_>>();
        let mut report = Report {
            satisfied: definition.fulfilled(&self.progress),
            ..Report::default()
        };
        if report.satisfied {
            return Ok(Plan {
                next,
                counts,
                report,
            });
        }
        if !definition.consume {
            for (progress, req) in next.progress.iter_mut().zip(&definition.requirements) {
                if *progress < req.quantity {
                    *progress = 0;
                }
            }
        }
        let required = definition
            .requirements
            .iter()
            .map(|r| r.stack.view())
            .collect::<Result<Vec<_>, _>>()?;
        let samples = slots
            .iter()
            .map(|s| s.as_ref().map(|s| s.stack.view()).transpose())
            .collect::<Result<Vec<_>, _>>()?;
        'slots: for (slot, sample) in samples.into_iter().enumerate() {
            let Some(sample) = sample else {
                continue;
            };
            let mut remaining = counts[slot];
            for (index, req) in definition.requirements.iter().enumerate() {
                if remaining == 0 {
                    break;
                }
                if next.progress[index] >= req.quantity {
                    continue;
                }
                report.attempts += 1;
                report.operations += 1;
                let allowance = budget
                    .checked_sub(report.operations)
                    .filter(|&v| v > 0)
                    .ok_or(MatchError::QueryLimit)?;
                let compared = compare_stacks(
                    Some(required[index]),
                    Some(sample),
                    req.damageable,
                    !definition.ignore_nbt,
                    definition.partial,
                    allowance,
                )?;
                report.operations += compared.operations;
                if !compared.matches {
                    continue;
                }
                let moved = (req.quantity - next.progress[index]).min(remaining);
                next.progress[index] += moved;
                remaining -= moved;
                if definition.consume {
                    counts[slot] -= moved;
                    report.consumed += u64::from(moved);
                }
                if definition.only_one && next.progress[index] >= req.quantity {
                    break 'slots;
                }
            }
        }
        report.satisfied = definition.fulfilled(&next.progress);
        report.changed = next != *self;
        Ok(Plan {
            next,
            counts,
            report,
        })
    }
    pub fn detect(
        &mut self,
        definition: &Definition,
        slots: &[Option<Slot>],
        budget: u32,
    ) -> Result<Report, Error> {
        if definition.consume {
            return Err(Error::WrongMode);
        }
        let plan = self.plan(definition, slots, budget)?;
        *self = plan.next;
        Ok(plan.report)
    }
    pub fn submit(
        &mut self,
        definition: &Definition,
        slots: &mut [Option<Slot>],
        budget: u32,
    ) -> Result<Report, Error> {
        if !definition.consume {
            return Err(Error::WrongMode);
        }
        let plan = self.plan(definition, slots, budget)?;
        // All validation and work can fail only before this publication boundary. No metadata cloning is needed.
        for (slot, quantity) in slots.iter_mut().zip(plan.counts) {
            if quantity == 0 {
                *slot = None;
            } else if let Some(slot) = slot {
                slot.quantity = quantity;
            }
        }
        *self = plan.next;
        Ok(plan.report)
    }
    pub fn encode(&self, definition: &Definition) -> Result<Vec<u8>, Error> {
        self.validate(definition)?;
        serde_json::to_vec(self).map_err(|_| Error::InvalidEncoding)
    }
    pub fn decode(bytes: &[u8], definition: &Definition) -> Result<Self, Error> {
        if bytes.len() > 1024 * 1024 {
            return Err(Error::EncodedLimit);
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| Error::InvalidEncoding)?;
        value.validate(definition)?;
        Ok(value)
    }
}
#[cfg(test)]
mod tests;
