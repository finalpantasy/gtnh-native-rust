use super::*;
fn body(cell: BlockPos, fraction: [f64; 3], size: [f64; 3]) -> Body {
    Body::new(Point::new(cell, fraction).unwrap(), size).unwrap()
}
fn home() -> Body {
    body(BlockPos::new(0, 0, 0), [0.25; 3], [0.5; 3])
}
fn close(actual: [f64; 3], expected: [f64; 3]) {
    for i in 0..3 {
        assert!(
            (actual[i] - expected[i]).abs() < 1e-9,
            "axis {i}: {} != {}",
            actual[i],
            expected[i]
        );
    }
}
#[test]
fn all_six_faces_stop_fast_motion_without_tunneling() {
    for (axis, sign, face) in [
        (0, 1., Face::NegativeX),
        (0, -1., Face::PositiveX),
        (1, 1., Face::NegativeY),
        (1, -1., Face::PositiveY),
        (2, 1., Face::NegativeZ),
        (2, -1., Face::PositiveZ),
    ] {
        let mut target = [0i64; 3];
        target[axis] = if sign > 0. { 2 } else { -2 };
        let target = BlockPos::new(target[0], target[1], target[2]);
        let mut delta = [0.; 3];
        delta[axis] = 4. * sign;
        let result = sweep(home(), delta, DEFAULT_STEPS, |p| {
            Some(if p == target { 7 } else { 0 })
        })
        .unwrap();
        let mut expected = [0.; 3];
        expected[axis] = 1.25 * sign;
        close(result.applied, expected);
        assert_eq!(result.contacts.len(), 1);
        assert_eq!(result.contacts[0].face, face);
        assert_eq!(result.contacts[0].position, target);
        assert_eq!(result.contacts[0].block, 7);
        assert!((result.contacts[0].time - 0.3125).abs() < 1e-12);
    }
}
#[test]
fn exact_endpoint_contacts_are_inclusive_on_all_six_faces() {
    for axis in 0..3 {
        for sign in [-1., 1.] {
            let mut target = [0; 3];
            target[axis] = if sign > 0. { 2 } else { -2 };
            let target = BlockPos::new(target[0], target[1], target[2]);
            let mut delta = [0.; 3];
            delta[axis] = sign * 1.25;
            let motion = sweep(home(), delta, DEFAULT_STEPS, |p| {
                Some(if p == target { 9 } else { 0 })
            })
            .unwrap();
            close(motion.applied, delta);
            assert_eq!(motion.contacts.len(), 1, "axis={axis} sign={sign}");
            assert_eq!(motion.contacts[0].time, 1.);
            assert_eq!(motion.contacts[0].position, target);
            assert_eq!(
                motion.contacts[0].face,
                face(axis, if sign > 0. { -1 } else { 1 })
            );
            assert_eq!(
                sweep(home(), delta, DEFAULT_STEPS, |p| (p != target).then_some(0)),
                Err(Error::Unavailable(target)),
            );
            let outward = delta.map(|v| -v);
            let away = sweep(motion.body, outward, DEFAULT_STEPS, |p| {
                Some(if p == target { 9 } else { 0 })
            })
            .unwrap();
            assert!(away.contacts.is_empty());
            close(away.applied, outward);
        }
    }
}
#[test]
fn simultaneous_floor_and_wall_contacts_keep_tangent_motion() {
    let start = body(BlockPos::new(0, 1, 0), [0.25, 0., 0.25], [0.5, 1., 0.5]);
    let motion = sweep(start, [4., -1., 3.], DEFAULT_STEPS, |p| {
        Some(if p.y == 0 || p.x == 2 { 1 } else { 0 })
    })
    .unwrap();
    close(motion.applied, [1.25, 0., 3.]);
    assert_eq!(motion.contacts.len(), 2);
    assert!(motion.contacts.iter().any(|h| h.face == Face::PositiveY));
    assert!(motion.contacts.iter().any(|h| h.face == Face::NegativeX));
}
#[test]
fn exact_corner_ties_block_all_three_axes_deterministically() {
    let motion = sweep(home(), [4.; 3], DEFAULT_STEPS, |p| {
        Some(if p.x == 2 || p.y == 2 || p.z == 2 {
            1
        } else {
            0
        })
    })
    .unwrap();
    close(motion.applied, [1.25; 3]);
    assert_eq!(motion.contacts.len(), 3);
    assert_eq!(
        motion.contacts.iter().map(|h| h.face).collect::<Vec<_>>(),
        [Face::NegativeX, Face::NegativeY, Face::NegativeZ]
    );
}
#[test]
fn flush_tangent_and_away_motion_do_not_stick_or_accumulate_penetration() {
    let mut start = body(BlockPos::new(1, 0, 0), [0.5, 0.25, 0.25], [0.5; 3]);
    let sample = |p: BlockPos| Some(if p.x == 2 { 1 } else { 0 });
    let tangent = sweep(start, [0., 0., 4.], DEFAULT_STEPS, sample).unwrap();
    close(tangent.applied, [0., 0., 4.]);
    assert!(tangent.contacts.is_empty());
    close(
        sweep(start, [-0.25, 0., 0.], DEFAULT_STEPS, sample)
            .unwrap()
            .applied,
        [-0.25, 0., 0.],
    );
    for _ in 0..1000 {
        let motion = sweep(start, [0.01, 0., 0.001], DEFAULT_STEPS, sample).unwrap();
        assert!(motion.applied[0].abs() < 1e-9);
        start = motion.body;
    }
    assert_eq!(start.minimum().cell().x, 1);
    assert!((start.minimum().fraction()[0] - 0.5).abs() < 1e-12);
}
#[test]
fn unknown_cells_are_errors_but_unknown_space_behind_a_solid_stop_is_not_entered() {
    assert!(matches!(
        sweep(home(), [1., 0., 0.], DEFAULT_STEPS, |_| None),
        Err(Error::Unavailable(_))
    ));
    for sign in [1., -1.] {
        let motion = sweep(home(), [4. * sign, 0., 0.], DEFAULT_STEPS, |p| {
            if (sign > 0. && p.x >= 3) || (sign < 0. && p.x <= -3) {
                None
            } else {
                Some(if p.x == if sign > 0. { 2 } else { -2 } {
                    1
                } else {
                    0
                })
            }
        })
        .unwrap();
        close(motion.applied, [1.25 * sign, 0., 0.]);
    }
}
#[test]
fn later_slide_failure_returns_no_partial_movement_result() {
    let start = home();
    let before = start;
    let result = sweep(start, [4., 0., 4.], DEFAULT_STEPS, |p| {
        if p.z == 4 {
            None
        } else {
            Some(if p.x == 2 { 1 } else { 0 })
        }
    });
    assert!(matches!(result, Err(Error::Unavailable(_))));
    assert_eq!(start, before);
}
#[test]
fn starting_overlap_is_explicit_even_when_trying_to_move_out() {
    assert_eq!(
        sweep(home(), [1., 0., 0.], DEFAULT_STEPS, |_| Some(5)),
        Err(Error::InitialOverlap {
            position: BlockPos::new(0, 0, 0),
            block: 5
        })
    );
}
#[test]
fn candidate_budget_and_invalid_inputs_never_masquerade_as_free_movement() {
    let start = home();
    let mut calls = 0;
    assert_eq!(
        sweep(start, [4., 0., 0.], 1, |_| {
            calls += 1;
            Some(0)
        }),
        Err(Error::QueryLimit)
    );
    assert!(calls <= 1);
    for delta in [[f64::NAN, 0., 0.], [f64::INFINITY, 0., 0.], [64.1, 0., 0.]] {
        let mut calls = 0;
        assert_eq!(
            sweep(start, delta, DEFAULT_STEPS, |_| {
                calls += 1;
                Some(0)
            }),
            Err(Error::InvalidDelta)
        );
        assert_eq!(calls, 0);
    }
    for budget in [0, 1_000_001] {
        assert_eq!(
            sweep(start, [0.; 3], budget, |_| Some(0)),
            Err(Error::InvalidBudget)
        );
    }
    for size in [
        [0.; 3],
        [-1., 1., 1.],
        [f64::NAN, 1., 1.],
        [17., 1., 1.],
        [1e-20, 1., 1.],
    ] {
        assert_eq!(Body::new(start.minimum(), size), Err(Error::InvalidBody));
    }
}
#[test]
fn translated_sweeps_preserve_fractional_motion_and_normals_near_integer_extremes() {
    let mut reference = None;
    for base in [
        BlockPos::new(0, 0, 0),
        BlockPos::new(-17, 16_000_000, -1),
        BlockPos::new(i64::MAX - 100, i64::MIN + 100, 1_000_000_000_000),
    ] {
        let start = body(base, [0.25; 3], [0.5; 3]);
        let motion = sweep(start, [4., -4., 3.], DEFAULT_STEPS, |p| {
            Some(if p.x == base.x + 2 { 1 } else { 0 })
        })
        .unwrap();
        close(motion.applied, [1.25, -4., 3.]);
        let normalized = (
            motion.applied,
            motion.body.minimum().fraction(),
            motion
                .contacts
                .iter()
                .map(|c| (c.face, c.time))
                .collect::<Vec<_>>(),
        );
        if let Some(expected) = &reference {
            assert_eq!(&normalized, expected);
        } else {
            reference = Some(normalized);
        }
    }
}
#[test]
fn integer_world_edges_fail_without_wrap_and_subnormal_movement_stays_finite() {
    let edge = body(BlockPos::new(i64::MAX, 0, 0), [0.25; 3], [0.5; 3]);
    assert_eq!(
        sweep(edge, [1., 0., 0.], DEFAULT_STEPS, |_| Some(0)),
        Err(Error::WorldBoundary)
    );
    assert_eq!(
        Body::new(
            Point::new(BlockPos::new(i64::MAX, 0, 0), [0.75; 3]).unwrap(),
            [0.5; 3]
        ),
        Err(Error::WorldBoundary)
    );
    let start = body(BlockPos::new(i64::MIN, 0, 0), [0., 0.25, 0.25], [0.5; 3]);
    assert_eq!(
        sweep(start, [-0.1, 0., 0.], DEFAULT_STEPS, |_| Some(0)),
        Err(Error::WorldBoundary)
    );
    let motion = sweep(start, [1e-300, 0., 0.], DEFAULT_STEPS, |_| Some(0)).unwrap();
    assert_eq!(motion.applied[0], 1e-300);
    assert_eq!(motion.body.minimum().fraction()[0], 1e-300);
}
#[test]
fn sub_ulp_negative_motion_at_a_cell_plane_rounds_to_a_valid_point() {
    let start = body(BlockPos::new(1, 0, 0), [0., 0.25, 0.25], [0.5; 3]);
    for tiny in [f64::from_bits(1), 1e-300, 1e-17] {
        let motion = sweep(start, [-tiny, 0.25, 0.], DEFAULT_STEPS, |_| Some(0)).unwrap();
        assert_eq!(motion.body.minimum().cell().x, 1);
        assert_eq!(motion.body.minimum().fraction()[0], 0.);
        close(motion.applied, [0., 0.25, 0.]);
        assert!(motion.contacts.is_empty());
    }
}
#[test]
fn isolated_corner_contacts_are_retained_alongside_an_independent_surface() {
    let a = sweep(home(), [4., 4., 0.], DEFAULT_STEPS, |p| {
        Some(if p == BlockPos::new(2, 2, 0) { 1 } else { 0 })
    })
    .unwrap();
    close(a.applied, [1.25, 1.25, 0.]);
    assert_eq!(a.contacts.len(), 2);
    let b = sweep(home(), [4.; 3], DEFAULT_STEPS, |p| {
        Some(if p == BlockPos::new(2, 2, 1) || p.z == 2 {
            1
        } else {
            0
        })
    })
    .unwrap();
    close(b.applied, [1.25; 3]);
    assert_eq!(b.contacts.len(), 3);
}
#[test]
fn tolerance_sized_penetration_can_exit_and_clip_inward_without_teleporting() {
    for axis in 0..3 {
        for high_side in [false, true] {
            for factor in [0., 0.25, 0.5, 1.] {
                let mut cell = [0i64; 3];
                let mut fraction = [0.25; 3];
                cell[axis] = if high_side { 2 } else { 1 };
                fraction[axis] = if high_side {
                    1. - POSITION_EPSILON * factor
                } else {
                    0.5 + POSITION_EPSILON * factor
                };
                // factor == 0 on the high side is the exact next cell boundary.
                let start = Body::new(
                    point(BlockPos::new(cell[0], cell[1], cell[2]), fraction).unwrap(),
                    [0.5; 3],
                )
                .unwrap();
                let away = if high_side { 1. } else { -1. };
                let sample = |p: BlockPos| Some(u32::from(coordinates(p)[axis] == 2));
                let mut delta = [0.; 3];
                delta[axis] = away * 0.1;
                let result = sweep(start, delta, DEFAULT_STEPS, sample).unwrap();
                close(result.applied, delta);
                assert!(result.contacts.is_empty());
                let tangent = (axis + 1) % 3;
                delta = [0.; 3];
                delta[axis] = -away * 1e-10;
                delta[tangent] = 4.;
                let result = sweep(start, delta, DEFAULT_STEPS, sample).unwrap();
                assert!(result.applied[axis].abs() <= 2. * POSITION_EPSILON);
                assert!((result.applied[tangent] - 4.).abs() < 1e-9);
                assert_eq!(result.contacts.len(), 1);
                assert!(
                    (floats(result.body)[axis] - if high_side { 3. } else { 1.5 }).abs() < 1e-12
                );
            }
        }
    }
}
fn random(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}
fn floats(body: Body) -> [f64; 3] {
    let c = coordinates(body.minimum().cell());
    std::array::from_fn(|i| c[i] as f64 + body.minimum().fraction()[i])
}
fn overlap(body: Body, cell: BlockPos) -> bool {
    let p = floats(body);
    let c = coordinates(cell);
    (0..3).all(|i| {
        p[i] + body.size()[i] > c[i] as f64 + 2. * POSITION_EPSILON
            && p[i] < c[i] as f64 + 1. - 2. * POSITION_EPSILON
    })
}
#[test]
fn three_thousand_sweeps_match_an_independent_expanded_box_time_oracle() {
    fn analytic(start: [f64; 3], size: [f64; 3], delta: [f64; 3], cell: BlockPos) -> Option<f64> {
        let c = coordinates(cell);
        let (mut enter, mut exit) = (f64::NEG_INFINITY, f64::INFINITY);
        for i in 0..3 {
            let lo = c[i] as f64 - size[i];
            let hi = c[i] as f64 + 1.;
            if delta[i] == 0. {
                if start[i] <= lo || start[i] >= hi {
                    return None;
                }
            } else {
                let a = (lo - start[i]) / delta[i];
                let b = (hi - start[i]) / delta[i];
                enter = enter.max(a.min(b));
                exit = exit.min(a.max(b));
            }
        }
        if enter <= 1. && exit > enter.max(0.) {
            Some(enter.max(0.))
        } else {
            None
        }
    }
    let mut seed = 0x9548_2367_1234;
    let mut checked = 0;
    while checked < 3000 {
        let origin = std::array::from_fn::<_, 3, _>(|_| (random(&mut seed) % 9) as i64 - 4);
        let target = std::array::from_fn::<_, 3, _>(|_| (random(&mut seed) % 9) as i64 - 4);
        let start = body(
            BlockPos::new(origin[0], origin[1], origin[2]),
            [0.17, 0.39, 0.61],
            [0.6, 1.8, 0.6],
        );
        let target = BlockPos::new(target[0], target[1], target[2]);
        if overlap(start, target) {
            continue;
        }
        let delta = std::array::from_fn(|_| (random(&mut seed) % 321) as f64 / 20. - 8.);
        let expected = analytic(floats(start), start.size(), delta, target);
        let mut calls = 0;
        let result = sweep(start, delta, 100_000, |p| {
            calls += 1;
            Some(if p == target { 1 } else { 0 })
        })
        .unwrap();
        assert_eq!(result.queries, calls);
        assert!(result.queries <= result.tested && result.tested <= 100_000);
        assert!(
            !overlap(result.body, target),
            "scenario {checked} penetrated obstacle"
        );
        match expected {
            Some(time) => {
                let found = result.contacts.first().expect("analytic impact missing");
                assert!(
                    (found.time - time).abs() < 1e-9,
                    "scenario {checked}: {} != {time}",
                    found.time
                );
                assert_eq!(found.position, target);
            }
            None => {
                assert!(
                    result.contacts.is_empty(),
                    "unexpected impact in scenario {checked}"
                );
                close(result.applied, delta);
            }
        }
        checked += 1;
    }
}
#[test]
fn fifteen_hundred_sparse_world_sweeps_end_without_penetration_or_extra_motion() {
    use std::collections::BTreeSet;
    let mut seed = 0x1398_7654_4321;
    let mut checked = 0;
    while checked < 1500 {
        let mut solids = BTreeSet::new();
        for _ in 0..24 {
            let c = std::array::from_fn::<_, 3, _>(|_| (random(&mut seed) % 11) as i64 - 5);
            solids.insert((c[0], c[1], c[2]));
        }
        let start = body(BlockPos::new(0, 0, 0), [0.17, 0.39, 0.61], [0.6, 1.8, 0.6]);
        if solids
            .iter()
            .any(|&(x, y, z)| overlap(start, BlockPos::new(x, y, z)))
        {
            continue;
        }
        let delta = std::array::from_fn(|_| (random(&mut seed) % 321) as f64 / 20. - 8.);
        let result = sweep(start, delta, 100_000, |p| {
            Some(u32::from(solids.contains(&(p.x, p.y, p.z))))
        })
        .unwrap();
        assert!(result.contacts.len() <= 3);
        assert!(
            result
                .contacts
                .windows(2)
                .all(|w| w[0].time <= w[1].time + TIME_EPSILON)
        );
        for &(x, y, z) in &solids {
            assert!(
                !overlap(result.body, BlockPos::new(x, y, z)),
                "sparse scenario {checked} penetrated solid"
            );
        }
        for (i, &requested) in delta.iter().enumerate() {
            assert!(result.applied[i].abs() <= requested.abs() + POSITION_EPSILON);
            assert!(result.applied[i] * requested >= -POSITION_EPSILON);
        }
        if result.contacts.is_empty() {
            close(result.applied, delta);
        }
        checked += 1;
    }
}

#[test]
fn varied_bodies_follow_nonpenetrating_piecewise_paths_through_sparse_worlds() {
    // Independent open-volume slab oracle over every segment, rather than only the
    // final pose: tunneling through a block and exiting it must also fail this check.
    fn penetrates(start: [f64; 3], size: [f64; 3], delta: [f64; 3], solid: [i64; 3]) -> bool {
        let (mut entry, mut exit) = (0f64, 1f64);
        for i in 0..3 {
            let low = solid[i] as f64 - size[i] + 1e-9;
            let high = solid[i] as f64 + 1. - 1e-9;
            if delta[i] == 0. {
                if start[i] <= low || start[i] >= high {
                    return false;
                }
            } else {
                let a = (low - start[i]) / delta[i];
                let b = (high - start[i]) / delta[i];
                entry = entry.max(a.min(b));
                exit = exit.min(a.max(b));
            }
        }
        entry < exit
    }
    let mut seed = 0x8573_5473_2468;
    let mut checked = 0;
    let sizes = [1e-6, 0.125, 0.5, 1., 1.8, 3.5];
    let fractions = [0., 0.125, 0.5, 0.999_999_999_999];
    while checked < 2500 {
        let size = std::array::from_fn(|_| sizes[(random(&mut seed) % 6) as usize]);
        let fraction = std::array::from_fn(|_| fractions[(random(&mut seed) % 4) as usize]);
        let start = body(BlockPos::new(0, 0, 0), fraction, size);
        let solids: Vec<[i64; 3]> = (0..32)
            .map(|_| std::array::from_fn(|_| (random(&mut seed) % 15) as i64 - 7))
            .collect();
        if solids
            .iter()
            .any(|&c| penetrates(fraction, size, [0.; 3], c))
        {
            continue;
        }
        let delta = std::array::from_fn(|_| (random(&mut seed) % 129) as f64 / 8. - 8.);
        let result = sweep(start, delta, 100_000, |p| {
            Some(u32::from(solids.contains(&coordinates(p))))
        })
        .unwrap();
        let mut position = fraction;
        let mut velocity = delta;
        let mut time = 0.;
        let mut blocked = [false; 3];
        for contact in &result.contacts {
            let displacement = velocity.map(|v| v * (contact.time - time));
            for &solid in &solids {
                assert!(
                    !penetrates(position, size, displacement, solid),
                    "scenario {checked} tunnels before contact: {start:?} {delta:?} {result:?} solid={solid:?}"
                );
            }
            for i in 0..3 {
                position[i] += displacement[i];
                if contact.face.normal()[i] != 0 {
                    assert!(!blocked[i], "one axis cannot be blocked twice");
                    blocked[i] = true;
                    velocity[i] = 0.;
                }
            }
            time = contact.time;
        }
        let displacement = velocity.map(|v| v * (1. - time));
        for &solid in &solids {
            assert!(
                !penetrates(position, size, displacement, solid),
                "scenario {checked} tunnels after contact: {start:?} {delta:?} {result:?} solid={solid:?}"
            );
        }
        close(
            floats(result.body),
            std::array::from_fn(|i| position[i] + displacement[i]),
        );
        checked += 1;
    }
}
