use gtnh_basic_machine::{Compressor, TickState, EU_CAPACITY, INTAKE_THRESHOLD, POLICY_VERSION};
use industrial_sim::{
    content::{Amount, Catalog, Content, Id, Kind, Recipe, Resource},
    inventory::Limits,
};
use serde::Deserialize;

const FIXTURE: &str = include_str!("../fixtures/compressor-lv-v1.json");
const MACHINE: &str = "reference:compressor";
const INPUT: &str = "reference:block_sand";
const OUTPUT: &str = "reference:block_sandstone";

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}

fn amount(resource: &str, quantity: u64) -> Amount {
    Amount {
        resource: id(resource),
        quantity,
    }
}

fn catalog() -> Catalog {
    Catalog::new(Content {
        schema_version: 1,
        provenance: "sanitized compressor fixture".into(),
        resources: vec![
            Resource {
                id: id(INPUT),
                kind: Kind::Item,
            },
            Resource {
                id: id(OUTPUT),
                kind: Kind::Item,
            },
        ],
        recipes: vec![Recipe {
            id: id("reference:compress_sand"),
            machine: id(MACHINE),
            duration_ticks: 300,
            eu_per_tick: 2,
            inputs: vec![amount(INPUT, 4)],
            outputs: vec![amount(OUTPUT, 1)],
        }],
    })
    .unwrap()
}

fn one_item_catalog() -> Catalog {
    let input = "reference:snow_block";
    let output = "reference:ice";
    Catalog::new(Content {
        schema_version: 1,
        provenance: "single-item compressor declaration".into(),
        resources: vec![
            Resource {
                id: id(input),
                kind: Kind::Item,
            },
            Resource {
                id: id(output),
                kind: Kind::Item,
            },
        ],
        recipes: vec![Recipe {
            id: id("reference:compress_snow_block"),
            machine: id(MACHINE),
            duration_ticks: 300,
            eu_per_tick: 2,
            inputs: vec![amount(input, 1)],
            outputs: vec![amount(output, 1)],
        }],
    })
    .unwrap()
}

fn limits(items: u64) -> Limits {
    Limits {
        items,
        fluid_mb: 0,
        kinds: 4,
    }
}

fn compressor(_catalog: &Catalog) -> Compressor {
    Compressor::new(id(MACHINE), limits(64), limits(64)).unwrap()
}

#[derive(Debug, Deserialize)]
struct Fixture {
    schema_version: u32,
    recipe: FixtureRecipe,
    scenarios: Vec<Scenario>,
}

#[derive(Debug, Deserialize)]
struct FixtureRecipe {
    input: String,
    output: String,
}

#[derive(Debug, Deserialize)]
struct Scenario {
    id: String,
    initial_input_count: u64,
    initial_output_count: u64,
    actions: Vec<Action>,
    samples: Vec<[i64; 8]>,
}

#[derive(Debug, Deserialize)]
struct Action {
    after_machine_tick: i64,
    kind: String,
    eu: Option<u64>,
    count: Option<u64>,
}

fn make_scenario_machine(catalog: &Catalog, scenario: &Scenario, input: &str, output: &str) -> Compressor {
    let mut machine = compressor(catalog);
    machine
        .edit_inputs(catalog, &[], &[amount(input, scenario.initial_input_count)])
        .unwrap();
    if scenario.initial_output_count != 0 {
        machine
            .edit_outputs(
                catalog,
                &[],
                &[amount(output, scenario.initial_output_count)],
            )
            .unwrap();
    }
    machine
}

fn apply_action(machine: &mut Compressor, catalog: &Catalog, action: &Action, output: &str) {
    match action.kind.as_str() {
        "fixture_charge" => machine.fixture_charge(action.eu.unwrap()).unwrap(),
        "fixture_output_clear" => machine
            .edit_outputs(
                catalog,
                &[amount(output, action.count.unwrap())],
                &[],
            )
            .unwrap(),
        other => panic!("unsupported fixture action {other}"),
    }
}

fn assert_sample(machine: &Compressor, row: &[i64; 8], input: &str, output: &str) {
    let observed = [
        machine.stored_eu() as i64,
        i64::from(machine.progress()),
        i64::from(machine.max_progress()),
        machine.eu_per_tick() as i64,
        machine.inputs().amount(&id(input)) as i64,
        machine.outputs().amount(&id(output)) as i64,
        machine.pending_output_count() as i64,
    ];
    assert_eq!(observed.as_slice(), &row[1..], "machine tick {}", row[0]);
}

#[test]
fn fixture_replays_all_samples_and_round_trips_each_sample_boundary() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(fixture.schema_version, 1);
    assert_eq!(fixture.scenarios.len(), 3);
    let catalog = catalog();
    let input = fixture.recipe.input.as_str();
    let output = fixture.recipe.output.as_str();
    let mut total_samples = 0;

    for scenario in &fixture.scenarios {
        let mut uninterrupted = make_scenario_machine(&catalog, scenario, input, output);
        let mut restored = uninterrupted.clone();
        let mut next_action = 0;
        let mut previous_tick = None;

        for row in &scenario.samples {
            if let Some(previous) = previous_tick {
                assert_eq!(row[0], previous + 1, "{} sample order", scenario.id);
            }
            previous_tick = Some(row[0]);
            uninterrupted.tick(&catalog).unwrap();
            restored.tick(&catalog).unwrap();
            assert_sample(&uninterrupted, row, input, output);
            assert_sample(&restored, row, input, output);
            assert_eq!(restored, uninterrupted, "{} at tick {}", scenario.id, row[0]);
            total_samples += 1;

            while let Some(action) = scenario.actions.get(next_action) {
                if action.after_machine_tick != row[0] {
                    break;
                }
                apply_action(&mut uninterrupted, &catalog, action, output);
                apply_action(&mut restored, &catalog, action, output);
                next_action += 1;
            }

            let bytes = restored.encode(&catalog).unwrap();
            restored = Compressor::decode(&bytes, &catalog).unwrap();
            assert_eq!(restored, uninterrupted, "round trip {} tick {}", scenario.id, row[0]);
        }

        assert_eq!(next_action, scenario.actions.len(), "{} actions applied", scenario.id);
        assert_eq!(uninterrupted.completed_batches(), 1, "{} output", scenario.id);
        assert_eq!(uninterrupted.inputs().amount(&id(input)), 4, "{} remaining input", scenario.id);
        assert_eq!(uninterrupted.outputs().amount(&id(output)), 1, "{} output count", scenario.id);
        assert_eq!(uninterrupted.received_eu(), match scenario.id.as_str() {
            "nominal" | "blocked_output" => 600,
            "shortage" => 856,
            other => panic!("unexpected scenario {other}"),
        });
        assert_eq!(uninterrupted.spent_eu(), uninterrupted.received_eu());
        assert_eq!(uninterrupted.stored_eu(), 0);
        assert_eq!(
            uninterrupted.wasted_work_eu(),
            if scenario.id == "shortage" { 256 } else { 0 }
        );
    }
    assert_eq!(total_samples, 1204);
}

#[test]
fn intake_is_exactly_256_eu_and_does_not_spend_on_the_intake_tick() {
    let catalog = catalog();
    let mut machine = compressor(&catalog);
    machine
        .edit_inputs(&catalog, &[], &[amount(INPUT, 4)])
        .unwrap();
    assert_eq!(EU_CAPACITY, 2048);
    assert_eq!(INTAKE_THRESHOLD, 256);

    machine.fixture_charge(255).unwrap();
    assert_eq!(machine.tick(&catalog).unwrap().state, TickState::WaitingForEnergy);
    assert_eq!(machine.inputs().amount(&id(INPUT)), 4);
    assert_eq!(machine.progress(), 0);
    assert_eq!(machine.stored_eu(), 255);

    machine.fixture_charge(1).unwrap();
    assert_eq!(machine.tick(&catalog).unwrap().state, TickState::Intake);
    assert_eq!(machine.inputs().amount(&id(INPUT)), 0);
    assert_eq!(machine.pending_output_count(), 1);
    assert_eq!(machine.progress(), 0);
    assert_eq!(machine.stored_eu(), 256);
    assert_eq!(machine.spent_eu(), 0);
}

#[test]
fn one_item_recipe_keeps_its_held_recipe_across_save_and_completes_once() {
    let catalog = one_item_catalog();
    let input = id("reference:snow_block");
    let output = id("reference:ice");
    let recipe = catalog.recipe(&id("reference:compress_snow_block")).unwrap();
    let mut machine = compressor(&catalog);
    machine
        .edit_inputs(&catalog, &[], &[amount(input.as_str(), 5)])
        .unwrap();
    machine.fixture_charge(600).unwrap();

    assert_eq!(machine.tick(&catalog).unwrap().state, TickState::Intake);
    assert_eq!(machine.inputs().amount(&input), 4);
    assert_eq!(machine.stored_eu(), 600);
    assert_eq!(machine.spent_eu(), 0);
    assert_eq!(machine.pending_output_count(), 1);
    assert_eq!(machine.active_recipe(), Some(recipe));

    let restored = Compressor::decode(&machine.encode(&catalog).unwrap(), &catalog).unwrap();
    assert_eq!(restored, machine);
    assert_eq!(restored.active_recipe(), Some(recipe));
    assert_eq!(restored.inputs().amount(&input), 4);

    let mut machine = restored;
    for _ in 0..299 {
        assert_eq!(machine.tick(&catalog).unwrap().state, TickState::Powered);
    }
    assert_eq!(machine.tick(&catalog).unwrap().state, TickState::Completed);
    assert_eq!(machine.stored_eu(), 0);
    assert_eq!(machine.received_eu(), 600);
    assert_eq!(machine.spent_eu(), 600);
    assert_eq!(machine.wasted_work_eu(), 0);
    assert_eq!(machine.completed_batches(), 1);
    assert_eq!(machine.inputs().amount(&input), 4);
    assert_eq!(machine.outputs().amount(&output), 1);
    assert_eq!(machine.pending_output_count(), 0);
    assert_eq!(machine.active_recipe(), None);
}

#[test]
fn failed_drain_resets_progress_and_runs_one_hundred_free_cooldown_ticks() {
    let catalog = catalog();
    let mut machine = compressor(&catalog);
    machine
        .edit_inputs(&catalog, &[], &[amount(INPUT, 4)])
        .unwrap();
    machine.fixture_charge(256).unwrap();
    assert_eq!(machine.tick(&catalog).unwrap().state, TickState::Intake);
    for _ in 0..128 {
        assert_eq!(machine.tick(&catalog).unwrap().energy_used, 2);
    }
    assert_eq!(machine.stored_eu(), 0);
    assert_eq!(machine.progress(), 128);

    assert_eq!(machine.tick(&catalog).unwrap().state, TickState::ShortageReset);
    assert_eq!(machine.progress(), -100);
    assert_eq!(machine.pending_output_count(), 1);
    assert_eq!(machine.wasted_work_eu(), 256);
    assert_eq!(machine.inputs().amount(&id(INPUT)), 0);

    for expected in -99..=0 {
        assert_eq!(machine.tick(&catalog).unwrap().state, TickState::Cooldown);
        assert_eq!(machine.progress(), expected);
        assert_eq!(machine.stored_eu(), 0);
    }
    assert_eq!(machine.progress(), 0);
    assert_eq!(machine.pending_output_count(), 1);

    machine.fixture_charge(600).unwrap();
    assert_eq!(machine.tick(&catalog).unwrap().state, TickState::Powered);
    assert_eq!(machine.progress(), 1);
    assert_eq!(machine.spent_eu(), 258);
}

#[test]
fn full_output_blocks_intake_and_active_output_insertion_is_rejected_atomically() {
    let catalog = catalog();
    let mut machine = compressor(&catalog);
    machine
        .edit_inputs(&catalog, &[], &[amount(INPUT, 4)])
        .unwrap();
    machine
        .edit_outputs(&catalog, &[], &[amount(OUTPUT, 64)])
        .unwrap();
    machine.fixture_charge(600).unwrap();
    assert_eq!(machine.tick(&catalog).unwrap().state, TickState::OutputBlocked);
    assert_eq!(machine.inputs().amount(&id(INPUT)), 4);
    assert_eq!(machine.stored_eu(), 600);
    assert_eq!(machine.pending_output_count(), 0);

    machine
        .edit_outputs(&catalog, &[amount(OUTPUT, 64)], &[])
        .unwrap();
    assert_eq!(machine.tick(&catalog).unwrap().state, TickState::Intake);
    let before = machine.encode(&catalog).unwrap();
    assert!(machine
        .edit_outputs(&catalog, &[], &[amount(OUTPUT, 1)])
        .is_err());
    assert_eq!(machine.encode(&catalog).unwrap(), before);
}

#[test]
fn unsupported_selected_recipe_declarations_fail_without_partial_mutation() {
    let base = Content {
        schema_version: 1,
        provenance: "unsupported shape test".into(),
        resources: vec![
            Resource { id: id(INPUT), kind: Kind::Item },
            Resource { id: id(OUTPUT), kind: Kind::Item },
            Resource { id: id("reference:dust"), kind: Kind::Item },
        ],
        recipes: vec![Recipe {
            id: id("reference:compress_sand"),
            machine: id(MACHINE),
            duration_ticks: 300,
            eu_per_tick: 2,
            inputs: vec![amount(INPUT, 4)],
            outputs: vec![amount(OUTPUT, 1)],
        }],
    };
    let mut cases = Vec::new();
    let mut value = base.clone();
    value.recipes[0].duration_ticks = 301;
    cases.push((value, vec![amount(INPUT, 4)]));
    let mut value = base.clone();
    value.recipes[0].eu_per_tick = 3;
    cases.push((value, vec![amount(INPUT, 4)]));
    let mut value = base.clone();
    value.recipes[0].inputs = vec![amount(INPUT, 2), amount("reference:dust", 2)];
    cases.push((
        value,
        vec![amount(INPUT, 2), amount("reference:dust", 2)],
    ));
    let mut value = base.clone();
    value.recipes[0].outputs[0].quantity = 2;
    cases.push((value, vec![amount(INPUT, 4)]));
    let mut value = base;
    value.resources.push(Resource {
        id: id("reference:fluid"),
        kind: Kind::Fluid,
    });
    value.recipes[0].outputs[0].resource = id("reference:fluid");
    cases.push((value, vec![amount(INPUT, 4)]));

    for (content, input_amounts) in cases {
        let unsupported = Catalog::new(content).unwrap();
        let mut machine = compressor(&unsupported);
        machine
            .edit_inputs(&unsupported, &[], &input_amounts)
            .unwrap();
        machine.fixture_charge(600).unwrap();
        let before = machine.encode(&unsupported).unwrap();
        assert!(machine.tick(&unsupported).is_err());
        assert_eq!(machine.encode(&unsupported).unwrap(), before);
    }
}

#[test]
fn catalog_selection_is_dynamic_and_active_save_binds_the_exact_recipe() {
    let mut content = Content {
        schema_version: 1,
        provenance: "selection test".into(),
        resources: vec![
            Resource { id: id(INPUT), kind: Kind::Item },
            Resource { id: id(OUTPUT), kind: Kind::Item },
            Resource { id: id("reference:stone"), kind: Kind::Item },
            Resource { id: id("reference:brick"), kind: Kind::Item },
        ],
        recipes: vec![
            Recipe {
                id: id("reference:compress_sand"),
                machine: id(MACHINE),
                duration_ticks: 300,
                eu_per_tick: 2,
                inputs: vec![amount(INPUT, 4)],
                outputs: vec![amount(OUTPUT, 1)],
            },
            Recipe {
                id: id("reference:compress_stone"),
                machine: id(MACHINE),
                duration_ticks: 300,
                eu_per_tick: 2,
                inputs: vec![amount("reference:stone", 4)],
                outputs: vec![amount("reference:brick", 1)],
            },
        ],
    };
    let catalog = Catalog::new(content.clone()).unwrap();
    let mut machine = compressor(&catalog);
    machine
        .edit_inputs(&catalog, &[], &[amount("reference:stone", 4)])
        .unwrap();
    machine.fixture_charge(256).unwrap();
    assert_eq!(machine.tick(&catalog).unwrap().state, TickState::Intake);
    assert_eq!(machine.active_recipe().unwrap().id.as_str(), "reference:compress_stone");
    let bytes = machine.encode(&catalog).unwrap();
    let restored = Compressor::decode(&bytes, &catalog).unwrap();
    assert_eq!(restored, machine);

    content.recipes[1].outputs[0].quantity = 2;
    let changed = Catalog::new(content).unwrap();
    assert!(Compressor::decode(&bytes, &changed).is_err());
}

#[test]
fn malformed_versions_progress_capacity_and_ledger_saves_are_rejected() {
    let catalog = catalog();
    let mut machine = compressor(&catalog);
    machine
        .edit_inputs(&catalog, &[], &[amount(INPUT, 4)])
        .unwrap();
    machine.fixture_charge(256).unwrap();
    machine.tick(&catalog).unwrap();
    let bytes = machine.encode(&catalog).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(original["policy_version"], POLICY_VERSION);

    for (path, value) in [
        (vec!["schema_version"], serde_json::json!(99)),
        (vec!["policy_version"], serde_json::json!(99)),
        (vec!["stored_eu"], serde_json::json!(EU_CAPACITY + 1)),
        (vec!["active", "progress"], serde_json::json!(300)),
        (vec!["active", "pending_output_count"], serde_json::json!(0)),
    ] {
        let mut value_json = original.clone();
        let mut cursor = &mut value_json;
        for segment in &path[..path.len() - 1] {
            cursor = &mut cursor[*segment];
        }
        cursor[*path.last().unwrap()] = value;
        let malformed = serde_json::to_vec(&value_json).unwrap();
        assert!(Compressor::decode(&malformed, &catalog).is_err(), "{path:?}");
    }

    let mut bad_ledger = original.clone();
    bad_ledger["received_eu"] = serde_json::json!(0);
    assert!(Compressor::decode(&serde_json::to_vec(&bad_ledger).unwrap(), &catalog).is_err());
    let mut unknown = original;
    unknown["unexpected"] = serde_json::json!(1);
    assert!(Compressor::decode(&serde_json::to_vec(&unknown).unwrap(), &catalog).is_err());
}

#[test]
fn charge_and_inventory_errors_do_not_partially_mutate_state() {
    let catalog = catalog();
    let mut machine = compressor(&catalog);
    machine
        .edit_inputs(&catalog, &[], &[amount(INPUT, 4)])
        .unwrap();
    machine.fixture_charge(2048).unwrap();
    let before = machine.encode(&catalog).unwrap();
    assert!(machine.fixture_charge(1).is_err());
    assert_eq!(machine.encode(&catalog).unwrap(), before);
    assert!(machine
        .edit_inputs(&catalog, &[], &[amount(INPUT, 61)])
        .is_err());
    assert_eq!(machine.encode(&catalog).unwrap(), before);

    let mut exhausted: serde_json::Value = serde_json::from_slice(&before).unwrap();
    exhausted["stored_eu"] = serde_json::json!(0);
    exhausted["received_eu"] = serde_json::json!(u64::MAX - 1);
    exhausted["spent_eu"] = serde_json::json!(u64::MAX - 1);
    let completed = (u64::MAX - 1) / 600;
    let wasted = (u64::MAX - 1) - completed * 600;
    exhausted["completed_batches"] = serde_json::json!(completed);
    exhausted["wasted_work_eu"] = serde_json::json!(wasted);
    let mut machine = Compressor::decode(&serde_json::to_vec(&exhausted).unwrap(), &catalog).unwrap();
    let before = machine.encode(&catalog).unwrap();
    assert!(machine.fixture_charge(EU_CAPACITY).is_err());
    assert_eq!(machine.encode(&catalog).unwrap(), before);
}
