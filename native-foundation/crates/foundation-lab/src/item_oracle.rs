//! Compare native compatibility results with pinned Java comparator/API-model fixtures.
use quest_core::item_match::{DEFAULT_BUDGET, Stack, Tag, compare_nbt, stack_match};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
type Error = Box<dyn std::error::Error>;
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Case {
    Nbt {
        required: Option<Tag>,
        sample: Option<Tag>,
        partial: bool,
        expected: bool,
    },
    Stack {
        required: Option<Stack>,
        sample: Option<Stack>,
        partial: bool,
        damageable: bool,
        nbt_check: bool,
        same_instance: bool,
        expected: bool,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cases {
    schema_version: u32,
    source_commit: String,
    cases: Vec<Case>,
}
pub fn run(path: &Path) -> Result<Value, Error> {
    if std::fs::metadata(path)?.len() > 32 * 1024 * 1024 {
        return Err("item oracle case file too large".into());
    }
    let input: Cases = serde_json::from_slice(&std::fs::read(path)?)?;
    if input.schema_version != 1
        || input.source_commit != "17d5f92f74144594c2890babc051c52032b7ec40"
        || input.cases.is_empty()
        || input.cases.len() > 100_000
    {
        return Err("invalid item oracle header/count".into());
    }
    let (mut nbt, mut stacks, mut operations) = (0usize, 0usize, 0u64);
    for (index, case) in input.cases.iter().enumerate() {
        let (comparison, expected) = match case {
            Case::Nbt {
                required,
                sample,
                partial,
                expected,
            } => {
                nbt += 1;
                (
                    compare_nbt(required.as_ref(), sample.as_ref(), *partial, DEFAULT_BUDGET)?,
                    *expected,
                )
            }
            Case::Stack {
                required,
                sample,
                partial,
                damageable,
                nbt_check,
                same_instance,
                expected,
            } => {
                stacks += 1;
                if *same_instance && required != sample {
                    return Err("same-object oracle case has different values".into());
                }
                let candidate = if *same_instance {
                    required.as_ref()
                } else {
                    sample.as_ref()
                };
                (
                    stack_match(
                        required.as_ref(),
                        candidate,
                        *damageable,
                        *nbt_check,
                        *partial,
                        DEFAULT_BUDGET,
                    )?,
                    *expected,
                )
            }
        };
        if comparison.matches != expected {
            return Err(format!(
                "legacy item comparator mismatch in case {index}: {} != {expected}",
                comparison.matches
            )
            .into());
        }
        operations += u64::from(comparison.operations);
    }
    Ok(
        json!({"status":"PASS","cases":input.cases.len(),"assertions":input.cases.len(),"nbt_cases":nbt,"stack_cases":stacks,"counted_native_operations":operations,
        "scope":"Exact pinned Java comparator over explicit API models; no actual Minecraft runtime, binary NBT, ore membership, retrieval/party or whole-mod parity"}),
    )
}
