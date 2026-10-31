//! Persistent recipe work and integer energy packets.

use crate::{
    Error,
    content::{Amount, Catalog, Id, Recipe},
    inventory::{Inventory, Limits},
};
use serde::{Deserialize, Serialize};

/// Reference: GT5-Unofficial 5.09.54.264 GTValues.V, first nine tiers.
pub const TIER_VOLTAGES: [u64; 9] = [8, 32, 128, 512, 2048, 8192, 32768, 131072, 524288];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Energy {
    stored: u64,
    capacity: u64,
    max_voltage: u64,
    max_amps: u64,
}
impl Energy {
    pub fn new(capacity: u64, max_voltage: u64, max_amps: u64) -> Result<Self, Error> {
        let value = Self {
            stored: 0,
            capacity,
            max_voltage,
            max_amps,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn stored(&self) -> u64 {
        self.stored
    }
    pub fn capacity(&self) -> u64 {
        self.capacity
    }
    pub fn max_voltage(&self) -> u64 {
        self.max_voltage
    }
    pub fn max_amps(&self) -> u64 {
        self.max_amps
    }
    pub fn receive(&mut self, voltage: u64, amps: u64) -> Result<u64, Error> {
        self.validate()?;
        if voltage == 0 || amps == 0 || voltage > self.max_voltage || amps > self.max_amps {
            return Err("unsupported energy packet".into());
        }
        let amount = voltage.checked_mul(amps).ok_or("energy packet overflow")?;
        let next = self
            .stored
            .checked_add(amount)
            .ok_or("energy store overflow")?;
        if next > self.capacity {
            return Err("insufficient energy capacity".into());
        }
        self.stored = next;
        Ok(amount)
    }
    fn validate(&self) -> Result<(), Error> {
        if self.capacity == 0
            || self.max_voltage == 0
            || self.max_amps == 0
            || self.stored > self.capacity
            || self.max_voltage.checked_mul(self.max_amps).is_none()
        {
            return Err("invalid energy store".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    recipe: Recipe,
    progress: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Machine {
    kind: Id,
    inputs: Inventory,
    outputs: Inventory,
    energy: Energy,
    active: Option<Job>,
    completed: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Idle,
    Running,
    Completed,
    NoPower,
    OutputBlocked,
    WrongTier,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Tick {
    pub state: State,
    pub started: bool,
    pub energy_used: u64,
}

impl Machine {
    pub fn new(kind: Id, inputs: Limits, outputs: Limits, energy: Energy) -> Result<Self, Error> {
        energy.validate()?;
        Ok(Self {
            kind,
            inputs: Inventory::new(inputs)?,
            outputs: Inventory::new(outputs)?,
            energy,
            active: None,
            completed: 0,
        })
    }
    pub fn kind(&self) -> &Id {
        &self.kind
    }
    pub fn inputs(&self) -> &Inventory {
        &self.inputs
    }
    pub fn inputs_mut(&mut self) -> &mut Inventory {
        &mut self.inputs
    }
    pub fn outputs(&self) -> &Inventory {
        &self.outputs
    }
    pub fn outputs_mut(&mut self) -> &mut Inventory {
        &mut self.outputs
    }
    pub fn energy(&self) -> &Energy {
        &self.energy
    }
    pub fn energy_mut(&mut self) -> &mut Energy {
        &mut self.energy
    }
    pub fn progress(&self) -> Option<u32> {
        self.active.as_ref().map(|j| j.progress)
    }
    pub fn held_inputs(&self) -> &[Amount] {
        self.active
            .as_ref()
            .map(|j| j.recipe.inputs.as_slice())
            .unwrap_or(&[])
    }
    pub fn completed(&self) -> u64 {
        self.completed
    }
    pub fn validate(&self, catalog: &Catalog) -> Result<(), Error> {
        self.inputs.validate(catalog)?;
        self.outputs.validate(catalog)?;
        self.energy.validate()?;
        if let Some(job) = &self.active {
            if catalog.recipe(&job.recipe.id) != Some(&job.recipe)
                || job.recipe.machine != self.kind
                || job.progress > job.recipe.duration_ticks
                || job.recipe.eu_per_tick > self.energy.max_voltage
            {
                return Err("invalid or changed active recipe".into());
            }
            // A valid active job could originally fit the configured output ports.
            Inventory::new(self.outputs.limits())?.apply(catalog, &[], &job.recipe.outputs)?;
        }
        Ok(())
    }
    pub fn tick(&mut self, catalog: &Catalog) -> Result<Tick, Error> {
        self.validate(catalog)?;
        let mut candidate = self.clone();
        let tick = candidate.tick_inner(catalog)?;
        *self = candidate;
        Ok(tick)
    }
    fn tick_inner(&mut self, catalog: &Catalog) -> Result<Tick, Error> {
        let mut tick = Tick {
            state: State::Idle,
            started: false,
            energy_used: 0,
        };
        if self.active.is_none() {
            let Some(recipe) = catalog.select(&self.kind, &self.inputs)?.recipe else {
                return Ok(tick);
            };
            if recipe.eu_per_tick > self.energy.max_voltage {
                tick.state = State::WrongTier;
                return Ok(tick);
            }
            if self.energy.stored < recipe.eu_per_tick {
                tick.state = State::NoPower;
                return Ok(tick);
            }
            if self
                .outputs
                .clone()
                .apply(catalog, &[], &recipe.outputs)
                .is_err()
            {
                tick.state = State::OutputBlocked;
                return Ok(tick);
            }
            self.inputs.apply(catalog, &recipe.inputs, &[])?;
            self.active = Some(Job {
                recipe: recipe.clone(),
                progress: 0,
            });
            tick.started = true;
        }
        let job = self.active.as_mut().unwrap();
        if job.progress < job.recipe.duration_ticks {
            if self.energy.stored < job.recipe.eu_per_tick {
                tick.state = State::NoPower;
                return Ok(tick);
            }
            self.energy.stored -= job.recipe.eu_per_tick;
            tick.energy_used = job.recipe.eu_per_tick;
            job.progress += 1;
        }
        if job.progress == job.recipe.duration_ticks {
            let mut outputs = self.outputs.clone();
            if outputs.apply(catalog, &[], &job.recipe.outputs).is_err() {
                tick.state = State::OutputBlocked;
                return Ok(tick);
            }
            self.completed = self
                .completed
                .checked_add(1)
                .ok_or("machine completion counter exhausted")?;
            self.outputs = outputs;
            self.active = None;
            tick.state = State::Completed;
        } else {
            tick.state = State::Running;
        }
        Ok(tick)
    }
    pub fn encode(&self, catalog: &Catalog) -> Result<Vec<u8>, Error> {
        self.validate(catalog)?;
        serde_json::to_vec(self).map_err(|e| e.to_string())
    }
    pub fn decode(bytes: &[u8], catalog: &Catalog) -> Result<Self, Error> {
        if bytes.len() > 1024 * 1024 {
            return Err("machine save too large".into());
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        value.validate(catalog)?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn prioritized_machine(expensive: bool) -> (Catalog, Machine) {
        let mut content: crate::content::Content =
            serde_json::from_slice(include_bytes!("../../../fixtures/industrial-v1.json")).unwrap();
        let mut first = content.recipes[0].clone();
        first.id = crate::content::Id::new("lab:a_priority").unwrap();
        if expensive {
            first.eu_per_tick = 64;
        } else {
            first.outputs[0].quantity = 100;
        }
        content.recipes.push(first);
        let catalog = Catalog::new(content).unwrap();
        let ports = Limits {
            items: 16,
            fluid_mb: 0,
            kinds: 4,
        };
        let mut machine = Machine::new(
            crate::content::Id::new("lab:crusher").unwrap(),
            ports,
            ports,
            Energy::new(128, 32, 1).unwrap(),
        )
        .unwrap();
        machine
            .inputs_mut()
            .apply(
                &catalog,
                &[],
                &[Amount {
                    resource: crate::content::Id::new("lab:iron_ore").unwrap(),
                    quantity: 1,
                }],
            )
            .unwrap();
        machine.energy_mut().receive(32, 1).unwrap();
        (catalog, machine)
    }
    #[test]
    fn indexed_lookup_keeps_the_first_match_even_when_a_later_recipe_fits_the_tier() {
        let (catalog, mut machine) = prioritized_machine(true);
        let before = machine.clone();
        assert_eq!(machine.tick(&catalog).unwrap().state, State::WrongTier);
        assert_eq!(machine, before);
    }
    #[test]
    fn indexed_lookup_keeps_the_first_match_even_when_a_later_recipe_fits_the_output() {
        let (catalog, mut machine) = prioritized_machine(false);
        let before = machine.clone();
        assert_eq!(machine.tick(&catalog).unwrap().state, State::OutputBlocked);
        assert_eq!(machine, before);
    }
    fn id(s: &str) -> Id {
        Id::new(format!("test:{s}")).unwrap()
    }
    fn setup() -> (Catalog, Machine) {
        let cat=Catalog::from_json(br#"{"schema_version":1,"provenance":"synthetic-test","resources":[{"id":"test:ore","kind":"item"},{"id":"test:dust","kind":"item"}],"recipes":[{"id":"test:grind","machine":"test:grinder","duration_ticks":3,"eu_per_tick":8,"inputs":[{"resource":"test:ore","quantity":1}],"outputs":[{"resource":"test:dust","quantity":2}]}]}"#).unwrap();
        let limits = Limits {
            items: 4,
            fluid_mb: 0,
            kinds: 2,
        };
        let mut machine = Machine::new(
            id("grinder"),
            limits,
            limits,
            Energy::new(128, 32, 2).unwrap(),
        )
        .unwrap();
        machine
            .inputs
            .apply(
                &cat,
                &[],
                &[Amount {
                    resource: id("ore"),
                    quantity: 2,
                }],
            )
            .unwrap();
        (cat, machine)
    }
    #[test]
    fn packets_reject_overvoltage_amperage_overflow_and_insufficient_capacity_atomically() {
        let mut e = Energy::new(128, 32, 2).unwrap();
        assert_eq!(e.receive(32, 2).unwrap(), 64);
        let old = e.clone();
        for packet in [
            (128, 1),
            (32, 3),
            (0, 1),
            (32, 0),
            (u64::MAX, u64::MAX),
            (32, 3),
        ] {
            assert!(e.receive(packet.0, packet.1).is_err());
            assert_eq!(e, old);
        }
        assert_eq!(e.receive(32, 2).unwrap(), 64);
        let old = e.clone();
        assert!(e.receive(8, 1).is_err());
        assert_eq!(e, old);
        assert!(Energy::new(0, 32, 1).is_err());
        assert!(Energy::new(128, u64::MAX, 2).is_err());
    }
    #[test]
    fn inputs_are_escrowed_once_and_starvation_does_not_advance_work() {
        let (cat, mut m) = setup();
        let before = m.clone();
        assert_eq!(m.tick(&cat).unwrap().state, State::NoPower);
        assert_eq!(m, before);
        m.energy.receive(8, 1).unwrap();
        let t = m.tick(&cat).unwrap();
        assert!(t.started);
        assert_eq!(t.energy_used, 8);
        assert_eq!(m.progress(), Some(1));
        assert_eq!(m.inputs.amount(&id("ore")), 1);
        assert_eq!(m.held_inputs()[0].quantity, 1);
        let before = m.clone();
        assert_eq!(m.tick(&cat).unwrap().state, State::NoPower);
        assert_eq!(m, before);
        m.energy.receive(8, 2).unwrap();
        m.tick(&cat).unwrap();
        assert_eq!(m.tick(&cat).unwrap().state, State::Completed);
        assert_eq!(m.outputs.amount(&id("dust")), 2);
        assert_eq!(m.completed, 1);
        assert!(m.held_inputs().is_empty());
        assert_eq!(m.energy.stored, 0);
    }
    #[test]
    fn output_pressure_holds_completed_work_without_spending_more_energy() {
        let (cat, mut m) = setup();
        m.energy.receive(32, 2).unwrap();
        m.tick(&cat).unwrap();
        m.outputs
            .apply(
                &cat,
                &[],
                &[Amount {
                    resource: id("dust"),
                    quantity: 4,
                }],
            )
            .unwrap();
        m.tick(&cat).unwrap();
        assert_eq!(m.tick(&cat).unwrap().state, State::OutputBlocked);
        assert_eq!(m.progress(), Some(3));
        let stored = m.energy.stored;
        for _ in 0..20 {
            assert_eq!(m.tick(&cat).unwrap().energy_used, 0);
        }
        assert_eq!(m.energy.stored, stored);
        assert_eq!(m.completed, 0);
        m.outputs
            .apply(
                &cat,
                &[Amount {
                    resource: id("dust"),
                    quantity: 2,
                }],
                &[],
            )
            .unwrap();
        assert_eq!(m.tick(&cat).unwrap().state, State::Completed);
        assert_eq!(m.completed, 1);
        assert_eq!(m.outputs.amount(&id("dust")), 4);
    }
    #[test]
    fn active_save_roundtrip_continues_once_and_changed_recipes_are_rejected() {
        let (cat, mut m) = setup();
        m.energy.receive(32, 1).unwrap();
        m.tick(&cat).unwrap();
        let bytes = m.encode(&cat).unwrap();
        let mut restored = Machine::decode(&bytes, &cat).unwrap();
        assert_eq!(restored, m);
        restored.tick(&cat).unwrap();
        restored.tick(&cat).unwrap();
        assert_eq!(restored.completed, 1);
        let mut changed = setup().0;
        changed
            .recipes
            .get_mut(&id("grind"))
            .unwrap()
            .duration_ticks = 4;
        assert!(Machine::decode(&bytes, &changed).is_err());
        let mut invalid: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        invalid["active"]["progress"] = serde_json::json!(4);
        assert!(Machine::decode(&serde_json::to_vec(&invalid).unwrap(), &cat).is_err());
    }
    #[test]
    fn an_under_tier_machine_never_consumes_inputs_or_energy() {
        let (cat, mut m) = setup();
        m.energy = Energy::new(128, 4, 2).unwrap();
        m.energy.receive(4, 2).unwrap();
        let before = m.clone();
        assert_eq!(m.tick(&cat).unwrap().state, State::WrongTier);
        assert_eq!(m, before);
    }

    #[test]
    fn exhausted_completion_counter_cannot_partially_consume_the_last_tick() {
        let (cat, mut m) = setup();
        m.energy.receive(32, 2).unwrap();
        m.tick(&cat).unwrap();
        m.tick(&cat).unwrap();
        m.completed = u64::MAX;
        let before = m.clone();
        assert!(m.tick(&cat).is_err());
        assert_eq!(m, before);
    }

    #[test]
    fn decoded_energy_and_port_corruption_are_rejected_before_a_tick() {
        let (cat, m) = setup();
        let bytes = m.encode(&cat).unwrap();
        for (field, value) in [
            ("stored", 129),
            ("capacity", 0),
            ("max_voltage", 0),
            ("max_amps", 0),
        ] {
            let mut data: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            data["energy"][field] = serde_json::json!(value);
            assert!(Machine::decode(&serde_json::to_vec(&data).unwrap(), &cat).is_err());
        }
        let mut data: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        data["inputs"]["contents"]["test:ore"] = serde_json::json!(5);
        assert!(Machine::decode(&serde_json::to_vec(&data).unwrap(), &cat).is_err());
    }
}
