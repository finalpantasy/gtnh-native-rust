use super::*;
use crate::item_match::{DEFAULT_BUDGET, Tag};
use std::collections::BTreeMap;
fn stack(name: &str, damage: i32) -> Stack {
    Stack {
        registry: format!("minecraft:{name}"),
        damage,
        nbt: None,
    }
}
fn requirement(name: &str, quantity: u32) -> Requirement {
    Requirement {
        stack: stack(name, 0),
        quantity,
        damageable: false,
        ore_alias: String::new(),
    }
}
fn definition(consume: bool) -> Definition {
    Definition {
        key: "synthetic:task/0".into(),
        requirements: vec![requirement("cobblestone", 2), requirement("wheat", 3)],
        consume,
        ignore_nbt: true,
        partial: true,
        only_one: false,
        group_detect: false,
        auto_consume: false,
    }
}
fn slot(name: &str, quantity: u32) -> Option<Slot> {
    Some(Slot {
        stack: stack(name, 0),
        quantity,
    })
}
#[test]
fn detection_resets_partial_counts_and_latches_fulfilled_requirements_across_scans() {
    let def = definition(false);
    let mut state = Progress::new(&def).unwrap();
    state
        .detect(&def, &[slot("cobblestone", 1)], DEFAULT_BUDGET)
        .unwrap();
    assert_eq!(state.values(), [1, 0]);
    state.detect(&def, &[], DEFAULT_BUDGET).unwrap();
    assert_eq!(state.values(), [0, 0]);
    let inventory = [slot("cobblestone", 2)];
    let before = inventory.clone();
    state.detect(&def, &inventory, DEFAULT_BUDGET).unwrap();
    assert_eq!(inventory, before);
    assert_eq!(state.values(), [2, 0]);
    state.detect(&def, &[], DEFAULT_BUDGET).unwrap();
    assert_eq!(state.values(), [2, 0]);
    let report = state
        .detect(&def, &[slot("wheat", 3)], DEFAULT_BUDGET)
        .unwrap();
    assert!(report.satisfied);
    assert_eq!(state.values(), [2, 3]);
}
#[test]
fn duplicate_requirements_do_not_double_count_one_scan_but_latch_between_scans() {
    let mut def = definition(false);
    def.requirements = vec![requirement("cobblestone", 2), requirement("cobblestone", 2)];
    let mut state = Progress::new(&def).unwrap();
    let inventory = [slot("cobblestone", 2)];
    assert!(
        !state
            .detect(&def, &inventory, DEFAULT_BUDGET)
            .unwrap()
            .satisfied
    );
    assert_eq!(state.values(), [2, 0]);
    assert!(
        state
            .detect(&def, &inventory, DEFAULT_BUDGET)
            .unwrap()
            .satisfied
    );
    assert_eq!(state.values(), [2, 2]);
    assert_eq!(inventory[0].as_ref().unwrap().quantity, 2);
}
#[test]
fn submission_conserves_quantities_splits_duplicate_requirements_and_preserves_leftovers() {
    let mut def = definition(true);
    def.requirements = vec![requirement("cobblestone", 2), requirement("cobblestone", 2)];
    let mut state = Progress::new(&def).unwrap();
    let mut inventory = [slot("cobblestone", 3)];
    assert_eq!(
        state
            .submit(&def, &mut inventory, DEFAULT_BUDGET)
            .unwrap()
            .consumed,
        3
    );
    assert_eq!(state.values(), [2, 1]);
    assert_eq!(inventory, [None]);
    let mut inventory = [slot("cobblestone", 5)];
    let report = state.submit(&def, &mut inventory, DEFAULT_BUDGET).unwrap();
    assert_eq!(report.consumed, 1);
    assert!(report.satisfied);
    assert_eq!(inventory[0].as_ref().unwrap().quantity, 4);
    let before = (state.clone(), inventory.clone());
    let report = state.submit(&def, &mut inventory, DEFAULT_BUDGET).unwrap();
    assert_eq!(report.consumed, 0);
    assert!(!report.changed);
    assert_eq!((state, inventory), before);
}
#[test]
fn only_one_stops_before_consuming_other_requirements() {
    let mut def = definition(true);
    def.only_one = true;
    let mut state = Progress::new(&def).unwrap();
    let mut inventory = [slot("cobblestone", 2), slot("wheat", 3)];
    let report = state.submit(&def, &mut inventory, DEFAULT_BUDGET).unwrap();
    assert!(report.satisfied);
    assert_eq!(report.consumed, 2);
    assert_eq!(state.values(), [2, 0]);
    assert_eq!(inventory, [None, slot("wheat", 3)]);
}
#[test]
fn budget_failure_after_a_staged_donation_rolls_back_progress_and_all_slots() {
    let def = definition(true);
    let mut state = Progress::new(&def).unwrap();
    let mut inventory = [slot("cobblestone", 2), slot("wheat", 3)];
    let before = (state.clone(), inventory.clone());
    assert_eq!(
        state.submit(&def, &mut inventory, 2),
        Err(Error::Match(MatchError::QueryLimit))
    );
    assert_eq!((state, inventory), before);
    let def = definition(false);
    let mut state = Progress::new(&def).unwrap();
    state
        .detect(&def, &[slot("cobblestone", 1)], DEFAULT_BUDGET)
        .unwrap();
    let before = state.clone();
    assert!(
        state
            .detect(&def, &[slot("cobblestone", 2), slot("wheat", 3)], 2)
            .is_err()
    );
    assert_eq!(state, before);
}
#[test]
fn mode_mismatch_and_unsupported_registry_assumptions_cannot_change_state() {
    for consume in [false, true] {
        let def = definition(consume);
        let mut state = Progress::new(&def).unwrap();
        let mut inventory = [slot("cobblestone", 2)];
        let before = (state.clone(), inventory.clone());
        let result = if consume {
            state.detect(&def, &inventory, DEFAULT_BUDGET)
        } else {
            state.submit(&def, &mut inventory, DEFAULT_BUDGET)
        };
        assert_eq!(result, Err(Error::WrongMode));
        assert_eq!((state, inventory), before);
    }
    for field in ["group_detect", "auto_consume"] {
        let mut value = serde_json::to_value(definition(false)).unwrap();
        value[field] = serde_json::json!(true);
        let def: Definition = serde_json::from_value(value).unwrap();
        assert_eq!(Progress::new(&def), Err(Error::UnsupportedDefinition));
    }
    let mut def = definition(false);
    def.requirements[0].ore_alias = "cobblestone".into();
    assert_eq!(Progress::new(&def), Err(Error::UnsupportedDefinition));
    let mut def = definition(false);
    let mut duplicate = def.requirements[0].clone();
    duplicate.damageable = true;
    def.requirements.push(duplicate);
    assert_eq!(Progress::new(&def), Err(Error::InvalidDefinition));
}
#[test]
fn metadata_nbt_and_damageability_filter_the_actual_submitted_stack() {
    let mut def = definition(true);
    def.requirements.truncate(1);
    def.requirements[0].stack.damage = 32767;
    def.ignore_nbt = false;
    def.requirements[0].stack.nbt = Some(Tag::Compound(BTreeMap::from([(
        "variant".into(),
        Tag::Int(1),
    )])));
    let mut state = Progress::new(&def).unwrap();
    let mut wrong = slot("cobblestone", 2).unwrap();
    wrong.stack.damage = 5;
    wrong.stack.nbt = Some(Tag::Compound(BTreeMap::from([(
        "variant".into(),
        Tag::Int(2),
    )])));
    let mut inventory = [Some(wrong)];
    let before = inventory.clone();
    assert_eq!(
        state
            .submit(&def, &mut inventory, DEFAULT_BUDGET)
            .unwrap()
            .consumed,
        0
    );
    assert_eq!(inventory, before);
    inventory[0].as_mut().unwrap().stack.nbt = Some(Tag::Compound(BTreeMap::from([
        ("variant".into(), Tag::Long(1)),
        ("extra".into(), Tag::Int(9)),
    ])));
    assert_eq!(
        state
            .submit(&def, &mut inventory, DEFAULT_BUDGET)
            .unwrap()
            .consumed,
        2
    );
    assert!(state.satisfied(&def).unwrap());
}
#[test]
fn saves_bind_task_order_quantities_flags_and_key_and_reject_corrupt_progress() {
    let def = definition(true);
    let mut state = Progress::new(&def).unwrap();
    state
        .submit(&def, &mut [slot("cobblestone", 1)], DEFAULT_BUDGET)
        .unwrap();
    let bytes = state.encode(&def).unwrap();
    assert_eq!(Progress::decode(&bytes, &def).unwrap(), state);
    for field in ["key", "consume", "ignore_nbt", "partial", "only_one"] {
        let mut value = serde_json::to_value(&def).unwrap();
        if field == "key" {
            value[field] = serde_json::json!("another:key");
        } else {
            value[field] = serde_json::json!(!value[field].as_bool().unwrap());
        }
        let changed: Definition = serde_json::from_value(value).unwrap();
        assert!(Progress::decode(&bytes, &changed).is_err());
    }
    let mut changed = def.clone();
    changed.requirements.reverse();
    assert!(Progress::decode(&bytes, &changed).is_err());
    changed = def.clone();
    changed.requirements[0].quantity += 1;
    assert!(Progress::decode(&bytes, &changed).is_err());
    for progress in [serde_json::json!([1]), serde_json::json!([3, 0])] {
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["progress"] = progress;
        assert!(Progress::decode(&serde_json::to_vec(&value).unwrap(), &def).is_err());
    }
    let duplicate = String::from_utf8(bytes).unwrap().replacen(
        "\"progress\":[1,0]",
        "\"progress\":[1,0],\"progress\":[1,0]",
        1,
    );
    assert!(Progress::decode(duplicate.as_bytes(), &def).is_err());
}
#[test]
fn slot_definition_and_encoded_size_bounds_fail_without_mutation() {
    for count in [0, 65] {
        let mut def = definition(false);
        def.requirements = vec![requirement("cobblestone", 1); count];
        assert!(Progress::new(&def).is_err());
    }
    for quantity in [0, u32::MAX] {
        let mut def = definition(false);
        def.requirements[0].quantity = quantity;
        assert!(Progress::new(&def).is_err());
    }
    let def = definition(true);
    let mut state = Progress::new(&def).unwrap();
    for mut inventory in [
        vec![slot("cobblestone", 1); 129],
        vec![slot("cobblestone", 0)],
    ] {
        let before = (state.clone(), inventory.clone());
        assert!(state.submit(&def, &mut inventory, DEFAULT_BUDGET).is_err());
        assert_eq!((state.clone(), inventory), before);
    }
    let tag = Tag::Compound(BTreeMap::from([(
        "text".into(),
        Tag::String("x".repeat(16384)),
    )]));
    let mut def = definition(false);
    def.requirements = vec![requirement("cobblestone", 1); 64];
    for req in &mut def.requirements {
        req.stack.nbt = Some(tag.clone());
    }
    assert_eq!(def.fingerprint(), Err(Error::EncodedLimit));
    let def = definition(true);
    let mut inventory = vec![slot("cobblestone", 1); 128];
    for slot in inventory.iter_mut().flatten() {
        slot.stack.nbt = Some(tag.clone());
    }
    let before = (state.clone(), inventory.clone());
    assert_eq!(
        state.submit(&def, &mut inventory, DEFAULT_BUDGET),
        Err(Error::EncodedLimit)
    );
    assert_eq!((state, inventory), before);
}
