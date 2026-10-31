use crate::Error;
use serde::{Deserialize, Serialize};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, BinaryHeap},
    fmt,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Id(String);
impl Id {
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        let Some((namespace, path)) = value.split_once(':') else {
            return Err("ID requires a namespace".into());
        };
        let valid = |part: &str| {
            !part.is_empty()
                && part.as_bytes()[0].is_ascii_lowercase()
                && part.bytes().all(|b| {
                    b.is_ascii_lowercase()
                        || b.is_ascii_digit()
                        || b == b'_'
                        || b == b'-'
                        || b == b'.'
                })
        };
        if value.len() > 128 || !valid(namespace) || !valid(path) {
            return Err("invalid content ID".into());
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for Id {
    type Error = Error;
    fn try_from(value: String) -> Result<Self, Error> {
        Self::new(value)
    }
}
impl From<Id> for String {
    fn from(value: Id) -> String {
        value.0
    }
}
impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Item,
    Fluid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    pub id: Id,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Amount {
    pub resource: Id,
    pub quantity: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub id: Id,
    pub machine: Id,
    pub duration_ticks: u32,
    pub eu_per_tick: u64,
    pub inputs: Vec<Amount>,
    pub outputs: Vec<Amount>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Content {
    pub schema_version: u32,
    pub provenance: String,
    pub resources: Vec<Resource>,
    pub recipes: Vec<Recipe>,
}

pub struct Catalog {
    pub(crate) resources: BTreeMap<Id, Resource>,
    pub(crate) recipes: BTreeMap<Id, Recipe>,
    anchors: BTreeMap<Id, BTreeMap<Id, Vec<Id>>>,
}
#[derive(Debug)]
pub struct Selection<'a> {
    pub recipe: Option<&'a Recipe>,
    pub candidates_examined: usize,
    pub streams: usize,
}
impl Catalog {
    pub fn select<'a>(
        &'a self,
        machine: &Id,
        inputs: &crate::inventory::Inventory,
    ) -> Result<Selection<'a>, Error> {
        inputs.validate(self)?;
        let mut result = Selection {
            recipe: None,
            candidates_examined: 0,
            streams: 0,
        };
        let Some(index) = self.anchors.get(machine) else {
            return Ok(result);
        };
        let lists: Vec<_> = inputs
            .contents()
            .filter_map(|(resource, _)| index.get(resource))
            .collect();
        result.streams = lists.len();
        let mut pending = BinaryHeap::new();
        for (stream, list) in lists.iter().enumerate() {
            if let Some(first) = list.first() {
                pending.push(Reverse((first, stream, 0usize)));
            }
        }
        while let Some(Reverse((identity, stream, offset))) = pending.pop() {
            let recipe = self
                .recipes
                .get(identity)
                .expect("immutable recipe index identity");
            result.candidates_examined += 1;
            if recipe
                .inputs
                .iter()
                .all(|a| inputs.amount(&a.resource) >= a.quantity)
            {
                result.recipe = Some(recipe);
                break;
            }
            if let Some(next) = lists[stream].get(offset + 1) {
                pending.push(Reverse((next, stream, offset + 1)));
            }
        }
        Ok(result)
    }
    pub fn new(content: Content) -> Result<Self, Error> {
        if content.schema_version != 1
            || content.provenance.is_empty()
            || content.provenance.len() > 1024
            || content.resources.is_empty()
            || content.resources.len() > 100_000
            || content.recipes.len() > 100_000
        {
            return Err("invalid content schema or bounds".into());
        }
        let mut catalog = Self {
            resources: BTreeMap::new(),
            recipes: BTreeMap::new(),
            anchors: BTreeMap::new(),
        };
        for resource in content.resources {
            if catalog
                .resources
                .insert(resource.id.clone(), resource)
                .is_some()
            {
                return Err("duplicate resource".into());
            }
        }
        for recipe in content.recipes {
            if recipe.duration_ticks == 0
                || recipe
                    .eu_per_tick
                    .checked_mul(u64::from(recipe.duration_ticks))
                    .is_none()
            {
                return Err(format!("invalid recipe duration/energy: {}", recipe.id));
            }
            for amounts in [&recipe.inputs, &recipe.outputs] {
                if amounts.is_empty() || amounts.len() > 64 {
                    return Err("invalid recipe port count".into());
                }
                let mut seen = BTreeSet::new();
                let mut totals = [0u64; 2];
                for amount in amounts {
                    if amount.quantity == 0 || !seen.insert(&amount.resource) {
                        return Err("zero or duplicate recipe amount".into());
                    }
                    let index = match catalog.kind(&amount.resource)? {
                        Kind::Item => 0,
                        Kind::Fluid => 1,
                    };
                    totals[index] = totals[index]
                        .checked_add(amount.quantity)
                        .ok_or("recipe quantity overflow")?;
                }
            }
            if catalog.recipes.insert(recipe.id.clone(), recipe).is_some() {
                return Err("duplicate recipe".into());
            }
        }
        // One rare required input anchors each recipe. A matching recipe must have that anchor.
        // Iterate the ordered recipe map so each candidate stream is already in ID priority order.
        let mut frequency: BTreeMap<(&Id, &Id), usize> = BTreeMap::new();
        for recipe in catalog.recipes.values() {
            for amount in &recipe.inputs {
                *frequency
                    .entry((&recipe.machine, &amount.resource))
                    .or_default() += 1;
            }
        }
        for recipe in catalog.recipes.values() {
            let anchor = recipe
                .inputs
                .iter()
                .min_by(|a, b| {
                    frequency[&(&recipe.machine, &a.resource)]
                        .cmp(&frequency[&(&recipe.machine, &b.resource)])
                        .then_with(|| a.resource.cmp(&b.resource))
                })
                .expect("nonempty inputs validated above");
            catalog
                .anchors
                .entry(recipe.machine.clone())
                .or_default()
                .entry(anchor.resource.clone())
                .or_default()
                .push(recipe.id.clone());
        }
        Ok(catalog)
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > 16 * 1024 * 1024 {
            return Err("content file too large".into());
        }
        Self::new(serde_json::from_slice(bytes).map_err(|e| e.to_string())?)
    }
    pub fn kind(&self, id: &Id) -> Result<Kind, Error> {
        self.resources
            .get(id)
            .map(|r| r.kind)
            .ok_or_else(|| format!("unknown resource: {id}"))
    }
    pub fn recipe(&self, id: &Id) -> Option<&Recipe> {
        self.recipes.get(id)
    }
    pub fn recipes(&self) -> impl Iterator<Item = &Recipe> {
        self.recipes.values()
    }
}

#[cfg(test)]
mod selection_tests;

#[cfg(test)]
mod tests {
    use super::*;
    pub(crate) fn fixture() -> Content {
        serde_json::from_str(r#"{"schema_version":1,"provenance":"synthetic-test","resources":[{"id":"test:ore","kind":"item"},{"id":"test:dust","kind":"item"},{"id":"test:water","kind":"fluid"}],"recipes":[{"id":"test:grind","machine":"test:grinder","duration_ticks":3,"eu_per_tick":8,"inputs":[{"resource":"test:ore","quantity":1}],"outputs":[{"resource":"test:dust","quantity":2}]}]}"#).unwrap()
    }
    #[test]
    fn ids_are_namespaced_bounded_and_path_safe() {
        for text in [
            "",
            "ore",
            "a:b:c",
            "../a:b",
            "test:UPPER",
            "test:",
            ":ore",
            "test:a/b",
        ] {
            assert!(Id::new(text).is_err(), "accepted {text}");
        }
        assert!(Id::new(format!("test:{}", "a".repeat(200))).is_err());
        assert!(Id::new("gtnh:iron_dust").is_ok());
    }
    #[test]
    fn duplicate_or_undefined_resources_and_recipes_are_rejected() {
        let mut c = fixture();
        c.resources.push(c.resources[0].clone());
        assert!(Catalog::new(c).is_err());
        let mut c = fixture();
        c.recipes.push(c.recipes[0].clone());
        assert!(Catalog::new(c).is_err());
        let mut c = fixture();
        c.recipes[0].inputs[0].resource = Id::new("test:absent").unwrap();
        assert!(Catalog::new(c).is_err());
    }
    #[test]
    fn invalid_recipe_quantities_durations_and_ambiguous_inputs_are_rejected() {
        let mut c = fixture();
        c.recipes[0].duration_ticks = 0;
        assert!(Catalog::new(c).is_err());
        let mut c = fixture();
        c.recipes[0].inputs[0].quantity = 0;
        assert!(Catalog::new(c).is_err());
        let mut c = fixture();
        let input = c.recipes[0].inputs[0].clone();
        c.recipes[0].inputs.push(input);
        assert!(Catalog::new(c).is_err());
        let mut c = fixture();
        c.schema_version = 2;
        assert!(Catalog::new(c).is_err());
    }
}
