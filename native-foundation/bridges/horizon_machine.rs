//! Harness-only bridge. No block registration, menu, generator or game hook is installed.
use horizon_client::{containers::ModBlockEntity, items::ItemStack};
use horizon_world::nbt::{Compound, Tag};
use industrial_sim::{
    content::{Amount, Catalog, Id},
    inventory::{Inventory, Limits},
    machine::{Energy, Machine, Tick},
};
use sha2::{Digest, Sha256};
pub const BLOCK: &str = "gtnh_eval:reference_compressor";
const CONTENT: &[u8] = include_bytes!("../fixtures/compressor-reference-v1.json");
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
type Error = String;
#[derive(Debug, PartialEq, Eq)]
pub struct Stats {
    pub stored: u64,
    pub received: u64,
    pub completed: u64,
    pub progress: Option<u32>,
    pub held_inputs: u64,
}
fn catalog() -> Result<Catalog, Error> {
    Catalog::from_json(CONTENT)
}
fn limits() -> Limits {
    Limits {
        items: 64,
        fluid_mb: 0,
        kinds: 1,
    }
}
pub fn new_entity() -> Result<ModBlockEntity, Error> {
    let c = catalog()?;
    let machine = Machine::new(
        Id::new("reference:compressor")?,
        limits(),
        limits(),
        Energy::new(4096, 8, 1)?,
    )?;
    let mut entity = ModBlockEntity {
        id: BLOCK.into(),
        nbt: Compound::new(),
        items: vec![None, None],
        data: vec![],
    };
    entity.nbt.insert("BridgeSchema".into(), Tag::Int(1));
    entity.nbt.insert(
        "BridgeContent".into(),
        Tag::ByteArray(Sha256::digest(CONTENT).to_vec()),
    );
    save(&mut entity, machine, 0, &c)?;
    Ok(entity)
}
fn load(entity: &ModBlockEntity, c: &Catalog) -> Result<(Machine, u64), Error> {
    if entity.id != BLOCK
        || entity.items.len() > 2
        || !matches!(entity.nbt.get(&"BridgeSchema".into()), Some(Tag::Int(1)))
    {
        return Err("unsupported bridge entity/schema/slot shape".into());
    }
    match entity.nbt.get(&"BridgeContent".into()) {
        Some(Tag::ByteArray(value)) if *value == Sha256::digest(CONTENT).to_vec() => {}
        _ => return Err("changed bridge content".into()),
    }
    let bytes = match entity.nbt.get(&"BridgeMachine".into()) {
        Some(Tag::ByteArray(b)) if b.len() <= 16384 => b,
        _ => return Err("invalid bridge payload".into()),
    };
    let received = match entity.nbt.get(&"BridgeReceivedEU".into()) {
        Some(Tag::Long(n)) if *n >= 0 => *n as u64,
        _ => return Err("invalid received energy".into()),
    };
    let machine = Machine::decode(bytes, c)?;
    if machine.kind().as_str() != "reference:compressor"
        || machine.inputs().limits() != limits()
        || machine.outputs().limits() != limits()
        || machine.inputs().contents().next().is_some()
        || machine.outputs().contents().next().is_some()
        || machine.energy().capacity() != 4096
        || machine.energy().max_voltage() != 8
        || machine.energy().max_amps() != 1
    {
        return Err("invalid bridge ownership or machine definition".into());
    }
    let work =
        (u128::from(machine.completed()) * 300 + u128::from(machine.progress().unwrap_or(0))) * 2;
    if u128::from(machine.energy().stored()) + work != u128::from(received) {
        return Err("bridge energy ledger mismatch".into());
    }
    Ok((machine, received))
}
fn clear(port: &mut Inventory, c: &Catalog) -> Result<(), Error> {
    let remove = port
        .contents()
        .map(|(r, n)| Amount {
            resource: r.clone(),
            quantity: n,
        })
        .collect::<Vec<_>>();
    port.apply(c, &remove, &[])
}
fn save(
    entity: &mut ModBlockEntity,
    mut machine: Machine,
    received: u64,
    c: &Catalog,
) -> Result<(), Error> {
    clear(machine.inputs_mut(), c)?;
    clear(machine.outputs_mut(), c)?;
    entity.data = vec![
        machine.energy().stored() as i32,
        4096,
        machine.progress().unwrap_or(0) as i32,
        300,
    ];
    let bytes = machine.encode(c)?;
    if bytes.len() > 16384 || received > i64::MAX as u64 {
        return Err("bridge state exceeds bounds".into());
    }
    entity
        .nbt
        .insert("BridgeMachine".into(), Tag::ByteArray(bytes));
    entity
        .nbt
        .insert("BridgeReceivedEU".into(), Tag::Long(received as i64));
    Ok(())
}
pub fn stats(entity: &ModBlockEntity) -> Result<Stats, Error> {
    let (m, received) = load(entity, &catalog()?)?;
    Ok(Stats {
        stored: m.energy().stored(),
        received,
        completed: m.completed(),
        progress: m.progress(),
        held_inputs: m.held_inputs().iter().map(|a| a.quantity).sum(),
    })
}
pub fn charge(entity: &mut ModBlockEntity, voltage: u64, amps: u64) -> Result<u64, Error> {
    let c = catalog()?;
    let (mut m, received) = load(entity, &c)?;
    let accepted = m.energy_mut().receive(voltage, amps)?;
    let received = received
        .checked_add(accepted)
        .filter(|v| *v <= i64::MAX as u64)
        .ok_or("bridge received-energy overflow")?;
    let mut staged = entity.clone();
    save(&mut staged, m, received, &c)?;
    *entity = staged;
    Ok(accepted)
}
fn stack_limit(name: &str) -> u32 {
    // Checked through give/inventory on the pinned actual Horizon client.
    if name == "minecraft:snowball" { 16 } else { 64 }
}
fn import(port: &mut Inventory, stack: &Option<ItemStack>, c: &Catalog) -> Result<(), Error> {
    if let Some(stack) = stack {
        if !(1..=stack_limit(&stack.item)).contains(&stack.count)
            || stack.damage != 0
            || stack.extra.is_some()
        {
            return Err("unsupported bridge stack state".into());
        }
        let resource = ALIASES
            .iter()
            .find(|(_, name)| *name == &*stack.item)
            .ok_or("unmapped host item")?
            .0;
        port.apply(
            c,
            &[],
            &[Amount {
                resource: Id::new(resource)?,
                quantity: u64::from(stack.count),
            }],
        )?;
    }
    Ok(())
}
fn export(port: &Inventory) -> Result<Option<ItemStack>, Error> {
    let mut entries = port.contents();
    let Some((resource, n)) = entries.next() else {
        return Ok(None);
    };
    if entries.next().is_some() || !(1..=64).contains(&n) {
        return Err("bridge output cannot fit host slot".into());
    }
    let name = ALIASES
        .iter()
        .find(|(id, _)| *id == resource.as_str())
        .ok_or("unmapped native item")?
        .1;
    if n > u64::from(stack_limit(name)) {
        return Err("bridge item exceeds host stack limit".into());
    }
    Ok(Some(ItemStack::new(name, n as u32)))
}
pub fn tick(entity: &mut ModBlockEntity) -> Result<Tick, Error> {
    let c = catalog()?;
    let (mut m, received) = load(entity, &c)?;
    let mut staged = entity.clone();
    staged.items.resize(2, None);
    import(m.inputs_mut(), &staged.items[0], &c)?;
    import(m.outputs_mut(), &staged.items[1], &c)?;
    let report = m.tick(&c)?;
    staged.items[0] = export(m.inputs())?;
    staged.items[1] = export(m.outputs())?;
    save(&mut staged, m, received, &c)?;
    // Single host-entity publication: slots and native escrow never have separate owners.
    *entity = staged;
    Ok(report)
}
