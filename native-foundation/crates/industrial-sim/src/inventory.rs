//! Bounded aggregate item/fluid ports (not slot-parity inventories).

use crate::{
    Error,
    content::{Amount, Catalog, Id, Kind},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub items: u64,
    pub fluid_mb: u64,
    pub kinds: usize,
}
impl Limits {
    fn validate(self) -> Result<(), Error> {
        if self.kinds == 0 || self.kinds > 64 || (self.items == 0 && self.fluid_mb == 0) {
            Err("invalid port limits".into())
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    limits: Limits,
    #[serde(deserialize_with = "unique_contents")]
    contents: BTreeMap<Id, u64>,
}

fn unique_contents<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<Id, u64>, D::Error> {
    struct Entries;
    impl<'de> serde::de::Visitor<'de> for Entries {
        type Value = BTreeMap<Id, u64>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("at most 64 distinct inventory entries")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut contents = BTreeMap::new();
            while let Some((id, quantity)) = map.next_entry::<Id, u64>()? {
                if contents.len() == 64 || contents.insert(id, quantity).is_some() {
                    return Err(serde::de::Error::custom(
                        "duplicate or excessive inventory entries",
                    ));
                }
            }
            Ok(contents)
        }
    }
    deserializer.deserialize_map(Entries)
}

impl Inventory {
    pub fn new(limits: Limits) -> Result<Self, Error> {
        limits.validate()?;
        Ok(Self {
            limits,
            contents: BTreeMap::new(),
        })
    }
    pub fn limits(&self) -> Limits {
        self.limits
    }
    pub fn amount(&self, id: &Id) -> u64 {
        *self.contents.get(id).unwrap_or(&0)
    }
    pub fn contents(&self) -> impl Iterator<Item = (&Id, u64)> {
        self.contents.iter().map(|(id, n)| (id, *n))
    }
    fn totals(&self, catalog: &Catalog) -> Result<[u64; 2], Error> {
        let mut totals = [0u64; 2];
        for (id, quantity) in &self.contents {
            if *quantity == 0 {
                return Err("zero inventory entry".into());
            }
            let index = match catalog.kind(id)? {
                Kind::Item => 0,
                Kind::Fluid => 1,
            };
            totals[index] = totals[index]
                .checked_add(*quantity)
                .ok_or("inventory quantity overflow")?;
        }
        Ok(totals)
    }
    pub fn validate(&self, catalog: &Catalog) -> Result<(), Error> {
        self.limits.validate()?;
        if self.contents.len() > self.limits.kinds {
            return Err("too many resource kinds".into());
        }
        let [items, fluid] = self.totals(catalog)?;
        if items > self.limits.items || fluid > self.limits.fluid_mb {
            return Err("port capacity exceeded".into());
        }
        Ok(())
    }
    pub fn apply(
        &mut self,
        catalog: &Catalog,
        remove: &[Amount],
        add: &[Amount],
    ) -> Result<(), Error> {
        self.validate(catalog)?;
        for amounts in [remove, add] {
            if amounts.len() > 64 {
                return Err("too many port edits".into());
            }
            let mut seen = BTreeSet::new();
            for amount in amounts {
                catalog.kind(&amount.resource)?;
                if amount.quantity == 0 || !seen.insert(&amount.resource) {
                    return Err("zero or duplicate port edit".into());
                }
            }
        }
        let mut candidate = self.clone();
        for amount in remove {
            let next = candidate
                .amount(&amount.resource)
                .checked_sub(amount.quantity)
                .ok_or("insufficient resource")?;
            if next == 0 {
                candidate.contents.remove(&amount.resource);
            } else {
                candidate.contents.insert(amount.resource.clone(), next);
            }
        }
        for amount in add {
            let next = candidate
                .amount(&amount.resource)
                .checked_add(amount.quantity)
                .ok_or("resource overflow")?;
            candidate.contents.insert(amount.resource.clone(), next);
        }
        candidate.validate(catalog)?;
        *self = candidate;
        Ok(())
    }
    pub fn room(&self, catalog: &Catalog, id: &Id) -> Result<u64, Error> {
        self.validate(catalog)?;
        let kind = catalog.kind(id)?;
        if !self.contents.contains_key(id) && self.contents.len() == self.limits.kinds {
            return Ok(0);
        }
        let [items, fluid] = self.totals(catalog)?;
        Ok(match kind {
            Kind::Item => self.limits.items - items,
            Kind::Fluid => self.limits.fluid_mb - fluid,
        })
    }
    pub fn transfer(
        &mut self,
        to: &mut Self,
        catalog: &Catalog,
        id: &Id,
        limit: u64,
    ) -> Result<u64, Error> {
        self.validate(catalog)?;
        let quantity = self.amount(id).min(limit).min(to.room(catalog, id)?);
        if quantity == 0 {
            return Ok(0);
        }
        let amount = [Amount {
            resource: id.clone(),
            quantity,
        }];
        let (mut source, mut target) = (self.clone(), to.clone());
        source.apply(catalog, &amount, &[])?;
        target.apply(catalog, &[], &amount)?;
        *self = source;
        *to = target;
        Ok(quantity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{Content, Resource};
    fn id(s: &str) -> Id {
        Id::new(format!("test:{s}")).unwrap()
    }
    fn catalog() -> Catalog {
        Catalog::new(Content {
            schema_version: 1,
            provenance: "synthetic".into(),
            resources: vec![
                Resource {
                    id: id("ore"),
                    kind: Kind::Item,
                },
                Resource {
                    id: id("dust"),
                    kind: Kind::Item,
                },
                Resource {
                    id: id("water"),
                    kind: Kind::Fluid,
                },
            ],
            recipes: vec![],
        })
        .unwrap()
    }
    fn inventory() -> Inventory {
        Inventory::new(Limits {
            items: 8,
            fluid_mb: 1000,
            kinds: 3,
        })
        .unwrap()
    }
    fn amount(name: &str, n: u64) -> Amount {
        Amount {
            resource: id(name),
            quantity: n,
        }
    }
    #[test]
    fn item_and_fluid_limits_are_independent_and_failed_edits_are_atomic() {
        let cat = catalog();
        let mut inv = inventory();
        inv.apply(&cat, &[], &[amount("ore", 8), amount("water", 1000)])
            .unwrap();
        let before = inv.clone();
        assert!(
            inv.apply(&cat, &[amount("ore", 2)], &[amount("dust", 3)])
                .is_err()
        );
        assert_eq!(inv, before);
        assert!(inv.apply(&cat, &[amount("water", 1001)], &[]).is_err());
        assert_eq!(inv, before);
        inv.apply(&cat, &[amount("ore", 2)], &[amount("dust", 2)])
            .unwrap();
        assert_eq!(inv.amount(&id("ore")), 6);
        assert_eq!(inv.amount(&id("dust")), 2);
    }
    #[test]
    fn transfer_is_partial_and_conserves_the_resource_under_backpressure() {
        let cat = catalog();
        let (mut a, mut b) = (inventory(), inventory());
        a.apply(&cat, &[], &[amount("ore", 8)]).unwrap();
        b.apply(&cat, &[], &[amount("dust", 6)]).unwrap();
        assert_eq!(a.transfer(&mut b, &cat, &id("ore"), 7).unwrap(), 2);
        assert_eq!(a.amount(&id("ore")) + b.amount(&id("ore")), 8);
        assert_eq!(a.transfer(&mut b, &cat, &id("ore"), 7).unwrap(), 0);
    }
    #[test]
    fn duplicate_unknown_zero_and_overflowing_amounts_are_rejected() {
        let cat = catalog();
        let mut inv = inventory();
        let before = inv.clone();
        for rows in [
            vec![amount("ore", 1), amount("ore", 1)],
            vec![amount("absent", 1)],
            vec![amount("ore", 0)],
            vec![amount("ore", u64::MAX), amount("dust", 1)],
        ] {
            assert!(inv.apply(&cat, &[], &rows).is_err());
            assert_eq!(inv, before);
        }
        assert!(
            Inventory::new(Limits {
                items: 0,
                fluid_mb: 0,
                kinds: 1
            })
            .is_err()
        );
        assert!(
            Inventory::new(Limits {
                items: 8,
                fluid_mb: 0,
                kinds: 65
            })
            .is_err()
        );
    }
    #[test]
    fn new_kinds_do_not_bypass_kind_limits_and_decoded_states_are_validated() {
        let cat = catalog();
        let mut inv = Inventory::new(Limits {
            items: 8,
            fluid_mb: 1000,
            kinds: 1,
        })
        .unwrap();
        inv.apply(&cat, &[], &[amount("ore", 1)]).unwrap();
        assert_eq!(inv.room(&cat, &id("water")).unwrap(), 0);
        assert!(inv.apply(&cat, &[], &[amount("water", 1)]).is_err());
        let invalid: Inventory = serde_json::from_str(
            r#"{"limits":{"items":8,"fluid_mb":0,"kinds":1},"contents":{"test:ore":9}}"#,
        )
        .unwrap();
        assert!(invalid.validate(&cat).is_err());
    }

    #[test]
    fn duplicate_serialized_entries_are_never_silently_merged() {
        let bytes=br#"{"limits":{"items":8,"fluid_mb":0,"kinds":2},"contents":{"test:ore":2,"test:ore":3}}"#;
        assert!(serde_json::from_slice::<Inventory>(bytes).is_err());
    }
}
