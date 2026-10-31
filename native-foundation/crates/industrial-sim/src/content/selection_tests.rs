use super::*;
use crate::inventory::{Inventory, Limits};
fn id(name: &str) -> Id {
    Id::new(format!("test:{name}")).unwrap()
}
fn resource(name: &str) -> Resource {
    Resource {
        id: id(name),
        kind: Kind::Item,
    }
}
fn recipe(name: &str, machine: &str, inputs: &[(&str, u64)]) -> Recipe {
    Recipe {
        id: id(name),
        machine: id(machine),
        duration_ticks: 1,
        eu_per_tick: 8,
        inputs: inputs
            .iter()
            .map(|&(s, n)| Amount {
                resource: id(s),
                quantity: n,
            })
            .collect(),
        outputs: vec![Amount {
            resource: id("output"),
            quantity: 1,
        }],
    }
}
fn inventory(catalog: &Catalog, amounts: &[(&str, u64)]) -> Inventory {
    let mut inv = Inventory::new(Limits {
        items: 100_000,
        fluid_mb: 100_000,
        kinds: 64,
    })
    .unwrap();
    let add = amounts
        .iter()
        .map(|&(s, n)| Amount {
            resource: id(s),
            quantity: n,
        })
        .collect::<Vec<_>>();
    inv.apply(catalog, &[], &add).unwrap();
    inv
}
fn naive<'a>(catalog: &'a Catalog, machine: &Id, inputs: &Inventory) -> Option<&'a Recipe> {
    catalog.recipes().find(|r| {
        &r.machine == machine
            && r.inputs
                .iter()
                .all(|a| inputs.amount(&a.resource) >= a.quantity)
    })
}
#[test]
fn rare_required_anchors_skip_unrelated_recipes_and_absent_required_resources() {
    let mut resources = vec![resource("water"), resource("output")];
    let mut recipes = Vec::new();
    for n in 0..1000 {
        let name = format!("ore{n:04}");
        resources.push(resource(&name));
        recipes.push(recipe(
            &format!("r{n:04}"),
            "grinder",
            &[(&name, 1), ("water", 1)],
        ));
    }
    let c = Catalog::new(Content {
        schema_version: 1,
        provenance: "synthetic-index".into(),
        resources,
        recipes,
    })
    .unwrap();
    let only_common = inventory(&c, &[("water", 100)]);
    let selected = c.select(&id("grinder"), &only_common).unwrap();
    assert!(selected.recipe.is_none());
    assert_eq!(selected.candidates_examined, 0);
    let inputs = inventory(&c, &[("water", 100), ("ore0777", 1)]);
    let selected = c.select(&id("grinder"), &inputs).unwrap();
    assert_eq!(selected.recipe.unwrap().id, id("r0777"));
    assert_eq!(selected.candidates_examined, 1);
    assert_eq!(selected.streams, 1);
}
#[test]
fn lowest_id_quantity_and_machine_selection_match_existing_order() {
    let mut content = Content {
        schema_version: 1,
        provenance: "synthetic-index".into(),
        resources: vec![
            resource("ore"),
            resource("dust"),
            resource("water"),
            resource("output"),
        ],
        recipes: vec![
            recipe("a", "grinder", &[("dust", 99), ("water", 2)]),
            recipe("b", "grinder", &[("ore", 1), ("water", 2)]),
            recipe("z", "grinder", &[("ore", 1)]),
            recipe("aa", "smelter", &[("ore", 1)]),
        ],
    };
    let c = Catalog::new(content.clone()).unwrap();
    content.recipes.reverse();
    content.resources.reverse();
    let reordered = Catalog::new(content).unwrap();
    let inputs = inventory(&c, &[("ore", 1), ("dust", 1), ("water", 2)]);
    for catalog in [&c, &reordered] {
        let selected = catalog.select(&id("grinder"), &inputs).unwrap();
        assert_eq!(selected.recipe.unwrap().id, id("b"));
        assert_eq!(selected.recipe, naive(catalog, &id("grinder"), &inputs));
        assert_eq!(
            catalog
                .select(&id("smelter"), &inputs)
                .unwrap()
                .recipe
                .unwrap()
                .id,
            id("aa")
        );
        assert!(
            catalog
                .select(&id("missing"), &inputs)
                .unwrap()
                .recipe
                .is_none()
        );
    }
    let empty = inventory(&c, &[]);
    assert!(c.select(&id("grinder"), &empty).unwrap().recipe.is_none());
}
#[test]
fn each_recipe_occurs_once_even_when_many_inputs_are_present() {
    let recipes = (0..100)
        .map(|n| recipe(&format!("r{n:04}"), "grinder", &[("ore", 2), ("water", 2)]))
        .collect();
    let c = Catalog::new(Content {
        schema_version: 1,
        provenance: "synthetic-index".into(),
        resources: vec![resource("ore"), resource("water"), resource("output")],
        recipes,
    })
    .unwrap();
    let inputs = inventory(&c, &[("ore", 1), ("water", 1)]);
    let selected = c.select(&id("grinder"), &inputs).unwrap();
    assert!(selected.recipe.is_none());
    assert_eq!(selected.candidates_examined, 100);
    assert_eq!(selected.streams, 1);
}
#[test]
fn maximum_inventory_kinds_bound_candidate_merge_scratch_and_keep_id_order() {
    let mut resources = vec![resource("output")];
    let mut recipes = Vec::new();
    let mut names = Vec::new();
    for n in 0..64 {
        let name = format!("ore{n:02}");
        resources.push(resource(&name));
        recipes.push(recipe(&format!("r{n:02}"), "grinder", &[(&name, 2)]));
        names.push(name);
    }
    let c = Catalog::new(Content {
        schema_version: 1,
        provenance: "synthetic-index".into(),
        resources,
        recipes,
    })
    .unwrap();
    let amounts = names.iter().map(|s| (s.as_str(), 1)).collect::<Vec<_>>();
    let inputs = inventory(&c, &amounts);
    let selected = c.select(&id("grinder"), &inputs).unwrap();
    assert_eq!(selected.streams, 64);
    assert_eq!(selected.candidates_examined, 64);
    assert!(selected.recipe.is_none());
    let mut corrupted: serde_json::Value = serde_json::to_value(inputs).unwrap();
    corrupted["limits"]["items"] = serde_json::json!(1);
    let invalid: Inventory = serde_json::from_value(corrupted).unwrap();
    assert!(c.select(&id("grinder"), &invalid).is_err());
}
#[test]
fn two_thousand_seeded_queries_match_a_naive_full_catalog_scan() {
    fn next(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }
    let mut seed = 0x8742_1234_0056u64;
    let mut resources = vec![resource("output")];
    for n in 0..32 {
        resources.push(resource(&format!("ore{n:02}")));
    }
    let mut recipes = Vec::new();
    for n in 0..500 {
        let mut inputs = BTreeMap::new();
        for _ in 0..3 {
            inputs.insert(
                format!("ore{:02}", next(&mut seed) % 32),
                1 + next(&mut seed) % 16,
            );
        }
        let input = inputs
            .iter()
            .map(|(s, &n)| (s.as_str(), n))
            .collect::<Vec<_>>();
        recipes.push(recipe(
            &format!("r{n:04}"),
            &format!("m{}", next(&mut seed) % 8),
            &input,
        ));
    }
    let c = Catalog::new(Content {
        schema_version: 1,
        provenance: "synthetic-index".into(),
        resources,
        recipes,
    })
    .unwrap();
    for scenario in 0..2000 {
        let machine = id(&format!("m{}", next(&mut seed) % 10));
        let mut amounts = Vec::new();
        for n in 0..32 {
            let count = next(&mut seed) % 24;
            if count > 0 {
                amounts.push((format!("ore{n:02}"), count));
            }
        }
        let inputs = inventory(
            &c,
            &amounts
                .iter()
                .map(|(s, n)| (s.as_str(), *n))
                .collect::<Vec<_>>(),
        );
        let selected = c.select(&machine, &inputs).unwrap();
        assert_eq!(
            selected.recipe,
            naive(&c, &machine, &inputs),
            "scenario {scenario}"
        );
        assert!(selected.streams <= 64 && selected.candidates_examined <= 500);
    }
}
