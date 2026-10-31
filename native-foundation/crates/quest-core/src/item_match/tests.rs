use super::*;
fn compound(rows: &[(&str, Tag)]) -> Tag {
    Tag::Compound(
        rows.iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}
fn matched(a: Option<&Tag>, b: Option<&Tag>, partial: bool) -> bool {
    compare_nbt(a, b, partial, DEFAULT_BUDGET).unwrap().matches
}
#[test]
fn empty_null_compound_and_list_are_equivalent_but_empty_arrays_are_typed_values() {
    let compound = Tag::Compound(BTreeMap::new());
    let list = Tag::List(vec![]);
    for a in [None, Some(&compound), Some(&list)] {
        for b in [None, Some(&compound), Some(&list)] {
            assert!(matched(a, b, false));
        }
    }
    assert!(!matched(None, Some(&Tag::ByteArray(vec![])), true));
    assert!(!matched(
        Some(&Tag::ByteArray(vec![])),
        Some(&Tag::IntArray(vec![])),
        false
    ));
    assert!(!matched(
        Some(&compound),
        Some(&super::tests::compound(&[("x", Tag::Int(1))])),
        true
    ));
}
#[test]
fn compound_required_keys_are_a_subset_even_when_partial_is_false() {
    let required = compound(&[("a", Tag::Int(1))]);
    let sample = compound(&[("a", Tag::Long(1)), ("extra", Tag::String("kept".into()))]);
    assert!(matched(Some(&required), Some(&sample), false));
    assert!(!matched(Some(&sample), Some(&required), false));
    assert!(!matched(
        Some(&required),
        Some(&compound(&[("a", Tag::Int(2))])),
        true
    ));
}
#[test]
fn numeric_types_keep_java_long_or_double_precision_nan_and_signed_zero_rules() {
    let ones = [
        Tag::Byte(1),
        Tag::Short(1),
        Tag::Int(1),
        Tag::Long(1),
        Tag::FloatBits(1f32.to_bits()),
        Tag::DoubleBits(1f64.to_bits()),
    ];
    for a in &ones {
        for b in &ones {
            assert!(matched(Some(a), Some(b), false));
        }
    }
    assert!(!matched(
        Some(&Tag::Long(9_007_199_254_740_993)),
        Some(&Tag::Long(9_007_199_254_740_992)),
        false
    ));
    assert!(matched(
        Some(&Tag::Long(9_007_199_254_740_993)),
        Some(&Tag::DoubleBits(9_007_199_254_740_992f64.to_bits())),
        false
    ));
    assert!(matched(
        Some(&Tag::FloatBits((-0f32).to_bits())),
        Some(&Tag::DoubleBits(0f64.to_bits())),
        false
    ));
    assert!(!matched(
        Some(&Tag::FloatBits(f32::NAN.to_bits())),
        Some(&Tag::FloatBits(f32::NAN.to_bits())),
        false
    ));
    assert!(matched(
        Some(&Tag::DoubleBits(f64::INFINITY.to_bits())),
        Some(&Tag::FloatBits(f32::INFINITY.to_bits())),
        false
    ));
}
#[test]
fn byte_and_int_arrays_are_unordered_multisets_with_duplicate_reservations() {
    for (required, same, missing, extra) in [
        (
            Tag::ByteArray(vec![1, 1, 2]),
            Tag::ByteArray(vec![2, 1, 1]),
            Tag::ByteArray(vec![1, 2, 2]),
            Tag::ByteArray(vec![2, 1, 3, 1]),
        ),
        (
            Tag::IntArray(vec![1, 1, 2]),
            Tag::IntArray(vec![2, 1, 1]),
            Tag::IntArray(vec![1, 2, 2]),
            Tag::IntArray(vec![2, 1, 3, 1]),
        ),
    ] {
        assert!(matched(Some(&required), Some(&same), false));
        assert!(!matched(Some(&required), Some(&missing), false));
        assert!(!matched(Some(&required), Some(&extra), false));
        assert!(matched(Some(&required), Some(&extra), true));
    }
}
#[test]
fn compound_lists_ignore_order_and_may_reuse_the_same_sample_entry() {
    let a = compound(&[("n", Tag::Int(1))]);
    let b = compound(&[("n", Tag::Int(2))]);
    assert!(matched(
        Some(&Tag::List(vec![a.clone(), b.clone()])),
        Some(&Tag::List(vec![b.clone(), a.clone()])),
        false
    ));
    assert!(matched(
        Some(&Tag::List(vec![a.clone(), a.clone()])),
        Some(&Tag::List(vec![a.clone(), b.clone()])),
        false
    ));
    assert!(!matched(
        Some(&Tag::List(vec![a.clone(), a.clone()])),
        Some(&Tag::List(vec![a.clone()])),
        false
    ));
    assert!(matched(
        Some(&Tag::List(vec![a])),
        Some(&Tag::List(vec![b.clone(), compound(&[("n", Tag::Int(1))])])),
        true
    ));
}
#[test]
fn noncompound_lists_follow_the_legacy_empty_compound_accessor_quirk() {
    let a = Tag::List(vec![Tag::Int(1), Tag::Int(2)]);
    let b = Tag::List(vec![
        Tag::String("different".into()),
        Tag::String("ignored".into()),
    ]);
    assert!(matched(Some(&a), Some(&b), false));
    assert!(!matched(
        Some(&a),
        Some(&Tag::List(vec![Tag::Int(99)])),
        true
    ));
}
fn stack(damage: i32, nbt: Option<Tag>) -> Stack {
    Stack {
        registry: "IC2:itemToolDrill".into(),
        damage,
        nbt,
    }
}
#[test]
fn stack_wildcard_is_required_side_only_and_counts_are_outside_this_api() {
    let a = stack(32767, None);
    let b = stack(3, None);
    assert!(
        stack_match(Some(&a), Some(&b), false, true, false, DEFAULT_BUDGET)
            .unwrap()
            .matches
    );
    assert!(
        !stack_match(Some(&b), Some(&a), false, true, false, DEFAULT_BUDGET)
            .unwrap()
            .matches
    );
    assert!(
        stack_match(Some(&b), Some(&a), true, true, false, DEFAULT_BUDGET)
            .unwrap()
            .matches
    );
    assert!(
        stack_match(None, None, false, true, false, DEFAULT_BUDGET)
            .unwrap()
            .matches
    );
    assert!(
        !stack_match(None, Some(&a), true, false, true, DEFAULT_BUDGET)
            .unwrap()
            .matches
    );
    let mut other = a.clone();
    other.registry = "ic2:itemToolDrill".into();
    assert!(
        !stack_match(Some(&a), Some(&other), true, false, true, DEFAULT_BUDGET)
            .unwrap()
            .matches
    );
}
#[test]
fn same_stack_object_shortcut_and_ignored_nbt_preserve_reference_behavior() {
    let a = stack(
        0,
        Some(compound(&[("nan", Tag::DoubleBits(f64::NAN.to_bits()))])),
    );
    let b = a.clone();
    assert!(
        stack_match(Some(&a), Some(&a), false, true, false, DEFAULT_BUDGET)
            .unwrap()
            .matches
    );
    assert!(
        !stack_match(Some(&a), Some(&b), false, true, false, DEFAULT_BUDGET)
            .unwrap()
            .matches
    );
    assert!(
        stack_match(Some(&a), Some(&b), false, false, false, DEFAULT_BUDGET)
            .unwrap()
            .matches
    );
}
#[test]
fn budget_exhaustion_and_invalid_values_are_errors_distinct_from_nonmatch() {
    let tag = compound(&[("a", Tag::Int(1)), ("b", Tag::Int(2))]);
    assert_eq!(
        compare_nbt(Some(&tag), Some(&tag), false, 1),
        Err(MatchError::QueryLimit)
    );
    for budget in [0, 1_000_001] {
        assert_eq!(
            compare_nbt(None, None, false, budget),
            Err(MatchError::InvalidBudget)
        );
    }
    let mut deep = Tag::Int(1);
    for _ in 0..17 {
        deep = Tag::List(vec![deep]);
    }
    assert_eq!(deep.validate(), Err(MatchError::DepthLimit));
    assert_eq!(
        Tag::IntArray(vec![0; 4096]).validate(),
        Err(MatchError::NodeLimit)
    );
    assert_eq!(
        Tag::String("x".repeat(16385)).validate(),
        Err(MatchError::ByteLimit)
    );
    assert_eq!(
        Tag::List(vec![Tag::Int(1), Tag::Byte(1)]).validate(),
        Err(MatchError::InvalidTag)
    );
    let mut invalid = stack(0, None);
    invalid.registry = "not an item".into();
    assert_eq!(
        stack_match(
            Some(&invalid),
            Some(&invalid),
            true,
            false,
            false,
            DEFAULT_BUDGET
        ),
        Err(MatchError::InvalidStack)
    );
}
#[test]
fn typed_codec_roundtrips_ieee_bits_and_rejects_duplicates_and_invalid_structures() {
    let tag = compound(&[
        ("f", Tag::FloatBits(f32::NAN.to_bits())),
        ("a", Tag::IntArray(vec![1, -1])),
    ]);
    assert_eq!(Tag::decode(&tag.encode().unwrap()).unwrap(), tag);
    let duplicate=br#"{"type":"compound","value":{"x":{"type":"int","value":1},"x":{"type":"int","value":2}}}"#;
    assert!(Tag::decode(duplicate).is_err());
    let invalid = Tag::List(vec![Tag::Int(1), Tag::Byte(1)]);
    assert!(invalid.encode().is_err());
    assert!(Tag::decode(&vec![b' '; 1024 * 1024 + 1]).is_err());
    assert!(Tag::decode(br#"{"type":"long_array","value":[1]}"#).is_err());
}
#[test]
fn adversarial_array_search_and_long_strings_exhaust_explicit_work_budgets() {
    let repeated = Tag::IntArray(vec![1; 1000]);
    assert_eq!(
        compare_nbt(Some(&repeated), Some(&repeated), false, DEFAULT_BUDGET),
        Err(MatchError::QueryLimit)
    );
    let enough = compare_nbt(Some(&repeated), Some(&repeated), false, 1_000_000).unwrap();
    assert!(enough.matches);
    assert_eq!(enough.operations, 500_501);
    let text = Tag::String("x".repeat(16384));
    assert_eq!(
        compare_nbt(Some(&text), Some(&text), false, 100),
        Err(MatchError::QueryLimit)
    );
    assert!(matched(Some(&text), Some(&text), false));
}
#[test]
fn aggregate_text_and_stack_root_bounds_are_validated_before_shortcuts() {
    let fields = (0..65)
        .map(|i| (format!("k{i}"), Tag::String("x".repeat(16384))))
        .collect();
    assert_eq!(Tag::Compound(fields).validate(), Err(MatchError::ByteLimit));
    let tag = Tag::Compound(BTreeMap::from([("x".repeat(257), Tag::Int(1))]));
    assert_eq!(tag.validate(), Err(MatchError::ByteLimit));
    let invalid = stack(0, Some(Tag::Int(1)));
    assert_eq!(
        stack_match(
            Some(&invalid),
            Some(&invalid),
            true,
            false,
            false,
            DEFAULT_BUDGET
        ),
        Err(MatchError::InvalidStack)
    );
    assert!(Tag::decode(br#"{"type":"int","value":1,"extra":2}"#).is_err());
}
