use crate::{Error, logic::Logic};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct QuestId(String);
impl QuestId {
    pub fn from_u128(value: u128) -> Self {
        Self(format!("{value:032x}"))
    }
    pub fn parse(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        if value.len() != 32
            || !value
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err("quest ID requires 32 lowercase hex digits".into());
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for QuestId {
    type Error = Error;
    fn try_from(value: String) -> Result<Self, Error> {
        Self::parse(value)
    }
}
impl From<QuestId> for String {
    fn from(value: QuestId) -> Self {
        value.0
    }
}
impl fmt::Display for QuestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: u32,
    pub goal: u64,
    pub optional: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quest {
    pub id: QuestId,
    pub prerequisites: Vec<QuestId>,
    pub logic: Logic,
    pub tasks: Vec<Task>,
    pub task_logic: Logic,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Content {
    pub schema_version: u32,
    pub quests: Vec<Quest>,
}
pub struct Catalog {
    pub(crate) quests: BTreeMap<QuestId, Quest>,
    fingerprint: String,
    missing: BTreeSet<QuestId>,
}
impl Catalog {
    pub fn new(content: Content) -> Result<Self, Error> {
        if content.schema_version != 1 || content.quests.len() > 100_000 {
            return Err("unsupported quest content schema or count".into());
        }
        let mut quests = BTreeMap::new();
        for quest in content.quests {
            if quest.prerequisites.len() > 1024 || quest.tasks.len() > 64 {
                return Err("quest definition exceeds bounds".into());
            }
            if quest.prerequisites.iter().collect::<BTreeSet<_>>().len()
                != quest.prerequisites.len()
            {
                return Err("duplicate prerequisite".into());
            }
            let mut tasks = BTreeSet::new();
            for task in &quest.tasks {
                if task.goal == 0 || !tasks.insert(task.id) {
                    return Err("zero goal or duplicate task ID".into());
                }
            }
            if quests.insert(quest.id.clone(), quest).is_some() {
                return Err("duplicate quest ID".into());
            }
        }
        let missing = quests
            .values()
            .flat_map(|q| q.prerequisites.iter())
            .filter(|id| !quests.contains_key(*id))
            .cloned()
            .collect();
        let encoded = serde_json::to_vec(&quests).map_err(|e| e.to_string())?;
        let fingerprint = format!("{:x}", Sha256::digest(&encoded));
        Ok(Self {
            quests,
            fingerprint,
            missing,
        })
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > 16 * 1024 * 1024 {
            return Err("quest definitions file too large".into());
        }
        Self::new(serde_json::from_slice(bytes).map_err(|e| e.to_string())?)
    }
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
    pub fn quest(&self, id: &QuestId) -> Option<&Quest> {
        self.quests.get(id)
    }
    pub fn missing(&self) -> &BTreeSet<QuestId> {
        &self.missing
    }
    fn counts(
        &self,
        id: &QuestId,
        mut complete: impl FnMut(&QuestId) -> bool,
    ) -> Result<(&Quest, u32, u32), Error> {
        let quest = self.quest(id).ok_or("unknown quest")?;
        let count = quest
            .prerequisites
            .iter()
            .filter(|id| self.quests.contains_key(*id) && complete(id))
            .count() as u32;
        Ok((quest, count, quest.prerequisites.len() as u32))
    }
    pub fn unlocked(&self, id: &QuestId, complete: &BTreeSet<QuestId>) -> Result<bool, Error> {
        self.unlocked_by(id, |id| complete.contains(id))
    }
    pub fn unlockable(&self, id: &QuestId, complete: &BTreeSet<QuestId>) -> Result<bool, Error> {
        let (quest, count, total) = self.counts(id, |id| complete.contains(id))?;
        if total == 0 {
            return Ok(true);
        }
        quest.logic.unlockable(count, total)
    }
    pub(crate) fn unlocked_by(
        &self,
        id: &QuestId,
        complete: impl FnMut(&QuestId) -> bool,
    ) -> Result<bool, Error> {
        let (quest, count, total) = self.counts(id, complete)?;
        if total == 0 {
            return Ok(true);
        }
        quest.logic.evaluate(count, total)
    }
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn quest(id: u128, prerequisites: &[u128], logic: Logic) -> Quest {
        Quest {
            id: QuestId::from_u128(id),
            prerequisites: prerequisites
                .iter()
                .map(|v| QuestId::from_u128(*v))
                .collect(),
            logic,
            tasks: vec![Task {
                id: 0,
                goal: 2,
                optional: false,
            }],
            task_logic: Logic::AND,
        }
    }
    #[test]
    fn quest_ids_are_fixed_width_hex_and_roundtrip_the_full_128_bits() {
        for value in [0, 1, u128::MAX] {
            let id = QuestId::from_u128(value);
            assert_eq!(id.as_str().len(), 32);
            assert_eq!(QuestId::parse(id.as_str()).unwrap(), id);
        }
        for value in [
            "",
            "1",
            "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF",
            "g0000000000000000000000000000000",
        ] {
            assert!(QuestId::parse(value).is_err());
        }
    }
    #[test]
    fn missing_prerequisites_and_cycles_are_explicit_and_do_not_unlock_for_free() {
        let catalog = Catalog::new(Content {
            schema_version: 1,
            quests: vec![
                quest(1, &[2], Logic::AND),
                quest(2, &[1], Logic::AND),
                quest(3, &[99], Logic::AND),
                quest(4, &[], Logic::OR),
            ],
        })
        .unwrap();
        assert_eq!(catalog.missing(), &BTreeSet::from([QuestId::from_u128(99)]));
        assert!(
            !catalog
                .unlocked(&QuestId::from_u128(1), &BTreeSet::new())
                .unwrap()
        );
        assert!(
            !catalog
                .unlocked(
                    &QuestId::from_u128(3),
                    &BTreeSet::from([QuestId::from_u128(99)])
                )
                .unwrap()
        );
        assert!(
            catalog
                .unlocked(&QuestId::from_u128(4), &BTreeSet::new())
                .unwrap()
        );
    }
    #[test]
    fn duplicate_quests_tasks_edges_and_zero_goals_are_rejected() {
        let q = quest(1, &[], Logic::AND);
        assert!(
            Catalog::new(Content {
                schema_version: 1,
                quests: vec![q.clone(), q.clone()]
            })
            .is_err()
        );
        let mut duplicate = q.clone();
        duplicate.tasks.push(duplicate.tasks[0].clone());
        assert!(
            Catalog::new(Content {
                schema_version: 1,
                quests: vec![duplicate]
            })
            .is_err()
        );
        let mut zero = q;
        zero.tasks[0].goal = 0;
        assert!(
            Catalog::new(Content {
                schema_version: 1,
                quests: vec![zero]
            })
            .is_err()
        );
        assert!(
            Catalog::new(Content {
                schema_version: 1,
                quests: vec![quest(1, &[2, 2], Logic::AND)]
            })
            .is_err()
        );
    }
    #[test]
    fn digest_is_stable_for_quest_order_but_changes_with_definitions() {
        let qs = vec![quest(1, &[], Logic::AND), quest(2, &[1], Logic::AND)];
        let first = Catalog::new(Content {
            schema_version: 1,
            quests: qs.clone(),
        })
        .unwrap();
        let mut reverse = qs.clone();
        reverse.reverse();
        let second = Catalog::new(Content {
            schema_version: 1,
            quests: reverse,
        })
        .unwrap();
        assert_eq!(first.fingerprint(), second.fingerprint());
        assert_eq!(first.fingerprint().len(), 64);
        let mut changed = qs;
        changed[0].tasks[0].goal = 3;
        let third = Catalog::new(Content {
            schema_version: 1,
            quests: changed,
        })
        .unwrap();
        assert_ne!(first.fingerprint(), third.fingerprint());
    }
}
