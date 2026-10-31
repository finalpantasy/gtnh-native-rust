//! Source-declared recipe parameters, with explicit internal symbol aliases.
use industrial_sim::{
    content::{Amount, Catalog, Recipe},
    inventory::{Inventory, Limits},
    machine::{Energy, Machine, State, Tick},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
type Error = Box<dyn std::error::Error>;
const CONTENT: &[u8] = include_bytes!("../../../fixtures/compressor-reference-v1.json");
fn checked_tick(
    machine: &mut Machine,
    delivered: &Inventory,
    recipe: &Recipe,
    catalog: &Catalog,
    received: u64,
    spent: &mut u64,
) -> Result<Tick, Error> {
    let report = machine.tick(catalog)?;
    *spent += report.energy_used;
    let input = &recipe.inputs[0];
    let output = &recipe.outputs[0];
    let held: u64 = machine.held_inputs().iter().map(|a| a.quantity).sum();
    assert_eq!(
        machine.inputs().amount(&input.resource)
            + held
            + (machine.outputs().amount(&output.resource) + delivered.amount(&output.resource))
                * input.quantity,
        3 * input.quantity
    );
    assert_eq!(machine.energy().stored() + *spent, received);
    Ok(report)
}
fn exercise(recipe: &Recipe, catalog: &Catalog) -> Result<Value, Error> {
    assert_eq!(
        (
            recipe.inputs.len(),
            recipe.outputs.len(),
            recipe.outputs[0].quantity
        ),
        (1, 1, 1)
    );
    assert_eq!((recipe.duration_ticks, recipe.eu_per_tick), (300, 2));
    let input = &recipe.inputs[0];
    let output = &recipe.outputs[0];
    let mut machine = Machine::new(
        recipe.machine.clone(),
        Limits {
            items: 12,
            fluid_mb: 0,
            kinds: 1,
        },
        Limits {
            items: 1,
            fluid_mb: 0,
            kinds: 1,
        },
        Energy::new(4096, 8, 1)?,
    )?;
    machine.inputs_mut().apply(
        catalog,
        &[],
        &[Amount {
            resource: input.resource.clone(),
            quantity: 3 * input.quantity,
        }],
    )?;
    let mut delivered = Inventory::new(Limits {
        items: 3,
        fluid_mb: 0,
        kinds: 1,
    })?;
    let (mut received, mut spent) = (0, 0);
    for _ in 0..2 {
        received += machine.energy_mut().receive(8, 1)?;
    }
    for _ in 0..8 {
        assert_eq!(
            checked_tick(
                &mut machine,
                &delivered,
                recipe,
                catalog,
                received,
                &mut spent
            )?
            .state,
            State::Running
        );
    }
    let before_pause = machine.clone();
    assert_eq!(
        checked_tick(
            &mut machine,
            &delivered,
            recipe,
            catalog,
            received,
            &mut spent
        )?
        .state,
        State::NoPower
    );
    assert_eq!(machine, before_pause);
    let save = machine.encode(catalog)?;
    machine = Machine::decode(&save, catalog)?;
    assert_eq!(machine, before_pause);
    assert_eq!(machine.progress(), Some(8));
    let mut changed: Value = serde_json::from_slice(CONTENT)?;
    let target = changed["recipes"]
        .as_array_mut()
        .ok_or("missing recipe array")?
        .iter_mut()
        .find(|r| r["id"] == recipe.id.as_str())
        .ok_or("missing changed recipe")?;
    target["duration_ticks"] = json!(301);
    let changed_catalog = Catalog::from_json(&serde_json::to_vec(&changed)?)?;
    assert!(Machine::decode(&save, &changed_catalog).is_err());
    for _ in 0..512 {
        if machine.energy().stored() == 4096 {
            break;
        }
        received += machine.energy_mut().receive(8, 1)?;
    }
    assert_eq!(machine.energy().stored(), 4096);
    for _ in 0..300 {
        if machine.completed() == 1 {
            break;
        }
        checked_tick(
            &mut machine,
            &delivered,
            recipe,
            catalog,
            received,
            &mut spent,
        )?;
    }
    assert_eq!(machine.completed(), 1);
    let before_block = machine.clone();
    for _ in 0..20 {
        assert_eq!(
            checked_tick(
                &mut machine,
                &delivered,
                recipe,
                catalog,
                received,
                &mut spent
            )?
            .state,
            State::OutputBlocked
        );
        assert_eq!(machine, before_block);
    }
    for _ in 0..1000 {
        machine
            .outputs_mut()
            .transfer(&mut delivered, catalog, &output.resource, 1)?;
        if machine.completed() == 3 {
            break;
        }
        checked_tick(
            &mut machine,
            &delivered,
            recipe,
            catalog,
            received,
            &mut spent,
        )?;
    }
    assert_eq!(machine.completed(), 3);
    assert_eq!(machine.inputs().amount(&input.resource), 0);
    assert_eq!(machine.outputs().amount(&output.resource), 0);
    assert!(machine.held_inputs().is_empty());
    assert_eq!(delivered.amount(&output.resource), 3);
    assert_eq!(spent, 1800);
    assert_eq!(received, spent + machine.energy().stored());
    Ok(
        json!({"recipe":recipe.id,"input_symbol_alias":input.resource,"output_symbol_alias":output.resource,
        "input_quantity_per_job":input.quantity,"initial_inputs":3*input.quantity,"delivered_outputs":3,
        "duration_ticks":300,"eu_per_tick":2,"completed":3,"energy_consumed":spent,"energy_received":received,
        "ending_energy":machine.energy().stored(),"blocked_ticks":20,"power_pause_checks":1,
        "active_reopen_progress":8,"changed_content_rejected":true}),
    )
}
pub fn run() -> Result<Value, Error> {
    let catalog = Catalog::from_json(CONTENT)?;
    let cases = catalog
        .recipes()
        .map(|r| exercise(r, &catalog))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(cases.len(), 6);
    Ok(
        json!({"status":"PASS","content_sha256":format!("{:x}",Sha256::digest(CONTENT)),"cases":cases,
        "completed_jobs":18,"energy_consumed":10800,
        "scope":"Six source-declared compressor parameter cases with explicit internal source-symbol aliases; native simulator energy/port policy and codec reopen. No actual GTNH registry, item identity, container, overclocking or filesystem-integration claim from this route."}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declared_recipes_preserve_batches_work_energy_pressure_and_active_save_identity() {
        let result = run().unwrap();
        assert_eq!(result["cases"].as_array().unwrap().len(), 6);
        assert_eq!(result["completed_jobs"], 18);
        assert_eq!(result["energy_consumed"], 10800);
        for row in result["cases"].as_array().unwrap() {
            assert_eq!(row["duration_ticks"], 300);
            assert_eq!(row["eu_per_tick"], 2);
            assert_eq!(row["completed"], 3);
            assert_eq!(row["energy_consumed"], 1800);
            assert_eq!(row["delivered_outputs"], 3);
            assert_eq!(row["blocked_ticks"], 20);
            assert_eq!(row["power_pause_checks"], 1);
            assert_eq!(row["active_reopen_progress"], 8);
            assert_eq!(row["changed_content_rejected"], true);
        }
    }
}
