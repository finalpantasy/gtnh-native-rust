//! Progress and staged reward claims; the caller persists reward and claim state together.

use crate::{
    Error,
    catalog::{Catalog, Quest, QuestId},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Progress {
    #[serde(deserialize_with = "task_entries")]
    tasks: BTreeMap<u32, u64>,
    complete: bool,
    claimed: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedBook {
    schema_version: u32,
    definitions_sha256: String,
    #[serde(deserialize_with = "quest_entries")]
    entries: BTreeMap<QuestId, Progress>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Book {
    saved: SavedBook,
}
const MAX_ENTRIES: usize = 16_384;
const MAX_SAVE_BYTES: usize = 4 * 1024 * 1024;

fn unique_map<'de, D, K, V>(deserializer: D, limit: usize) -> Result<BTreeMap<K, V>, D::Error>
where
    D: serde::Deserializer<'de>,
    K: Deserialize<'de> + Ord,
    V: Deserialize<'de>,
{
    struct Entries<K, V> {
        limit: usize,
        marker: std::marker::PhantomData<(K, V)>,
    }
    impl<'de, K: Deserialize<'de> + Ord, V: Deserialize<'de>> serde::de::Visitor<'de>
        for Entries<K, V>
    {
        type Value = BTreeMap<K, V>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a bounded map of distinct progress entries")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut entries = BTreeMap::new();
            while let Some((key, value)) = map.next_entry()? {
                if entries.len() == self.limit || entries.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom(
                        "duplicate or excessive progress entries",
                    ));
                }
            }
            Ok(entries)
        }
    }
    deserializer.deserialize_map(Entries {
        limit,
        marker: std::marker::PhantomData,
    })
}
fn quest_entries<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<QuestId, Progress>, D::Error> {
    unique_map(d, MAX_ENTRIES)
}
fn task_entries<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<u32, u64>, D::Error> {
    unique_map(d, 64)
}

fn tasks_met(quest: &Quest, progress: Option<&Progress>) -> Result<bool, Error> {
    let mut total = 0;
    let mut complete = 0;
    for task in quest.tasks.iter().filter(|t| !t.optional) {
        total += 1;
        if progress
            .and_then(|p| p.tasks.get(&task.id))
            .copied()
            .unwrap_or(0)
            >= task.goal
        {
            complete += 1;
        }
    }
    if total == 0 {
        Ok(true)
    } else {
        quest.task_logic.evaluate(complete, total)
    }
}
impl Book {
    pub fn new(catalog: &Catalog) -> Self {
        Self {
            saved: SavedBook {
                schema_version: 1,
                definitions_sha256: catalog.fingerprint().into(),
                entries: BTreeMap::new(),
            },
        }
    }
    pub fn is_complete(&self, id: &QuestId) -> bool {
        self.saved.entries.get(id).is_some_and(|p| p.complete)
    }
    pub fn has_claimed(&self, id: &QuestId) -> bool {
        self.saved.entries.get(id).is_some_and(|p| p.claimed)
    }
    pub fn task_progress(&self, id: &QuestId, task: u32) -> u64 {
        self.saved
            .entries
            .get(id)
            .and_then(|p| p.tasks.get(&task))
            .copied()
            .unwrap_or(0)
    }
    fn check_catalog(&self, catalog: &Catalog) -> Result<(), Error> {
        if self.saved.schema_version != 1 || self.saved.definitions_sha256 != catalog.fingerprint()
        {
            return Err("quest state requires explicit content migration".into());
        }
        Ok(())
    }
    pub fn unlocked(&self, catalog: &Catalog, id: &QuestId) -> Result<bool, Error> {
        self.check_catalog(catalog)?;
        catalog.unlocked_by(id, |id| self.is_complete(id))
    }
    fn entry(&mut self, id: &QuestId) -> Result<&mut Progress, Error> {
        if !self.saved.entries.contains_key(id) && self.saved.entries.len() == MAX_ENTRIES {
            return Err("quest progress entry limit".into());
        }
        Ok(self.saved.entries.entry(id.clone()).or_default())
    }
    /// Record an absolute monotonic event count; live inventory task detection is outside this core.
    pub fn record_total(
        &mut self,
        catalog: &Catalog,
        id: &QuestId,
        task: u32,
        total: u64,
    ) -> Result<bool, Error> {
        self.check_catalog(catalog)?;
        let quest = catalog.quest(id).ok_or("unknown quest")?;
        let definition = quest
            .tasks
            .iter()
            .find(|t| t.id == task)
            .ok_or("unknown quest task")?;
        if self.is_complete(id) {
            return Ok(false);
        }
        if !self.unlocked(catalog, id)? {
            return Err("quest is locked".into());
        }
        let previous = self.task_progress(id, task);
        let next = total.min(definition.goal);
        if next < previous {
            return Err("progress counters cannot decrease".into());
        }
        if next == previous {
            return Ok(false);
        }
        self.entry(id)?.tasks.insert(task, next);
        Ok(true)
    }
    pub fn complete(&mut self, catalog: &Catalog, id: &QuestId) -> Result<bool, Error> {
        self.check_catalog(catalog)?;
        let quest = catalog.quest(id).ok_or("unknown quest")?;
        if self.is_complete(id) {
            return Ok(false);
        }
        if !self.unlocked(catalog, id)? || !tasks_met(quest, self.saved.entries.get(id))? {
            return Err("quest requirements are not complete".into());
        }
        self.entry(id)?.complete = true;
        Ok(true)
    }
    /// Stage a claim flag. Persist this book and the reward inventory in the same transaction.
    pub fn claim(&mut self, catalog: &Catalog, id: &QuestId) -> Result<bool, Error> {
        self.check_catalog(catalog)?;
        catalog.quest(id).ok_or("unknown quest")?;
        if !self.is_complete(id) {
            return Err("cannot claim an incomplete quest".into());
        }
        if self.has_claimed(id) {
            return Ok(false);
        }
        self.entry(id)?.claimed = true;
        Ok(true)
    }
    fn validate(&self, catalog: &Catalog) -> Result<(), Error> {
        self.check_catalog(catalog)?;
        if self.saved.entries.len() > MAX_ENTRIES {
            return Err("excessive quest progress entries".into());
        }
        for (id, progress) in &self.saved.entries {
            let quest = catalog.quest(id).ok_or("unknown saved quest")?;
            if progress.tasks.len() > quest.tasks.len()
                || (progress.claimed && !progress.complete)
                || (progress.complete && !tasks_met(quest, Some(progress))?)
            {
                return Err("invalid saved quest completion".into());
            }
            for (task, quantity) in &progress.tasks {
                let definition = quest
                    .tasks
                    .iter()
                    .find(|t| t.id == *task)
                    .ok_or("unknown saved task")?;
                if *quantity == 0 || *quantity > definition.goal {
                    return Err("invalid saved task count".into());
                }
            }
        }
        Ok(())
    }
    pub fn encode(&self, catalog: &Catalog) -> Result<Vec<u8>, Error> {
        self.validate(catalog)?;
        let bytes = serde_json::to_vec(&self.saved).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_SAVE_BYTES {
            return Err("quest save exceeds 4 MiB".into());
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8], catalog: &Catalog) -> Result<Self, Error> {
        if bytes.len() > MAX_SAVE_BYTES {
            return Err("quest save exceeds 4 MiB".into());
        }
        let book = Self {
            saved: serde_json::from_slice(bytes).map_err(|e| e.to_string())?,
        };
        book.validate(catalog)?;
        Ok(book)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        catalog::{Content, Task, tests::quest},
        logic::Logic,
    };
    fn id(n: u128) -> QuestId {
        QuestId::from_u128(n)
    }
    fn setup() -> (Catalog, Book) {
        let mut xor = quest(2, &[0, 1], Logic::XOR);
        xor.tasks.push(Task {
            id: 7,
            goal: 99,
            optional: true,
        });
        let mut optional = quest(3, &[], Logic::OR);
        optional.tasks[0].optional = true;
        optional.task_logic = Logic::OR;
        let catalog = Catalog::new(Content {
            schema_version: 1,
            quests: vec![
                quest(0, &[], Logic::AND),
                quest(1, &[], Logic::AND),
                xor,
                optional,
            ],
        })
        .unwrap();
        let book = Book::new(&catalog);
        (catalog, book)
    }
    #[test]
    fn locked_quests_and_unknown_tasks_cannot_gain_progress() {
        let (cat, mut b) = setup();
        let before = b.clone();
        assert!(b.record_total(&cat, &id(2), 0, 2).is_err());
        assert_eq!(b, before);
        assert!(b.record_total(&cat, &id(0), 99, 2).is_err());
        assert_eq!(b, before);
        assert!(b.claim(&cat, &id(0)).is_err());
        assert_eq!(b, before);
    }
    #[test]
    fn optional_tasks_do_not_block_and_xor_relocking_never_undoes_completion() {
        let (cat, mut b) = setup();
        assert!(b.complete(&cat, &id(3)).unwrap());
        assert!(b.record_total(&cat, &id(0), 0, 2).unwrap());
        assert!(b.complete(&cat, &id(0)).unwrap());
        assert!(b.unlocked(&cat, &id(2)).unwrap());
        b.record_total(&cat, &id(2), 0, 2).unwrap();
        assert!(b.complete(&cat, &id(2)).unwrap());
        assert!(b.is_complete(&id(2)));
        assert_eq!(b.task_progress(&id(2), 7), 0);
        b.record_total(&cat, &id(1), 0, 2).unwrap();
        b.complete(&cat, &id(1)).unwrap();
        assert!(!b.unlocked(&cat, &id(2)).unwrap());
        assert!(b.is_complete(&id(2)));
        assert!(b.claim(&cat, &id(2)).unwrap());
        assert!(!b.claim(&cat, &id(2)).unwrap());
    }
    #[test]
    fn counters_are_monotonic_capped_and_save_resume_preserves_single_claim() {
        let (cat, mut b) = setup();
        b.record_total(&cat, &id(0), 0, 1).unwrap();
        let before = b.clone();
        assert!(b.record_total(&cat, &id(0), 0, 0).is_err());
        assert_eq!(b, before);
        assert!(b.complete(&cat, &id(0)).is_err());
        assert_eq!(b, before);
        let mut loaded = Book::decode(&b.encode(&cat).unwrap(), &cat).unwrap();
        assert_eq!(loaded, b);
        loaded.record_total(&cat, &id(0), 0, u64::MAX).unwrap();
        assert_eq!(loaded.task_progress(&id(0), 0), 2);
        loaded.complete(&cat, &id(0)).unwrap();
        assert!(loaded.claim(&cat, &id(0)).unwrap());
        let mut reloaded = Book::decode(&loaded.encode(&cat).unwrap(), &cat).unwrap();
        assert!(!reloaded.claim(&cat, &id(0)).unwrap());
    }
    #[test]
    fn mismatched_content_unknown_progress_and_impossible_claims_are_rejected() {
        let (cat, b) = setup();
        let bytes = b.encode(&cat).unwrap();
        let mut changed = quest(0, &[], Logic::AND);
        changed.tasks[0].goal = 3;
        let other = Catalog::new(Content {
            schema_version: 1,
            quests: vec![changed],
        })
        .unwrap();
        assert!(Book::decode(&bytes, &other).is_err());
        let mut data: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        data["entries"][id(0).as_str()] =
            serde_json::json!({"tasks":{},"complete":false,"claimed":true});
        assert!(Book::decode(&serde_json::to_vec(&data).unwrap(), &cat).is_err());
        data["entries"] =
            serde_json::json!({id(99).as_str():{"tasks":{},"complete":false,"claimed":false}});
        assert!(Book::decode(&serde_json::to_vec(&data).unwrap(), &cat).is_err());
    }
    #[test]
    fn duplicate_save_entries_and_duplicate_task_progress_are_rejected() {
        let (cat, _) = setup();
        let p = r#"{"tasks":{},"complete":false,"claimed":false}"#;
        let bytes = format!(
            r#"{{"schema_version":1,"definitions_sha256":"{}","entries":{{"{}":{},"{}":{}}}}}"#,
            cat.fingerprint(),
            id(0),
            p,
            id(0),
            p
        );
        assert!(Book::decode(bytes.as_bytes(), &cat).is_err());
        let bytes = format!(
            r#"{{"schema_version":1,"definitions_sha256":"{}","entries":{{"{}":{{"tasks":{{"0":1,"0":2}},"complete":false,"claimed":false}}}}}}"#,
            cat.fingerprint(),
            id(0)
        );
        assert!(Book::decode(bytes.as_bytes(), &cat).is_err());
    }
}
