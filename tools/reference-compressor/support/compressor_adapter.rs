//! Transactional owner adapter for the evaluation compressor's three-slot
//! Horizon host entity. Keep the industrial-sim policy in `super::bridge`.

use super::bridge;
use gtnh_compressor_eval_industrial_sim as industrial_sim;
use horizon_client::{containers::ModBlockEntity, items::ItemStack};
use horizon_world::nbt::Tag;

/// The pack-owned block ID. The bridge keeps its historical private ID.
pub const BLOCK: &str = "gtnh_compressor_eval:reference_compressor";
const TEST_CHARGE_MARKER: &str = "GTNHCompressorTestCharge";
type Error = String;

pub type Stats = bridge::Stats;

/// Creates an owned three-slot host entity with no implicit energy charge.
pub fn new_entity() -> Result<ModBlockEntity, Error> {
    let mut entity = bridge::new_entity()?;
    entity.id = BLOCK.into();
    entity.items = vec![None, None, None];
    Ok(entity)
}

/// Converts the owned three-slot entity to the bridge's two-slot view.
///
/// All validation is performed against a clone. In particular, malformed
/// `Items` retained by Horizon's save decoder must never be overwritten by a
/// successful bridge save.
fn bridge_view(entity: &ModBlockEntity) -> Result<ModBlockEntity, Error> {
    if entity.id != BLOCK {
        return Err("entity is not owned by the evaluation compressor".into());
    }
    if entity.items.len() > 3 {
        return Err("compressor has more than three host slots".into());
    }
    if entity
        .items
        .get(1)
        .is_some_and(|reserved| reserved.is_some())
    {
        return Err("compressor reserved slot 1 must remain empty".into());
    }
    if entity.nbt.contains_key(&"Items".into()) {
        return Err("compressor contains noncanonical raw Items NBT".into());
    }

    let mut view = entity.clone();
    view.id = bridge::BLOCK.into();
    view.items = vec![
        entity.items.first().cloned().unwrap_or(None),
        entity.items.get(2).cloned().unwrap_or(None),
    ];
    Ok(view)
}

/// Maps a successfully updated bridge clone back onto the complete owned host
/// entity. The reserved slot is always published empty.
fn owned_entity(original: &ModBlockEntity, mut view: ModBlockEntity) -> ModBlockEntity {
    let input = view.items.first().cloned().unwrap_or(None);
    let output = view.items.get(1).cloned().unwrap_or(None);
    let mut published = original.clone();
    published.id = BLOCK.into();
    published.nbt = view.nbt;
    published.data = std::mem::take(&mut view.data);
    published.items = vec![input, None, output];
    published
}

/// Reads validated persisted stats without changing the entity, including its
/// transient menu data.
pub fn stats(entity: &ModBlockEntity) -> Result<Stats, Error> {
    let view = bridge_view(entity)?;
    bridge::stats(&view)
}

/// Reconstructs the furnace-style menu data from validated persisted state.
pub fn menu_data(entity: &ModBlockEntity) -> Result<[i32; 4], Error> {
    let state = stats(entity)?;
    Ok([
        state.stored as i32,
        4096,
        state.progress.unwrap_or(0) as i32,
        300,
    ])
}

/// Refreshes the transient menu values after loading or before opening a menu.
/// Rejected entities are left byte-for-byte unchanged.
pub fn refresh_menu_data(entity: &mut ModBlockEntity) -> Result<(), Error> {
    let data = menu_data(entity)?;
    entity.data = data.to_vec();
    Ok(())
}

/// Adds one explicitly supplied, validator-bounded packet transactionally.
pub fn charge(entity: &mut ModBlockEntity, voltage: u64, amps: u64) -> Result<u64, Error> {
    let mut view = bridge_view(entity)?;
    let accepted = bridge::charge(&mut view, voltage, amps)?;
    let published = owned_entity(entity, view);
    *entity = published;
    Ok(accepted)
}

/// Runs one ordinary authoritative machine tick. Host slots and native escrow
/// are published together only after every bridge check and transition passes.
pub fn tick(entity: &mut ModBlockEntity) -> Result<industrial_sim::machine::Tick, Error> {
    let mut view = bridge_view(entity)?;
    let report = bridge::tick(&mut view)?;
    let published = owned_entity(entity, view);
    *entity = published;
    Ok(report)
}

/// Applies the evaluation-only 600-EU setup charge exactly once.
///
/// This is deliberately separate from `charge`: normal packet validation and
/// partial-power experiments continue to use `charge` directly.
pub fn test_charge_once(entity: &mut ModBlockEntity) -> Result<u64, Error> {
    let mut view = bridge_view(entity)?;
    let state = bridge::stats(&view)?;
    match entity.nbt.get(&TEST_CHARGE_MARKER.into()) {
        Some(Tag::Int(1)) => {
            if state.received < 600 {
                return Err("test-charge marker contradicts received energy".into());
            }
            return Ok(0);
        }
        Some(_) => return Err("malformed test-charge marker".into()),
        None => {}
    }
    if state.received != 0
        || state.stored != 0
        || state.completed != 0
        || state.progress.is_some()
        || state.held_inputs != 0
    {
        return Err("test charge requires a pristine uncharged machine".into());
    }

    let mut total = 0_u64;
    for _ in 0..75 {
        let accepted = bridge::charge(&mut view, 8, 1)?;
        if accepted != 8 {
            return Err("test-charge packet was not accepted in full".into());
        }
        total = total
            .checked_add(accepted)
            .ok_or("test-charge total overflow")?;
    }
    if total != 600 {
        return Err("test charge did not supply exactly 600 EU".into());
    }
    view.nbt.insert(TEST_CHARGE_MARKER.into(), Tag::Int(1));
    let published = owned_entity(entity, view);
    *entity = published;
    Ok(total)
}
