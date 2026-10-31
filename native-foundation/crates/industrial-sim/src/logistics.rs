//! Original bounded aggregate-port routing experiment; no GTNH pipe parity claim.
use crate::{
    Error,
    content::{Catalog, Id, Kind},
    inventory::Inventory,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub type PortId = u8;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub id: u16,
    pub source: PortId,
    pub target: PortId,
    pub resource: Id,
    pub limit: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub edges: Vec<Edge>,
    pub max_attempts: u16,
    pub max_units: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    schema_version: u32,
    definition: Definition,
    kinds: Vec<Kind>,
    ticks: u64,
    cursor: usize,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    pub attempts: u16,
    pub moved_units: u64,
    pub transfers: u16,
    pub unavailable: u16,
    pub blocked: u16,
}
impl Definition {
    fn normalized(&self, catalog: &Catalog) -> Result<Self, Error> {
        if self.edges.is_empty()
            || self.edges.len() > 1024
            || !(1..=1024).contains(&self.max_attempts)
            || self.max_units == 0
        {
            return Err("invalid logistics graph or work budgets".into());
        }
        let mut result = self.clone();
        result.edges.sort_by_key(|e| e.id);
        let mut routes = BTreeSet::new();
        let mut previous = None;
        for edge in &result.edges {
            catalog.kind(&edge.resource)?;
            if edge.limit == 0
                || edge.source == edge.target
                || previous == Some(edge.id)
                || !routes.insert((edge.source, edge.target, &edge.resource))
            {
                return Err("invalid or duplicate logistics edge".into());
            }
            previous = Some(edge.id);
        }
        Ok(result)
    }
}
impl Network {
    pub fn new(definition: Definition, catalog: &Catalog) -> Result<Self, Error> {
        let definition = definition.normalized(catalog)?;
        let kinds = definition
            .edges
            .iter()
            .map(|e| catalog.kind(&e.resource))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            schema_version: 1,
            definition,
            kinds,
            ticks: 0,
            cursor: 0,
        })
    }
    pub fn ticks(&self) -> u64 {
        self.ticks
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    fn validate(&self, catalog: &Catalog) -> Result<(), Error> {
        let expected = Self::new(self.definition.clone(), catalog)?;
        if self.schema_version != 1
            || self.definition != expected.definition
            || self.kinds != expected.kinds
            || self.cursor >= self.definition.edges.len()
        {
            return Err("invalid or changed logistics save".into());
        }
        Ok(())
    }
    pub fn step(
        &mut self,
        catalog: &Catalog,
        ports: &mut BTreeMap<PortId, Inventory>,
    ) -> Result<Report, Error> {
        self.validate(catalog)?;
        let next_tick = self
            .ticks
            .checked_add(1)
            .ok_or("logistics tick sequence exhausted")?;
        let mut supply = BTreeMap::new();
        for (&port, inventory) in ports.iter() {
            inventory.validate(catalog)?;
            for (resource, quantity) in inventory.contents() {
                supply.insert((port, resource.clone()), quantity);
            }
        }
        let mut staged = ports.clone();
        let mut cursor = self.cursor;
        let mut report = Report::default();
        for _ in 0..usize::from(self.definition.max_attempts).min(self.definition.edges.len()) {
            let edge = &self.definition.edges[cursor];
            cursor = (cursor + 1) % self.definition.edges.len();
            report.attempts += 1;
            if !staged.contains_key(&edge.source) || !staged.contains_key(&edge.target) {
                report.unavailable += 1;
                continue;
            }
            let key = (edge.source, edge.resource.clone());
            let available = supply.get(&key).copied().unwrap_or(0);
            let allowance = available
                .min(edge.limit)
                .min(self.definition.max_units - report.moved_units);
            let mut source = staged.remove(&edge.source).expect("source checked above");
            let moved = source.transfer(
                staged
                    .get_mut(&edge.target)
                    .expect("distinct target checked above"),
                catalog,
                &edge.resource,
                allowance,
            )?;
            staged.insert(edge.source, source);
            if moved == 0 {
                report.blocked += 1;
            } else {
                supply.insert(key, available - moved);
                report.moved_units += moved;
                report.transfers += 1;
            }
            if report.moved_units == self.definition.max_units {
                break;
            }
        }
        *ports = staged;
        self.cursor = cursor;
        self.ticks = next_tick;
        Ok(report)
    }
    pub fn encode(&self, catalog: &Catalog) -> Result<Vec<u8>, Error> {
        self.validate(catalog)?;
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if bytes.len() > 1024 * 1024 {
            return Err("logistics save too large".into());
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8], expected: &Definition, catalog: &Catalog) -> Result<Self, Error> {
        if bytes.len() > 1024 * 1024 {
            return Err("logistics save too large".into());
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        value.validate(catalog)?;
        if value.definition != expected.normalized(catalog)? {
            return Err("logistics topology or budgets changed".into());
        }
        Ok(value)
    }
}
#[cfg(test)]
mod tests;
