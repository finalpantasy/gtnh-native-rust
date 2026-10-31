//! Direct comparison with a separately executed pinned MIT BetterQuesting gate reference.
use quest_core::logic::Logic;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, io::Read, path::Path};
type Error = Box<dyn std::error::Error>;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    repository: String,
    commit: String,
    version: String,
    license: String,
    enum_sha256: String,
    driver_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    logic: Logic,
    completed: u32,
    total: u32,
    result: bool,
    unlockable: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    schema_version: u32,
    maximum_total: u32,
    reference: Source,
    cases: Vec<Case>,
}
pub fn run(path: &Path) -> Result<Value, Error> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("gate reference exceeds decode budget".into());
    }
    let data: Reference = serde_json::from_slice(&bytes)?;
    let source = &data.reference;
    let hash = |s: &str| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    if data.schema_version != 1
        || data.maximum_total != 64
        || source.repository != "https://github.com/GTNewHorizons/BetterQuesting"
        || source.commit != "17d5f92f74144594c2890babc051c52032b7ec40"
        || source.version != "3.8.92-GTNH"
        || source.license != "MIT"
        || !hash(&source.enum_sha256)
        || !hash(&source.driver_sha256)
    {
        return Err("unexpected quest reference identity".into());
    }
    let expected = 6 * (data.maximum_total as usize + 1) * (data.maximum_total as usize + 2) / 2;
    if data.cases.len() != expected {
        return Err("incomplete reference case set".into());
    }
    let mut unique = BTreeSet::new();
    let mut assertions = 0;
    for case in &data.cases {
        if case.total > data.maximum_total
            || case.completed > case.total
            || !unique.insert((format!("{:?}", case.logic), case.completed, case.total))
        {
            return Err("duplicate or invalid gate reference case".into());
        }
        assert_eq!(
            case.logic.evaluate(case.completed, case.total)?,
            case.result,
            "reference truth mismatch: {:?} {}/{}",
            case.logic,
            case.completed,
            case.total
        );
        assert_eq!(
            case.logic.unlockable(case.completed, case.total)?,
            case.unlockable,
            "reference unlockability mismatch: {:?} {}/{}",
            case.logic,
            case.completed,
            case.total
        );
        assertions += 2;
    }
    Ok(
        json!({"status":"PASS","cases":unique.len(),"assertions":assertions,"maximum_total":data.maximum_total,"reference_commit":source.commit,"reference_version":source.version,
        "enum_sha256":source.enum_sha256,"driver_sha256":source.driver_sha256,"scope":"Direct pinned MIT EnumLogic valid-domain outputs; no task/party/global/repeat parity claim. Invalid-count rejection is separately unit-tested."}),
    )
}
