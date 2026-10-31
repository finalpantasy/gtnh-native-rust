//! Synthetic recipe selection measurement. No pack recipes or game performance claims.
use industrial_sim::{
    content::{Amount, Catalog, Content, Id, Kind, Recipe, Resource},
    inventory::{Inventory, Limits},
};
use serde_json::{Value, json};
use std::{
    io::{self, Write},
    time::Instant,
};
type Error = Box<dyn std::error::Error>;
fn id(name: &str) -> Id {
    Id::new(format!("bench:{name}")).unwrap()
}
pub fn run(
    recipes: usize,
    queries: usize,
    mut checkpoint: impl FnMut(Value) -> Result<(), Error>,
) -> Result<Value, Error> {
    if !(1..=99_998).contains(&recipes) || !(1..=10_000).contains(&queries) {
        return Err("recipe benchmark bounds exceeded".into());
    }
    checkpoint(json!({"phase":"initial","recipes":recipes,"queries":queries}))?;
    let start = Instant::now();
    let mut resources = vec![
        Resource {
            id: id("water"),
            kind: Kind::Fluid,
        },
        Resource {
            id: id("output"),
            kind: Kind::Item,
        },
    ];
    let mut definitions = Vec::with_capacity(recipes);
    for n in 0..recipes {
        let ore = id(&format!("ore{n:06}"));
        resources.push(Resource {
            id: ore.clone(),
            kind: Kind::Item,
        });
        definitions.push(Recipe {
            id: id(&format!("r{n:06}")),
            machine: id(&format!("m{}", n % 64)),
            duration_ticks: 5,
            eu_per_tick: 8,
            inputs: vec![
                Amount {
                    resource: ore,
                    quantity: 1,
                },
                Amount {
                    resource: id("water"),
                    quantity: 1,
                },
            ],
            outputs: vec![Amount {
                resource: id("output"),
                quantity: 1,
            }],
        });
    }
    let generation_ms = start.elapsed().as_secs_f64() * 1000.;
    checkpoint(json!({"phase":"content-generated","recipes":recipes}))?;
    let start = Instant::now();
    let catalog = Catalog::new(Content {
        schema_version: 1,
        provenance: "Synthetic unique ore plus common water; not GTNH data".into(),
        resources,
        recipes: definitions,
    })?;
    let construction_ms = start.elapsed().as_secs_f64() * 1000.;
    checkpoint(
        json!({"phase":"catalog-indexed","recipes":recipes,"construction_ms":construction_ms}),
    )?;
    let mut seed = 0x0123_4567_9876u64;
    let mut cases = Vec::with_capacity(queries);
    for _ in 0..queries {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let n = seed as usize % recipes;
        let mut inventory = Inventory::new(Limits {
            items: 1,
            fluid_mb: 1,
            kinds: 2,
        })?;
        inventory.apply(
            &catalog,
            &[],
            &[
                Amount {
                    resource: id(&format!("ore{n:06}")),
                    quantity: 1,
                },
                Amount {
                    resource: id("water"),
                    quantity: 1,
                },
            ],
        )?;
        cases.push((
            id(&format!("m{}", n % 64)),
            inventory,
            id(&format!("r{n:06}")),
        ));
    }
    checkpoint(json!({"phase":"queries-prepared","queries":queries}))?;
    for (machine, inventory, expected) in cases.iter().take(64) {
        assert_eq!(
            &catalog
                .select(machine, inventory)?
                .recipe
                .ok_or("warmup missed recipe")?
                .id,
            expected
        );
    }
    let mut candidates = 0usize;
    let mut streams = 0usize;
    let start = Instant::now();
    for (machine, inventory, expected) in &cases {
        let selected = catalog.select(machine, inventory)?;
        candidates += selected.candidates_examined;
        streams += selected.streams;
        assert_eq!(
            &selected.recipe.ok_or("indexed lookup missed recipe")?.id,
            expected
        );
    }
    let indexed_ms = start.elapsed().as_secs_f64() * 1000.;
    checkpoint(
        json!({"phase":"indexed-queried","candidates":candidates,"milliseconds":indexed_ms}),
    )?;
    let start = Instant::now();
    let mut naive_candidates = 0usize;
    for (machine, inventory, expected) in &cases {
        let selected = catalog
            .recipes()
            .find(|recipe| {
                naive_candidates += 1;
                &recipe.machine == machine
                    && recipe
                        .inputs
                        .iter()
                        .all(|a| inventory.amount(&a.resource) >= a.quantity)
            })
            .ok_or("naive lookup missed recipe")?;
        assert_eq!(&selected.id, expected);
    }
    let naive_ms = start.elapsed().as_secs_f64() * 1000.;
    assert_eq!(candidates, queries);
    assert_eq!(streams, queries);
    if recipes > 1000 {
        assert!(naive_candidates > candidates * 100);
    }
    checkpoint(
        json!({"phase":"naive-queried","candidates":naive_candidates,"milliseconds":naive_ms}),
    )?;
    Ok(
        json!({"status":"PASS","recipes":recipes,"resources":recipes+2,"machine_types":recipes.min(64),"queries":queries,"oracle_assertions":queries*2,"candidate_streams":streams,"indexed_candidates":candidates,"naive_candidates":naive_candidates,
        "generation_ms":generation_ms,"construction_ms":construction_ms,"indexed_ms":indexed_ms,"naive_ms":naive_ms,"observed_query_speedup":if indexed_ms>0. {Some(naive_ms/indexed_ms)} else {None},
        "scope":"One local optimized-process sample, prepared inventories, unique rare anchors plus common water, exact naive oracle; no universal complexity, GTNH recipe parity or full-game frame claim. Worst-case common anchors can still scan per-machine recipes."}),
    )
}
pub fn execute(args: &[String]) -> Result<(), Error> {
    if args.len() != 2 {
        return Err("recipe-probe requires recipe count and query count".into());
    }
    let result = run(args[0].parse()?, args[1].parse()?, |row| {
        println!("{row}");
        io::stdout().flush()?;
        let mut response = String::new();
        if io::stdin().read_line(&mut response)? == 0 || response.trim() != "sampled" {
            return Err("recipe probe sampler did not acknowledge checkpoint".into());
        }
        Ok(())
    })?;
    println!(
        "{}",
        json!({"phase":"done","status":"PASS","result":result})
    );
    Ok(())
}
