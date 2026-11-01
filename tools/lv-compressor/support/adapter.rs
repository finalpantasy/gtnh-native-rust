//! Horizon host adapter for the verified low-voltage compressor policy.
//!
//! Host slots are the only physical item store. The policy's port inventories
//! persist the last published slot values so active output extraction can be
//! distinguished from an unsupported insertion.

use gtnh_lv_compressor_eval_basic_machine as basic_machine;
use gtnh_lv_compressor_eval_industrial_sim as industrial_sim;
use horizon_client::{containers::ModBlockEntity, items::ItemStack};
use horizon_world::nbt::{Compound, JavaString, Tag};
use industrial_sim::{
    content::{Amount, Catalog, Id},
    inventory::{Inventory, Limits},
};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

pub const BLOCK: &str = "gtnh_lv_compressor_eval:lv_compressor";

const CONTENT: &[u8] = include_bytes!("fixtures/compressor-reference-v1.json");
const MACHINE_KIND: &str = "reference:compressor";
const MAX_MACHINE_BYTES: usize = 16 * 1024;
const SCHEMA_KEY: &str = "LVBridgeSchema";
const CONTENT_KEY: &str = "LVBridgeContent";
const MACHINE_KEY: &str = "LVBridgeMachine";
const TICKS_KEY: &str = "LVBridgeTicks";
const TRACE_KEY: &str = "LVBridgeTrace";
const TRACE_LABELS: [&str; 3] = ["nominal", "shortage", "blocked_output"];

type Error = String;

/// Read-only machine and host-slot state exposed to menus and trace helpers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stats {
    pub stored_eu: u64,
    pub received_eu: u64,
    pub spent_eu: u64,
    pub wasted_eu: u64,
    pub progress: i32,
    pub max_progress: u32,
    pub eu_per_tick: u64,
    pub completed_batches: u64,
    pub held_input_quantity: u64,
    pub pending_output_count: u64,
    pub physical_input_count: u64,
    pub physical_output_count: u64,
    pub tick_count: u64,
}

struct Loaded {
    machine: basic_machine::Compressor,
    ticks: u64,
}

fn catalog() -> Result<&'static Catalog, Error> {
    static CATALOG: OnceLock<Result<Catalog, String>> = OnceLock::new();
    match CATALOG.get_or_init(|| Catalog::from_json(CONTENT).map_err(|error| error.to_string())) {
        Ok(catalog) => Ok(catalog),
        Err(error) => Err(error.clone()),
    }
}

fn port_limits() -> Limits {
    Limits {
        items: 64,
        fluid_mb: 0,
        kinds: 1,
    }
}

fn content_hash() -> Vec<u8> {
    Sha256::digest(CONTENT).to_vec()
}

fn tag_key(value: &str) -> JavaString {
    value.into()
}

fn trace_label(value: &JavaString) -> Option<&'static str> {
    TRACE_LABELS
        .iter()
        .copied()
        .find(|allowed| value.0.iter().copied().eq(allowed.encode_utf16()))
}

fn read_trace(entity: &ModBlockEntity) -> Result<Option<&'static str>, Error> {
    match entity.nbt.get(&tag_key(TRACE_KEY)) {
        None => Ok(None),
        Some(Tag::String(label)) => trace_label(label)
            .map(Some)
            .ok_or_else(|| "invalid LV compressor trace label".into()),
        Some(_) => Err("invalid LV compressor trace label".into()),
    }
}

fn validate_host_shape(entity: &ModBlockEntity) -> Result<(), Error> {
    if entity.id != BLOCK {
        return Err("entity is not owned by the LV compressor adapter".into());
    }
    if entity.items.len() > 3 {
        return Err("LV compressor has more than three host slots".into());
    }
    if entity
        .items
        .get(1)
        .is_some_and(|reserved| reserved.is_some())
    {
        return Err("LV compressor reserved slot 1 must remain empty".into());
    }
    if entity.nbt.contains_key(&tag_key("Items")) {
        return Err("LV compressor contains noncanonical raw Items NBT".into());
    }
    // The prior prototype has a different owner ID and schema. Reject its
    // marker set explicitly as well, so renaming a legacy save cannot migrate
    // its state by accident.
    if [
        "BridgeSchema",
        "BridgeContent",
        "BridgeMachine",
        "BridgeReceivedEU",
        "GTNHCompressorTestCharge",
    ]
    .iter()
    .any(|key| entity.nbt.contains_key(&tag_key(key)))
    {
        return Err("legacy compressor prototype state is unsupported".into());
    }
    read_trace(entity)?;
    Ok(())
}

fn stack_limit(item: &str) -> u32 {
    if item == "minecraft:snowball" {
        16
    } else {
        64
    }
}

const ALIASES: [(&str, &str); 11] = [
    ("reference:block_sand", "minecraft:sand"),
    ("reference:block_sandstone", "minecraft:sandstone"),
    ("reference:item_clay_ball", "minecraft:clay_ball"),
    ("reference:block_clay", "minecraft:clay"),
    ("reference:item_brick", "minecraft:brick"),
    ("reference:block_brick_block", "minecraft:bricks"),
    ("reference:item_netherbrick", "minecraft:nether_brick"),
    ("reference:block_nether_brick", "minecraft:nether_bricks"),
    ("reference:item_snowball", "minecraft:snowball"),
    ("reference:block_snow", "minecraft:snow_block"),
    ("reference:block_ice", "minecraft:ice"),
];

fn host_resource(stack: &ItemStack) -> Result<Id, Error> {
    if !(1..=stack_limit(&stack.item)).contains(&stack.count)
        || stack.damage != 0
        || stack.extra.is_some()
    {
        return Err("unsupported LV compressor host stack state".into());
    }
    let resource = ALIASES
        .iter()
        .find(|(_, host)| *host == &*stack.item)
        .ok_or("unmapped LV compressor host item")?
        .0;
    Id::new(resource)
}

fn host_slot(entity: &ModBlockEntity, index: usize) -> Result<Option<ItemStack>, Error> {
    let stack = entity.items.get(index).cloned().unwrap_or(None);
    if let Some(stack) = &stack {
        host_resource(stack)?;
    }
    Ok(stack)
}

fn amount_for_stack(stack: &ItemStack) -> Result<Amount, Error> {
    Ok(Amount {
        resource: host_resource(stack)?,
        quantity: u64::from(stack.count),
    })
}

fn amounts_in(port: &Inventory) -> Vec<Amount> {
    port.contents()
        .map(|(resource, quantity)| Amount {
            resource: resource.clone(),
            quantity,
        })
        .collect()
}

fn export(port: &Inventory) -> Result<Option<ItemStack>, Error> {
    let mut entries = port.contents();
    let Some((resource, quantity)) = entries.next() else {
        return Ok(None);
    };
    if entries.next().is_some() || !(1..=64).contains(&quantity) {
        return Err("LV compressor port cannot fit one host slot".into());
    }
    let host = ALIASES
        .iter()
        .find(|(native, _)| *native == resource.as_str())
        .ok_or("unmapped LV compressor policy item")?
        .1;
    if quantity > u64::from(stack_limit(host)) {
        return Err("LV compressor policy stack exceeds the host limit".into());
    }
    Ok(Some(ItemStack::new(host, quantity as u32)))
}

fn validate_machine(machine: &basic_machine::Compressor) -> Result<(), Error> {
    if machine.kind().as_str() != MACHINE_KIND
        || machine.inputs().limits() != port_limits()
        || machine.outputs().limits() != port_limits()
    {
        return Err("invalid LV compressor policy ownership or port definition".into());
    }
    Ok(())
}

fn decode(entity: &ModBlockEntity, content: &Catalog) -> Result<Loaded, Error> {
    validate_host_shape(entity)?;
    if !matches!(entity.nbt.get(&tag_key(SCHEMA_KEY)), Some(Tag::Int(1))) {
        return Err("unsupported LV compressor bridge schema".into());
    }
    match entity.nbt.get(&tag_key(CONTENT_KEY)) {
        Some(Tag::ByteArray(value)) if *value == content_hash() => {}
        _ => return Err("LV compressor content hash does not match".into()),
    }
    let bytes = match entity.nbt.get(&tag_key(MACHINE_KEY)) {
        Some(Tag::ByteArray(bytes)) if bytes.len() <= MAX_MACHINE_BYTES => bytes,
        _ => return Err("invalid or oversized LV compressor policy payload".into()),
    };
    let ticks = match entity.nbt.get(&tag_key(TICKS_KEY)) {
        Some(Tag::Long(value)) if *value >= 0 => *value as u64,
        _ => return Err("invalid LV compressor tick counter".into()),
    };
    let machine = basic_machine::Compressor::decode(bytes, content)?;
    validate_machine(&machine)?;
    Ok(Loaded { machine, ticks })
}

fn remove_active_output_delta(
    mirror: Option<ItemStack>,
    physical: Option<ItemStack>,
) -> Result<Vec<Amount>, Error> {
    match (mirror, physical) {
        (None, None) => Ok(Vec::new()),
        (None, Some(_)) => {
            Err("output insertion during active LV compressor work is unsupported".into())
        }
        (Some(previous), None) => Ok(vec![amount_for_stack(&previous)?]),
        (Some(previous), Some(current)) => {
            if previous.item != current.item
                || previous.damage != current.damage
                || previous.extra != current.extra
                || current.count > previous.count
            {
                return Err("output insertion or identity change during active LV compressor work is unsupported".into());
            }
            let removed = previous.count - current.count;
            if removed == 0 {
                Ok(Vec::new())
            } else {
                Ok(vec![Amount {
                    resource: host_resource(&previous)?,
                    quantity: u64::from(removed),
                }])
            }
        }
    }
}

fn reconcile(entity: &ModBlockEntity, content: &Catalog) -> Result<Loaded, Error> {
    validate_host_shape(entity)?;
    let input = host_slot(entity, 0)?;
    let output = host_slot(entity, 2)?;
    let mut loaded = decode(entity, content)?;

    // Validate both persisted mirrors against actual single-slot host
    // representability before editing either policy port.
    let _ = export(loaded.machine.inputs())?;
    let _ = export(loaded.machine.outputs())?;

    let old_inputs = amounts_in(loaded.machine.inputs());
    let new_inputs = input
        .as_ref()
        .map(amount_for_stack)
        .transpose()?
        .into_iter()
        .collect::<Vec<_>>();
    loaded
        .machine
        .edit_inputs(content, &old_inputs, &new_inputs)?;

    if loaded.machine.max_progress() != 0 {
        let old_output = export(loaded.machine.outputs())?;
        let removal = remove_active_output_delta(old_output, output)?;
        loaded.machine.edit_outputs(content, &removal, &[])?;
    } else {
        let old_outputs = amounts_in(loaded.machine.outputs());
        let new_outputs = output
            .as_ref()
            .map(amount_for_stack)
            .transpose()?
            .into_iter()
            .collect::<Vec<_>>();
        loaded
            .machine
            .edit_outputs(content, &old_outputs, &new_outputs)?;
    }
    Ok(loaded)
}

fn held_input_quantity(machine: &basic_machine::Compressor) -> u64 {
    machine
        .active_recipe()
        .map(|recipe| recipe.inputs.iter().map(|amount| amount.quantity).sum())
        .unwrap_or(0)
}

fn stats_from(loaded: &Loaded, entity: &ModBlockEntity) -> Result<Stats, Error> {
    let input = host_slot(entity, 0)?;
    let output = host_slot(entity, 2)?;
    Ok(Stats {
        stored_eu: loaded.machine.stored_eu(),
        received_eu: loaded.machine.received_eu(),
        spent_eu: loaded.machine.spent_eu(),
        wasted_eu: loaded.machine.wasted_work_eu(),
        progress: loaded.machine.progress(),
        max_progress: loaded.machine.max_progress(),
        eu_per_tick: loaded.machine.eu_per_tick(),
        completed_batches: loaded.machine.completed_batches(),
        held_input_quantity: held_input_quantity(&loaded.machine),
        pending_output_count: loaded.machine.pending_output_count(),
        physical_input_count: input.map_or(0, |stack| u64::from(stack.count)),
        physical_output_count: output.map_or(0, |stack| u64::from(stack.count)),
        tick_count: loaded.ticks,
    })
}

fn menu_from(stats: &Stats) -> [i32; 4] {
    [stats.stored_eu as i32, 2048, stats.progress, 300]
}

fn encode_machine(
    machine: &basic_machine::Compressor,
    content: &Catalog,
) -> Result<Vec<u8>, Error> {
    validate_machine(machine)?;
    let bytes = machine.encode(content)?;
    if bytes.len() > MAX_MACHINE_BYTES {
        return Err("LV compressor policy payload exceeds the save bound".into());
    }
    Ok(bytes)
}

fn publish(
    entity: &mut ModBlockEntity,
    machine: basic_machine::Compressor,
    ticks: u64,
    content: &Catalog,
) -> Result<(), Error> {
    let input = export(machine.inputs())?;
    let output = export(machine.outputs())?;
    let tick_tag = i64::try_from(ticks).map_err(|_| "LV compressor tick counter overflow")?;
    let machine_bytes = encode_machine(&machine, content)?;
    let mut staged = entity.clone();
    staged.items = vec![input, None, output];
    staged
        .nbt
        .insert(tag_key(MACHINE_KEY), Tag::ByteArray(machine_bytes));
    staged.nbt.insert(tag_key(TICKS_KEY), Tag::Long(tick_tag));
    let loaded = Loaded { machine, ticks };
    staged.data = menu_from(&stats_from(&loaded, &staged)?).to_vec();
    *entity = staged;
    Ok(())
}

/// Creates a canonical, uncharged entity with three host slots and no trace.
pub fn new_entity() -> Result<ModBlockEntity, Error> {
    let content = catalog()?;
    let machine =
        basic_machine::Compressor::new(Id::new(MACHINE_KIND)?, port_limits(), port_limits())?;
    let bytes = encode_machine(&machine, content)?;
    let mut entity = ModBlockEntity {
        id: BLOCK.into(),
        nbt: Compound::new(),
        items: vec![None, None, None],
        data: vec![0, 2048, 0, 300],
    };
    entity.nbt.insert(tag_key(SCHEMA_KEY), Tag::Int(1));
    entity
        .nbt
        .insert(tag_key(CONTENT_KEY), Tag::ByteArray(content_hash()));
    entity
        .nbt
        .insert(tag_key(MACHINE_KEY), Tag::ByteArray(bytes));
    entity.nbt.insert(tag_key(TICKS_KEY), Tag::Long(0));
    Ok(entity)
}

/// Reads validated state and reconciles a clone with the current host slots.
/// The entity, including host items and transient menu values, is unchanged.
pub fn stats(entity: &ModBlockEntity) -> Result<Stats, Error> {
    let content = catalog()?;
    let loaded = reconcile(entity, content)?;
    stats_from(&loaded, entity)
}

/// Reconstructs the furnace-style menu values while preserving signed progress.
pub fn menu_data(entity: &ModBlockEntity) -> Result<[i32; 4], Error> {
    Ok(menu_from(&stats(entity)?))
}

/// Refreshes transient menu data only; slot contents and policy bytes stay intact.
pub fn refresh_menu_data(entity: &mut ModBlockEntity) -> Result<(), Error> {
    let data = menu_data(entity)?.to_vec();
    let mut staged = entity.clone();
    staged.data = data;
    *entity = staged;
    Ok(())
}

/// Applies an explicit bounded fixture charge. No network or packet is modeled.
pub fn fixture_charge(entity: &mut ModBlockEntity, amount_eu: u64) -> Result<u64, Error> {
    let content = catalog()?;
    let mut loaded = reconcile(entity, content)?;
    loaded.machine.fixture_charge(amount_eu)?;
    publish(entity, loaded.machine, loaded.ticks, content)?;
    Ok(amount_eu)
}

/// Runs one policy tick and atomically publishes slots, policy state, tick count,
/// and refreshed transient menu values after all validation succeeds.
pub fn tick(entity: &mut ModBlockEntity) -> Result<basic_machine::TickOutcome, Error> {
    let content = catalog()?;
    let mut loaded = reconcile(entity, content)?;
    let next_ticks = loaded
        .ticks
        .checked_add(1)
        .filter(|ticks| *ticks <= i64::MAX as u64)
        .ok_or("LV compressor tick counter overflow")?;
    let outcome = loaded.machine.tick(content)?;
    publish(entity, loaded.machine, next_ticks, content)?;
    Ok(outcome)
}

/// Marks an opt-in trace before any charge or machine tick. Fixture inventory
/// may be present; this function changes only `LVBridgeTrace`.
pub fn mark_trace(entity: &mut ModBlockEntity, label: &str) -> Result<(), Error> {
    if !TRACE_LABELS.contains(&label) {
        return Err("unrecognized LV compressor trace label".into());
    }
    let content = catalog()?;
    let loaded = reconcile(entity, content)?;
    if loaded.ticks != 0
        || loaded.machine.stored_eu() != 0
        || loaded.machine.received_eu() != 0
        || loaded.machine.spent_eu() != 0
        || loaded.machine.wasted_work_eu() != 0
        || loaded.machine.completed_batches() != 0
        || loaded.machine.max_progress() != 0
        || loaded.machine.pending_output_count() != 0
    {
        return Err("LV compressor trace must be marked before work or energy".into());
    }
    let mut staged = entity.clone();
    staged
        .nbt
        .insert(tag_key(TRACE_KEY), Tag::String(label.into()));
    *entity = staged;
    Ok(())
}

/// Returns one stable ASCII numeric trace record for marked entities.
pub fn trace_line(entity: &ModBlockEntity) -> Result<Option<String>, Error> {
    let content = catalog()?;
    let Some(label) = read_trace(entity)? else {
        // Still validate ownership, save fields, policy bytes, and host slots.
        let _ = reconcile(entity, content)?;
        return Ok(None);
    };
    let loaded = reconcile(entity, content)?;
    let stats = stats_from(&loaded, entity)?;
    Ok(Some(format!(
        "GTNH_LV_SAMPLE label={label} tick={} stored={} progress={} max={} eut={} input={} output={} pending={} received={} spent={} wasted={} completed={}",
        stats.tick_count,
        stats.stored_eu,
        stats.progress,
        stats.max_progress,
        stats.eu_per_tick,
        stats.physical_input_count,
        stats.physical_output_count,
        stats.pending_output_count,
        stats.received_eu,
        stats.spent_eu,
        stats.wasted_eu,
        stats.completed_batches,
    )))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
