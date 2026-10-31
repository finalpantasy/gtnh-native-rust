//! Explicit bounded BetterQuesting 3.8.92 legacy comparison, not generic Minecraft NBT equality.
//! Adapted from pinned MIT ItemComparison.java; see THIRD_PARTY_NOTICES.md.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};
pub const DEFAULT_BUDGET: u32 = 65_536;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    FloatBits(u32),
    DoubleBits(u64),
    String(String),
    ByteArray(Vec<i8>),
    IntArray(Vec<i32>),
    List(Vec<Tag>),
    Compound(#[serde(deserialize_with = "unique_compound")] BTreeMap<String, Tag>),
}
fn unique_compound<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, Tag>, D::Error> {
    struct Fields;
    impl<'de> serde::de::Visitor<'de> for Fields {
        type Value = BTreeMap<String, Tag>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("at most 4096 unique compound keys")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, tag)) = map.next_entry::<String, Tag>()? {
                if key.len() > 256 || result.len() >= 4096 || result.insert(key, tag).is_some() {
                    return Err(serde::de::Error::custom(
                        "duplicate or excessive compound field",
                    ));
                }
            }
            Ok(result)
        }
    }
    d.deserialize_map(Fields)
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stack {
    pub registry: String,
    pub damage: i32,
    pub nbt: Option<Tag>,
}
/// A borrowed stack whose finite shape has already been checked; its fields cannot be mutated.
#[derive(Clone, Copy)]
pub struct StackView<'a>(&'a Stack);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchError {
    DepthLimit,
    NodeLimit,
    ByteLimit,
    InvalidTag,
    InvalidStack,
    InvalidBudget,
    QueryLimit,
    InvalidEncoding,
}
impl fmt::Display for MatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for MatchError {}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Comparison {
    pub matches: bool,
    pub operations: u32,
}
impl Tag {
    fn kind(&self) -> u8 {
        match self {
            Self::Byte(_) => 1,
            Self::Short(_) => 2,
            Self::Int(_) => 3,
            Self::Long(_) => 4,
            Self::FloatBits(_) => 5,
            Self::DoubleBits(_) => 6,
            Self::ByteArray(_) => 7,
            Self::String(_) => 8,
            Self::List(_) => 9,
            Self::Compound(_) => 10,
            Self::IntArray(_) => 11,
        }
    }
    fn integer(&self) -> Option<i64> {
        match self {
            Self::Byte(n) => Some(i64::from(*n)),
            Self::Short(n) => Some(i64::from(*n)),
            Self::Int(n) => Some(i64::from(*n)),
            Self::Long(n) => Some(*n),
            _ => None,
        }
    }
    fn real(&self) -> Option<f64> {
        match self {
            Self::FloatBits(n) => Some(f64::from(f32::from_bits(*n))),
            Self::DoubleBits(n) => Some(f64::from_bits(*n)),
            _ => self.integer().map(|n| n as f64),
        }
    }
    pub fn validate(&self) -> Result<(), MatchError> {
        fn visit(
            tag: &Tag,
            depth: u32,
            nodes: &mut usize,
            bytes: &mut usize,
        ) -> Result<(), MatchError> {
            if depth > 16 {
                return Err(MatchError::DepthLimit);
            }
            *nodes += 1;
            if *nodes > 4096 {
                return Err(MatchError::NodeLimit);
            }
            match tag {
                Tag::String(s) => {
                    if s.len() > 16384 {
                        return Err(MatchError::ByteLimit);
                    }
                    *bytes += s.len();
                }
                Tag::ByteArray(a) => {
                    *nodes = nodes.saturating_add(a.len());
                    *bytes = bytes.saturating_add(a.len());
                }
                Tag::IntArray(a) => {
                    *nodes = nodes.saturating_add(a.len());
                    *bytes = bytes.saturating_add(a.len().saturating_mul(4));
                }
                Tag::List(items) => {
                    if items.len() > 4096 {
                        return Err(MatchError::NodeLimit);
                    }
                    if let Some(first) = items.first()
                        && items.iter().any(|tag| tag.kind() != first.kind())
                    {
                        return Err(MatchError::InvalidTag);
                    }
                    for item in items {
                        visit(item, depth + 1, nodes, bytes)?;
                    }
                }
                Tag::Compound(fields) => {
                    for (key, tag) in fields {
                        if key.len() > 256 {
                            return Err(MatchError::ByteLimit);
                        }
                        *bytes += key.len();
                        visit(tag, depth + 1, nodes, bytes)?;
                    }
                }
                _ => {}
            }
            if *nodes > 4096 {
                return Err(MatchError::NodeLimit);
            }
            if *bytes > 1024 * 1024 {
                return Err(MatchError::ByteLimit);
            }
            Ok(())
        }
        visit(self, 0, &mut 0, &mut 0)
    }
    pub fn encode(&self) -> Result<Vec<u8>, MatchError> {
        self.validate()?;
        let data = serde_json::to_vec(self).map_err(|_| MatchError::InvalidEncoding)?;
        if data.len() > 1024 * 1024 {
            return Err(MatchError::ByteLimit);
        }
        Ok(data)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, MatchError> {
        if bytes.len() > 1024 * 1024 {
            return Err(MatchError::ByteLimit);
        }
        let tag: Self = serde_json::from_slice(bytes).map_err(|_| MatchError::InvalidEncoding)?;
        tag.validate()?;
        Ok(tag)
    }
}
impl Stack {
    pub fn view(&self) -> Result<StackView<'_>, MatchError> {
        self.validate()?;
        Ok(StackView(self))
    }
    pub fn validate(&self) -> Result<(), MatchError> {
        let Some((namespace, path)) = self.registry.split_once(':') else {
            return Err(MatchError::InvalidStack);
        };
        if self.registry.len() > 256
            || namespace.is_empty()
            || path.is_empty()
            || path.contains(':')
            || !self.registry.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(MatchError::InvalidStack);
        }
        if let Some(tag) = &self.nbt {
            if !matches!(tag, Tag::Compound(_)) {
                return Err(MatchError::InvalidStack);
            }
            tag.validate()?;
        }
        Ok(())
    }
}
struct Work {
    limit: u32,
    used: u32,
}
impl Work {
    fn new(limit: u32) -> Result<Self, MatchError> {
        if !(1..=1_000_000).contains(&limit) {
            Err(MatchError::InvalidBudget)
        } else {
            Ok(Self { limit, used: 0 })
        }
    }
    fn charge(&mut self, cost: u32) -> Result<(), MatchError> {
        if cost > self.limit - self.used {
            return Err(MatchError::QueryLimit);
        }
        self.used += cost;
        Ok(())
    }
    fn result(self, matches: bool) -> Comparison {
        Comparison {
            matches,
            operations: self.used,
        }
    }
}
fn empty(tag: Option<&Tag>) -> bool {
    match tag {
        None => true,
        Some(Tag::Compound(m)) => m.is_empty(),
        Some(Tag::List(l)) => l.is_empty(),
        _ => false,
    }
}
fn multiset<T: PartialEq>(
    required: &[T],
    sample: &[T],
    partial: bool,
    work: &mut Work,
) -> Result<bool, MatchError> {
    if required.len() > sample.len() || (!partial && required.len() != sample.len()) {
        return Ok(false);
    }
    let mut used = vec![false; sample.len()];
    for requested in required {
        let mut found = false;
        for (index, candidate) in sample.iter().enumerate() {
            work.charge(1)?;
            if !used[index] && requested == candidate {
                used[index] = true;
                found = true;
                break;
            }
        }
        if !found {
            return Ok(false);
        }
    }
    Ok(true)
}
fn compare(
    required: Option<&Tag>,
    sample: Option<&Tag>,
    partial: bool,
    work: &mut Work,
) -> Result<bool, MatchError> {
    work.charge(1)?;
    if empty(required) != empty(sample) {
        return Ok(false);
    }
    if empty(required) {
        return Ok(true);
    }
    let (a, b) = (
        required.expect("nonempty requested tag"),
        sample.expect("nonempty sample tag"),
    );
    let numeric = (1..=6).contains(&a.kind()) && (1..=6).contains(&b.kind());
    if !numeric && a.kind() != b.kind() {
        return Ok(false);
    }
    if numeric {
        return Ok(if a.kind() >= 5 || b.kind() >= 5 {
            a.real() == b.real()
        } else {
            a.integer() == b.integer()
        });
    }
    match (a, b) {
        (Tag::Compound(required), Tag::Compound(sample)) => {
            for (key, tag) in required {
                work.charge(key.len() as u32 + 1)?;
                let Some(candidate) = sample.get(key) else {
                    return Ok(false);
                };
                if !compare(Some(tag), Some(candidate), partial, work)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (Tag::List(required), Tag::List(sample)) => {
            if required.len() > sample.len() || (!partial && required.len() != sample.len()) {
                return Ok(false);
            }
            for requested in required {
                // The legacy getCompoundTagAt accessor returns an empty compound for noncompound entries.
                let requested = if matches!(requested, Tag::Compound(_)) {
                    Some(requested)
                } else {
                    None
                };
                let mut found = false;
                for candidate in sample {
                    let candidate = if matches!(candidate, Tag::Compound(_)) {
                        Some(candidate)
                    } else {
                        None
                    };
                    if compare(requested, candidate, partial, work)? {
                        found = true;
                        break;
                    }
                }
                if !found {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (Tag::ByteArray(a), Tag::ByteArray(b)) => multiset(a, b, partial, work),
        (Tag::IntArray(a), Tag::IntArray(b)) => multiset(a, b, partial, work),
        (Tag::String(a), Tag::String(b)) => {
            if a.len() != b.len() {
                return Ok(false);
            }
            work.charge(a.len() as u32)?;
            Ok(a == b)
        }
        _ => unreachable!("all validated legacy tag kinds handled above"),
    }
}
pub fn compare_nbt(
    required: Option<&Tag>,
    sample: Option<&Tag>,
    partial: bool,
    budget: u32,
) -> Result<Comparison, MatchError> {
    let mut work = Work::new(budget)?;
    if let Some(tag) = required {
        tag.validate()?;
    }
    if let Some(tag) = sample {
        tag.validate()?;
    }
    let matches = compare(required, sample, partial, &mut work)?;
    Ok(work.result(matches))
}
pub fn stack_match(
    required: Option<&Stack>,
    sample: Option<&Stack>,
    damageable: bool,
    nbt_check: bool,
    partial: bool,
    budget: u32,
) -> Result<Comparison, MatchError> {
    Work::new(budget)?;
    let required = required.map(Stack::view).transpose()?;
    let sample = sample.map(Stack::view).transpose()?;
    compare_stacks(required, sample, damageable, nbt_check, partial, budget)
}
pub fn compare_stacks(
    required: Option<StackView<'_>>,
    sample: Option<StackView<'_>>,
    damageable: bool,
    nbt_check: bool,
    partial: bool,
    budget: u32,
) -> Result<Comparison, MatchError> {
    let mut work = Work::new(budget)?;
    let required = required.map(|v| v.0);
    let sample = sample.map(|v| v.0);
    let (a, b) = match (required, sample) {
        (None, None) => return Ok(work.result(true)),
        (Some(a), Some(b)) => (a, b),
        _ => return Ok(work.result(false)),
    };
    if std::ptr::eq(a, b) {
        return Ok(work.result(true));
    }
    if a.registry != b.registry || !(a.damage == b.damage || damageable || a.damage == 32767) {
        return Ok(work.result(false));
    }
    let matches = if nbt_check {
        compare(a.nbt.as_ref(), b.nbt.as_ref(), partial, &mut work)?
    } else {
        true
    };
    Ok(work.result(matches))
}
#[cfg(test)]
mod tests;
