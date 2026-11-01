//! Opt-in policy for the observed low-voltage basic compressor processing slice.
//!
//! This crate models fixture charges and integer machine ticks. It is not an
//! electrical network, generic GregTech machine, or Java save adapter.

use industrial_sim::{
    Error,
    content::{Amount, Catalog, Id, Kind, Recipe},
    inventory::{Inventory, Limits},
};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;
pub const POLICY_VERSION: u32 = 1;
pub const EU_CAPACITY: u64 = 2048;
pub const INTAKE_THRESHOLD: u64 = 256;
pub const COOLDOWN_TICKS: i32 = 100;
pub const RECIPE_TICKS: u32 = 300;
pub const RECIPE_EU_PER_TICK: u64 = 2;
pub const OUTPUT_BATCH: u64 = 1;
const MAX_SAVE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActiveWork {
    recipe: Recipe,
    progress: i32,
    held_inputs: Vec<Amount>,
    pending_output_count: u64,
}

/// A narrowly scoped compressor using the observed LV 300-tick/2-EU policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compressor {
    schema_version: u32,
    policy_version: u32,
    kind: Id,
    inputs: Inventory,
    outputs: Inventory,
    stored_eu: u64,
    received_eu: u64,
    spent_eu: u64,
    wasted_work_eu: u64,
    active: Option<ActiveWork>,
    completed_batches: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickState {
    Idle,
    WaitingForEnergy,
    OutputBlocked,
    Intake,
    Powered,
    ShortageReset,
    Cooldown,
    Completed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickOutcome {
    pub state: TickState,
    pub energy_used: u64,
}

impl Compressor {
    pub fn new(kind: Id, input_limits: Limits, output_limits: Limits) -> Result<Self, Error> {
        validate_item_limits(input_limits)?;
        validate_item_limits(output_limits)?;
        Ok(Self {
            schema_version: SCHEMA_VERSION,
            policy_version: POLICY_VERSION,
            kind,
            inputs: Inventory::new(input_limits)?,
            outputs: Inventory::new(output_limits)?,
            stored_eu: 0,
            received_eu: 0,
            spent_eu: 0,
            wasted_work_eu: 0,
            active: None,
            completed_batches: 0,
        })
    }

    pub fn kind(&self) -> &Id {
        &self.kind
    }

    pub fn inputs(&self) -> &Inventory {
        &self.inputs
    }

    pub fn outputs(&self) -> &Inventory {
        &self.outputs
    }

    pub fn stored_eu(&self) -> u64 {
        self.stored_eu
    }

    pub fn received_eu(&self) -> u64 {
        self.received_eu
    }

    pub fn spent_eu(&self) -> u64 {
        self.spent_eu
    }

    pub fn wasted_work_eu(&self) -> u64 {
        self.wasted_work_eu
    }

    pub fn progress(&self) -> i32 {
        self.active.as_ref().map(|active| active.progress).unwrap_or(0)
    }

    pub fn max_progress(&self) -> u32 {
        if self.active.is_some() { RECIPE_TICKS } else { 0 }
    }

    pub fn eu_per_tick(&self) -> u64 {
        if self.active.is_some() { RECIPE_EU_PER_TICK } else { 0 }
    }

    pub fn pending_output_count(&self) -> u64 {
        self.active
            .as_ref()
            .map(|active| active.pending_output_count)
            .unwrap_or(0)
    }

    pub fn active_recipe(&self) -> Option<&Recipe> {
        self.active.as_ref().map(|active| &active.recipe)
    }

    pub fn completed_batches(&self) -> u64 {
        self.completed_batches
    }

    /// Adds fixture energy without modeling a voltage network or packet input.
    pub fn fixture_charge(&mut self, eu: u64) -> Result<(), Error> {
        self.validate_counters()?;
        if eu == 0 {
            return Err("fixture charge must be positive".into());
        }
        let mut candidate = self.clone();
        let stored = candidate
            .stored_eu
            .checked_add(eu)
            .ok_or("EU store overflow")?;
        if stored > EU_CAPACITY {
            return Err("compressor EU capacity exceeded".into());
        }
        candidate.received_eu = candidate
            .received_eu
            .checked_add(eu)
            .ok_or("received EU ledger overflow")?;
        candidate.stored_eu = stored;
        candidate.validate_counters()?;
        *self = candidate;
        Ok(())
    }

    /// Applies an atomic input-port edit using the shared inventory contract.
    pub fn edit_inputs(
        &mut self,
        catalog: &Catalog,
        remove: &[Amount],
        add: &[Amount],
    ) -> Result<(), Error> {
        self.validate(catalog)?;
        let mut candidate = self.clone();
        candidate.inputs.apply(catalog, remove, add)?;
        candidate.validate(catalog)?;
        *self = candidate;
        Ok(())
    }

    /// Applies an atomic output-port edit. Inserting output during active work is unsupported.
    pub fn edit_outputs(
        &mut self,
        catalog: &Catalog,
        remove: &[Amount],
        add: &[Amount],
    ) -> Result<(), Error> {
        self.validate(catalog)?;
        if self.active.is_some() && !add.is_empty() {
            return Err("output insertion during active compressor work is unsupported".into());
        }
        let mut candidate = self.clone();
        candidate.outputs.apply(catalog, remove, add)?;
        candidate.validate(catalog)?;
        *self = candidate;
        Ok(())
    }

    /// Advances one compressor machine tick, committing only a fully valid result.
    pub fn tick(&mut self, catalog: &Catalog) -> Result<TickOutcome, Error> {
        self.validate(catalog)?;
        let mut candidate = self.clone();
        let outcome = candidate.tick_inner(catalog)?;
        candidate.validate(catalog)?;
        *self = candidate;
        Ok(outcome)
    }

    fn tick_inner(&mut self, catalog: &Catalog) -> Result<TickOutcome, Error> {
        if self.active.is_none() {
            let Some(recipe) = catalog.select(&self.kind, &self.inputs)?.recipe else {
                return Ok(TickOutcome {
                    state: TickState::Idle,
                    energy_used: 0,
                });
            };
            validate_supported_recipe(catalog, &self.kind, recipe)?;

            let mut candidate_outputs = self.outputs.clone();
            if candidate_outputs
                .apply(catalog, &[], &recipe.outputs)
                .is_err()
            {
                return Ok(TickOutcome {
                    state: TickState::OutputBlocked,
                    energy_used: 0,
                });
            }
            if self.stored_eu < INTAKE_THRESHOLD {
                return Ok(TickOutcome {
                    state: TickState::WaitingForEnergy,
                    energy_used: 0,
                });
            }

            let recipe = recipe.clone();
            self.inputs.apply(catalog, &recipe.inputs, &[])?;
            self.active = Some(ActiveWork {
                held_inputs: recipe.inputs.clone(),
                recipe,
                progress: 0,
                pending_output_count: OUTPUT_BATCH,
            });
            return Ok(TickOutcome {
                state: TickState::Intake,
                energy_used: 0,
            });
        }

        let progress = self.active.as_ref().expect("active work checked").progress;
        if progress < 0 {
            let active = self.active.as_mut().expect("active work checked");
            active.progress = active
                .progress
                .checked_add(1)
                .ok_or("compressor cooldown progress overflow")?;
            return Ok(TickOutcome {
                state: TickState::Cooldown,
                energy_used: 0,
            });
        }

        if self.stored_eu < RECIPE_EU_PER_TICK {
            let lost_work = u64::try_from(progress)
                .map_err(|_| "negative compressor progress outside cooldown")?
                .checked_mul(RECIPE_EU_PER_TICK)
                .ok_or("wasted work ledger overflow")?;
            self.wasted_work_eu = self
                .wasted_work_eu
                .checked_add(lost_work)
                .ok_or("wasted work ledger overflow")?;
            self.active.as_mut().expect("active work checked").progress = -COOLDOWN_TICKS;
            return Ok(TickOutcome {
                state: TickState::ShortageReset,
                energy_used: 0,
            });
        }

        self.stored_eu -= RECIPE_EU_PER_TICK;
        self.spent_eu = self
            .spent_eu
            .checked_add(RECIPE_EU_PER_TICK)
            .ok_or("spent EU ledger overflow")?;
        let next_progress = progress
            .checked_add(1)
            .ok_or("compressor progress overflow")?;

        if next_progress == RECIPE_TICKS as i32 {
            let active = self.active.as_ref().expect("active work checked");
            let mut outputs = self.outputs.clone();
            outputs.apply(catalog, &[], &active.recipe.outputs)?;
            self.completed_batches = self
                .completed_batches
                .checked_add(1)
                .ok_or("compressor completion ledger overflow")?;
            self.outputs = outputs;
            self.active = None;
            Ok(TickOutcome {
                state: TickState::Completed,
                energy_used: RECIPE_EU_PER_TICK,
            })
        } else {
            self.active.as_mut().expect("active work checked").progress = next_progress;
            Ok(TickOutcome {
                state: TickState::Powered,
                energy_used: RECIPE_EU_PER_TICK,
            })
        }
    }

    pub fn validate(&self, catalog: &Catalog) -> Result<(), Error> {
        self.validate_counters()?;
        self.inputs.validate(catalog)?;
        self.outputs.validate(catalog)?;
        validate_item_limits(self.inputs.limits())?;
        validate_item_limits(self.outputs.limits())?;

        if let Some(active) = &self.active {
            if catalog.recipe(&active.recipe.id) != Some(&active.recipe)
                || active.recipe.machine != self.kind
                || active.progress < -COOLDOWN_TICKS
                || active.progress >= RECIPE_TICKS as i32
                || active.held_inputs != active.recipe.inputs
                || active.pending_output_count != OUTPUT_BATCH
            {
                return Err("invalid or changed active compressor recipe/state".into());
            }
            validate_supported_recipe(catalog, &self.kind, &active.recipe)?;

            let mut with_pending_output = self.outputs.clone();
            with_pending_output.apply(catalog, &[], &active.recipe.outputs)?;
        }
        Ok(())
    }

    fn validate_counters(&self) -> Result<(), Error> {
        if self.schema_version != SCHEMA_VERSION || self.policy_version != POLICY_VERSION {
            return Err("unsupported compressor save or policy version".into());
        }
        if self.stored_eu > EU_CAPACITY {
            return Err("compressor EU capacity exceeded".into());
        }
        let accounted_eu = self
            .stored_eu
            .checked_add(self.spent_eu)
            .ok_or("compressor EU ledger overflow")?;
        if accounted_eu != self.received_eu
            || self.spent_eu % RECIPE_EU_PER_TICK != 0
            || self.wasted_work_eu > self.spent_eu
            || self.wasted_work_eu % RECIPE_EU_PER_TICK != 0
        {
            return Err("inconsistent compressor energy ledger".into());
        }
        let eu_per_batch = u64::from(RECIPE_TICKS)
            .checked_mul(RECIPE_EU_PER_TICK)
            .ok_or("compressor recipe energy overflow")?;
        let completed_work = self
            .completed_batches
            .checked_mul(eu_per_batch)
            .ok_or("compressor completed work ledger overflow")?;
        let current_work = match self.active.as_ref() {
            Some(active) => u64::try_from(active.progress.max(0))
                .map_err(|_| "invalid active compressor progress")?
                .checked_mul(RECIPE_EU_PER_TICK)
                .ok_or("active compressor work ledger overflow")?,
            None => 0,
        };
        let explained_spend = completed_work
            .checked_add(self.wasted_work_eu)
            .and_then(|value| value.checked_add(current_work))
            .ok_or("compressor work ledger overflow")?;
        if explained_spend != self.spent_eu {
            return Err("spent EU does not match completed, active, and wasted work".into());
        }
        Ok(())
    }

    pub fn encode(&self, catalog: &Catalog) -> Result<Vec<u8>, Error> {
        self.validate(catalog)?;
        let bytes = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_SAVE_BYTES {
            return Err("compressor save too large".into());
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8], catalog: &Catalog) -> Result<Self, Error> {
        if bytes.len() > MAX_SAVE_BYTES {
            return Err("compressor save too large".into());
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        value.validate(catalog)?;
        Ok(value)
    }
}

fn validate_item_limits(limits: Limits) -> Result<(), Error> {
    if limits.fluid_mb != 0 {
        return Err("compressor ports accept plain items only".into());
    }
    Inventory::new(limits)?;
    Ok(())
}

fn validate_supported_recipe(
    catalog: &Catalog,
    kind: &Id,
    recipe: &Recipe,
) -> Result<(), Error> {
    if recipe.machine != *kind
        || recipe.duration_ticks != RECIPE_TICKS
        || recipe.eu_per_tick != RECIPE_EU_PER_TICK
        || recipe.inputs.len() != 1
        || recipe.outputs.len() != 1
        || recipe.outputs[0].quantity != OUTPUT_BATCH
    {
        return Err("unsupported LV compressor recipe declaration".into());
    }
    if catalog.kind(&recipe.inputs[0].resource)? != Kind::Item
        || catalog.kind(&recipe.outputs[0].resource)? != Kind::Item
    {
        return Err("LV compressor recipes require plain item inputs and outputs".into());
    }
    Ok(())
}
