//! Single-player Detector-count conformance over explicit model command sequences.
use quest_core::{
    item_match::DEFAULT_BUDGET,
    retrieval::{Definition, Progress, Slot},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
type Error = Box<dyn std::error::Error>;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    slots: Vec<Option<Slot>>,
    expected_progress: Vec<u32>,
    expected_slots: Vec<Option<Slot>>,
    expected_satisfied: bool,
    expected_consumed: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    definition: Definition,
    steps: Vec<Step>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cases {
    schema_version: u32,
    source_commit: String,
    scenarios: Vec<Scenario>,
}
pub fn run(path: &Path) -> Result<Value, Error> {
    if std::fs::metadata(path)?.len() > 32 * 1024 * 1024 {
        return Err("retrieval oracle input too large".into());
    }
    let input: Cases = serde_json::from_slice(&std::fs::read(path)?)?;
    if input.schema_version != 1
        || input.source_commit != "17d5f92f74144594c2890babc051c52032b7ec40"
        || input.scenarios.is_empty()
        || input.scenarios.len() > 2000
    {
        return Err("invalid retrieval oracle header/count".into());
    }
    let (mut steps, mut consumed, mut operations) = (0usize, 0u64, 0u64);
    for (index, scenario) in input.scenarios.iter().enumerate() {
        if scenario.steps.is_empty() || scenario.steps.len() > 16 {
            return Err("invalid retrieval oracle step count".into());
        }
        let mut progress = Progress::new(&scenario.definition)?;
        for (step, row) in scenario.steps.iter().enumerate() {
            let mut inventory = row.slots.clone();
            let report = if scenario.definition.consume {
                progress.submit(&scenario.definition, &mut inventory, DEFAULT_BUDGET)?
            } else {
                progress.detect(&scenario.definition, &inventory, DEFAULT_BUDGET)?
            };
            if progress.values() != row.expected_progress
                || inventory != row.expected_slots
                || report.satisfied != row.expected_satisfied
                || report.consumed != row.expected_consumed
            {
                return Err(
                    format!("retrieval oracle mismatch in scenario {index}, step {step}").into(),
                );
            }
            // Reopen every step so the same comparison also exercises definition-bound persistence.
            progress = Progress::decode(
                &progress.encode(&scenario.definition)?,
                &scenario.definition,
            )?;
            steps += 1;
            consumed += report.consumed;
            operations += u64::from(report.operations);
        }
    }
    Ok(
        json!({"status":"PASS","scenarios":input.scenarios.len(),"steps":steps,"assertions":steps*4,"reopens":steps,"consumed_units":consumed,"counted_operations":operations,
        "scope":"Exact pinned Detector over single-player model donation/detection commands and a local satisfaction latch; no actual Minecraft inventory callbacks, completion timing, party/group/ore/auto-consume parity"}),
    )
}
