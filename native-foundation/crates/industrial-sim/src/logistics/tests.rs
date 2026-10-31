use super::*;
use crate::{content::Amount, inventory::Limits};
fn id(s: &str) -> Id {
    Id::new(format!("lab:{s}")).unwrap()
}
fn catalog() -> Catalog {
    Catalog::from_json(include_bytes!("../../../../fixtures/industrial-v1.json")).unwrap()
}
fn edge(n: u16, source: u8, target: u8, resource: &str, limit: u64) -> Edge {
    Edge {
        id: n,
        source,
        target,
        resource: id(resource),
        limit,
    }
}
fn def(edges: Vec<Edge>, attempts: u16, units: u64) -> Definition {
    Definition {
        edges,
        max_attempts: attempts,
        max_units: units,
    }
}
fn port(catalog: &Catalog, ore: u64, water: u64, items: u64, fluid_mb: u64) -> Inventory {
    let mut inv = Inventory::new(Limits {
        items,
        fluid_mb,
        kinds: 4,
    })
    .unwrap();
    for (resource, quantity) in [(id("iron_ore"), ore), (id("water"), water)] {
        if quantity > 0 {
            inv.apply(catalog, &[], &[Amount { resource, quantity }])
                .unwrap();
        }
    }
    inv
}
#[test]
fn arrivals_cannot_be_forwarded_again_in_the_same_tick() {
    let c = catalog();
    let d = def(
        vec![edge(0, 0, 1, "iron_ore", 8), edge(1, 1, 2, "iron_ore", 8)],
        2,
        100,
    );
    let mut n = Network::new(d, &c).unwrap();
    let mut p = BTreeMap::from([
        (0, port(&c, 8, 0, 8, 0)),
        (1, port(&c, 0, 0, 8, 0)),
        (2, port(&c, 0, 0, 8, 0)),
    ]);
    assert_eq!(n.step(&c, &mut p).unwrap().moved_units, 8);
    assert_eq!(p[&0].amount(&id("iron_ore")), 0);
    assert_eq!(p[&1].amount(&id("iron_ore")), 8);
    assert_eq!(p[&2].amount(&id("iron_ore")), 0);
    assert_eq!(n.step(&c, &mut p).unwrap().moved_units, 8);
    assert_eq!(p[&2].amount(&id("iron_ore")), 8);
}
#[test]
fn bounded_rotating_cursor_gives_both_split_outputs_opportunities() {
    let c = catalog();
    let mut n = Network::new(
        def(
            vec![edge(4, 0, 2, "iron_ore", 1), edge(1, 0, 1, "iron_ore", 1)],
            1,
            1,
        ),
        &c,
    )
    .unwrap();
    let mut p = BTreeMap::from([
        (0, port(&c, 20, 0, 20, 0)),
        (1, port(&c, 0, 0, 20, 0)),
        (2, port(&c, 0, 0, 20, 0)),
    ]);
    for tick in 0..20 {
        let r = n.step(&c, &mut p).unwrap();
        assert_eq!(r.attempts, 1);
        assert_eq!(r.moved_units, 1);
        assert_eq!(n.ticks(), tick + 1);
    }
    assert_eq!(p[&1].amount(&id("iron_ore")), 10);
    assert_eq!(p[&2].amount(&id("iron_ore")), 10);
    assert_eq!(n.cursor(), 0);
}
#[test]
fn filters_and_capacity_and_global_quantity_budget_are_enforced() {
    let c = catalog();
    let mut n = Network::new(
        def(
            vec![edge(0, 0, 1, "water", 100), edge(1, 0, 1, "iron_ore", 100)],
            2,
            6,
        ),
        &c,
    )
    .unwrap();
    let mut p = BTreeMap::from([(0, port(&c, 10, 100, 10, 100)), (1, port(&c, 0, 0, 2, 4))]);
    let r = n.step(&c, &mut p).unwrap();
    assert_eq!(r.attempts, 2);
    assert_eq!(r.moved_units, 6);
    assert_eq!(r.transfers, 2);
    assert_eq!(p[&0].amount(&id("iron_ore")), 8);
    assert_eq!(p[&0].amount(&id("water")), 96);
    assert_eq!(p[&1].amount(&id("water")), 4);
    assert_eq!(n.step(&c, &mut p).unwrap().moved_units, 0);
}
#[test]
fn unavailable_and_blocked_edges_consume_bounded_opportunities_without_loading() {
    let c = catalog();
    let mut n = Network::new(
        def(
            vec![
                edge(0, 0, 9, "iron_ore", 1),
                edge(1, 0, 1, "iron_ore", 1),
                edge(2, 0, 2, "iron_ore", 1),
            ],
            1,
            5,
        ),
        &c,
    )
    .unwrap();
    let mut p = BTreeMap::from([
        (0, port(&c, 10, 0, 10, 0)),
        (1, port(&c, 1, 0, 1, 0)),
        (2, port(&c, 0, 0, 10, 0)),
    ]);
    let before = p.clone();
    let r = n.step(&c, &mut p).unwrap();
    assert_eq!(r.unavailable, 1);
    assert_eq!(p, before);
    assert_eq!(n.step(&c, &mut p).unwrap().blocked, 1);
    assert_eq!(p, before);
    assert_eq!(n.step(&c, &mut p).unwrap().moved_units, 1);
    assert!(!p.contains_key(&9));
}
#[test]
fn cycles_preserve_original_supply_and_never_duplicate() {
    let c = catalog();
    let mut n = Network::new(
        def(
            vec![edge(0, 0, 1, "iron_ore", 10), edge(1, 1, 0, "iron_ore", 10)],
            2,
            20,
        ),
        &c,
    )
    .unwrap();
    let mut p = BTreeMap::from([(0, port(&c, 10, 0, 20, 0)), (1, port(&c, 0, 0, 20, 0))]);
    for tick in 0..40 {
        let r = n.step(&c, &mut p).unwrap();
        assert_eq!(r.moved_units, 10);
        assert_eq!(p[&(tick % 2)].amount(&id("iron_ore")), 0);
        assert_eq!(
            p.values().map(|i| i.amount(&id("iron_ore"))).sum::<u64>(),
            10
        );
    }
}
#[test]
fn malformed_and_excessive_graphs_are_rejected() {
    let c = catalog();
    let valid = def(vec![edge(0, 0, 1, "iron_ore", 1)], 1, 1);
    for d in [
        def(vec![], 1, 1),
        def(valid.edges.clone(), 0, 1),
        def(valid.edges.clone(), 1025, 1),
        def(valid.edges.clone(), 1, 0),
        def(vec![edge(0, 0, 0, "iron_ore", 1)], 1, 1),
        def(vec![edge(0, 0, 1, "iron_ore", 0)], 1, 1),
        def(
            vec![edge(0, 0, 1, "iron_ore", 1), edge(0, 1, 2, "water", 1)],
            1,
            1,
        ),
        def(
            vec![edge(0, 0, 1, "iron_ore", 1), edge(1, 0, 1, "iron_ore", 2)],
            1,
            1,
        ),
        def(
            (0..1025).map(|i| edge(i, 0, 1, "iron_ore", 1)).collect(),
            1,
            1,
        ),
        def(vec![edge(0, 0, 1, "missing", 1)], 1, 1),
    ] {
        assert!(Network::new(d, &c).is_err());
    }
}
#[test]
fn save_reload_binds_topology_kinds_and_normalized_order() {
    let c = catalog();
    let d = def(
        vec![edge(7, 0, 2, "water", 2), edge(0, 0, 1, "iron_ore", 1)],
        1,
        5,
    );
    let mut n = Network::new(d.clone(), &c).unwrap();
    let mut p = BTreeMap::from([
        (0, port(&c, 3, 10, 3, 10)),
        (1, port(&c, 0, 0, 10, 0)),
        (2, port(&c, 0, 0, 0, 10)),
    ]);
    n.step(&c, &mut p).unwrap();
    let bytes = n.encode(&c).unwrap();
    let mut reordered = d.clone();
    reordered.edges.reverse();
    let mut reopened = Network::decode(&bytes, &reordered, &c).unwrap();
    assert_eq!(n, reopened);
    let (mut a, mut b) = (p.clone(), p);
    assert_eq!(
        n.step(&c, &mut a).unwrap(),
        reopened.step(&c, &mut b).unwrap()
    );
    assert_eq!(a, b);
    assert_eq!(n, reopened);
    let mut changed = d.clone();
    changed.edges[0].limit += 1;
    assert!(Network::decode(&bytes, &changed, &c).is_err());
    let mut content: crate::content::Content =
        serde_json::from_slice(include_bytes!("../../../../fixtures/industrial-v1.json")).unwrap();
    content
        .resources
        .iter_mut()
        .find(|r| r.id == id("water"))
        .unwrap()
        .kind = Kind::Item;
    let changed_catalog = Catalog::new(content).unwrap();
    assert!(Network::decode(&bytes, &d, &changed_catalog).is_err());
}
#[test]
fn malformed_state_tick_overflow_and_invalid_ports_leave_everything_unchanged() {
    let c = catalog();
    let d = def(vec![edge(0, 0, 1, "iron_ore", 1)], 1, 1);
    let n = Network::new(d.clone(), &c).unwrap();
    let bytes = n.encode(&c).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for (key, value) in [
        ("schema_version", serde_json::json!(2)),
        ("cursor", serde_json::json!(1)),
        ("kinds", serde_json::json!([])),
    ] {
        let mut state = original.clone();
        state[key] = value;
        assert!(Network::decode(&serde_json::to_vec(&state).unwrap(), &d, &c).is_err());
    }
    assert!(Network::decode(&vec![b' '; 1024 * 1024 + 1], &d, &c).is_err());
    let duplicate = String::from_utf8(bytes.clone()).unwrap().replacen(
        "\"ticks\":0",
        "\"ticks\":0,\"ticks\":0",
        1,
    );
    assert!(Network::decode(duplicate.as_bytes(), &d, &c).is_err());
    let mut max = original.clone();
    max["ticks"] = serde_json::json!(u64::MAX);
    let mut n = Network::decode(&serde_json::to_vec(&max).unwrap(), &d, &c).unwrap();
    let mut p = BTreeMap::from([(0, port(&c, 2, 0, 2, 0)), (1, port(&c, 0, 0, 2, 0))]);
    let old = (n.clone(), p.clone());
    assert!(n.step(&c, &mut p).is_err());
    assert_eq!((n, p), old);
    let mut n = Network::new(d, &c).unwrap();
    let mut bad: serde_json::Value = serde_json::to_value(port(&c, 2, 0, 2, 0)).unwrap();
    bad["limits"]["items"] = serde_json::json!(1);
    let invalid: Inventory = serde_json::from_value(bad).unwrap();
    let mut p = BTreeMap::from([(0, invalid), (1, port(&c, 0, 0, 2, 0))]);
    let old = (n.clone(), p.clone());
    assert!(n.step(&c, &mut p).is_err());
    assert_eq!((n, p), old);
}

#[test]
fn maximum_quantities_never_wrap_and_reservations_cover_all_outgoing_edges() {
    let c = catalog();
    let mut n = Network::new(
        def(
            vec![
                edge(0, 0, 1, "iron_ore", u64::MAX),
                edge(1, 0, 2, "iron_ore", u64::MAX),
            ],
            2,
            u64::MAX,
        ),
        &c,
    )
    .unwrap();
    let mut p = BTreeMap::from([
        (0, port(&c, u64::MAX, 0, u64::MAX, 0)),
        (1, port(&c, 0, 0, u64::MAX - 1, 0)),
        (2, port(&c, 0, 0, u64::MAX, 0)),
    ]);
    let r = n.step(&c, &mut p).unwrap();
    assert_eq!(r.moved_units, u64::MAX);
    assert_eq!(r.attempts, 2);
    assert_eq!(p[&0].amount(&id("iron_ore")), 0);
    assert_eq!(p[&1].amount(&id("iron_ore")), u64::MAX - 1);
    assert_eq!(p[&2].amount(&id("iron_ore")), 1);
}

#[test]
fn maximum_graph_examines_each_edge_at_most_once_per_tick() {
    let c = catalog();
    let edges = (0..1024u16)
        .map(|i| {
            edge(
                i,
                (i / 255) as u8,
                ((i % 255 + 1 + i / 255) % 256) as u8,
                "iron_ore",
                1,
            )
        })
        .collect();
    let mut n = Network::new(def(edges, 1024, 100), &c).unwrap();
    let mut p = BTreeMap::new();
    let r = n.step(&c, &mut p).unwrap();
    assert_eq!(r.attempts, 1024);
    assert_eq!(r.unavailable, 1024);
    assert_eq!(n.cursor(), 0);
    assert!(p.is_empty());
}

#[test]
fn ten_thousand_seeded_ticks_conserve_each_resource_and_replay_across_restarts() {
    let c = catalog();
    let mut edges = Vec::new();
    for source in 0..16u8 {
        for skip in [1, 3] {
            for resource in ["iron_ore", "water"] {
                edges.push(edge(
                    edges.len() as u16,
                    source,
                    (source + skip) % 16,
                    resource,
                    7 + u64::from(source),
                ));
            }
        }
    }
    let d = def(edges, 11, 43);
    let mut a = Network::new(d.clone(), &c).unwrap();
    let mut reordered = d.clone();
    reordered.edges.reverse();
    let mut b = Network::new(reordered, &c).unwrap();
    let initial = (0..16u8)
        .map(|p| (p, port(&c, u64::from(p) * 3, u64::from(p) * 17, 500, 2000)))
        .collect::<BTreeMap<_, _>>();
    let (mut master_a, mut master_b) = (initial.clone(), initial);
    let total = |ports: &BTreeMap<PortId, Inventory>, name: &str| {
        ports
            .values()
            .map(|i| u128::from(i.amount(&id(name))))
            .sum::<u128>()
    };
    let totals = [total(&master_a, "iron_ore"), total(&master_a, "water")];
    let mut seed = 0x1234_5678u64;
    let mut moves = 0;
    for tick in 0..10_000 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let active = |ports: &BTreeMap<PortId, Inventory>| {
            ports
                .iter()
                .filter(|(id, _)| seed & (1u64 << u32::from(**id)) != 0)
                .map(|(&p, i)| (p, i.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        let (mut pa, mut pb) = (active(&master_a), active(&master_b));
        let ra = a.step(&c, &mut pa).unwrap();
        let rb = b.step(&c, &mut pb).unwrap();
        assert_eq!(ra, rb);
        assert!(ra.attempts <= 11 && ra.moved_units <= 43);
        moves += ra.transfers;
        master_a.extend(pa);
        master_b.extend(pb);
        assert_eq!(master_a, master_b);
        assert_eq!(total(&master_a, "iron_ore"), totals[0]);
        assert_eq!(total(&master_a, "water"), totals[1]);
        for inv in master_a.values() {
            inv.validate(&c).unwrap();
        }
        if tick % 17 == 0 {
            b = Network::decode(&b.encode(&c).unwrap(), &d, &c).unwrap();
        }
        assert_eq!(a, b);
    }
    assert!(moves > 10_000, "stress route must exercise real transfers");
    assert_eq!(a.ticks(), 10_000);
}
