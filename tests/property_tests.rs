//! Property-based tests using `proptest`.
//!
//! Each test asserts an invariant that must hold for ALL inputs,
//! not just specific hand-crafted cases. proptest auto-generates thousands
//! of random inputs and searches for counter-examples.

use epub_rs::parser::positions::ArchiveEntryLength;
use epub_rs::parser::positions::ReflowableStrategy;
use epub_rs::processor::normalize_path;
use proptest::prelude::*;
use std::str::FromStr;

// ── ArchiveEntryLength properties ─────────────────────────────────────────────

proptest! {
    /// position_count must NEVER return 0, regardless of file size.
    #[test]
    fn prop_position_count_always_at_least_one(n in 0u64..=100_000_000u64) {
        let s = ArchiveEntryLength { page_length: 1024 };
        prop_assert!(s.position_count(n) >= 1,
            "position_count({n}) returned 0 — must always be >= 1");
    }

    /// position_count must be monotonically non-decreasing:
    /// a larger file never produces fewer positions.
    #[test]
    fn prop_position_count_monotonic(a in 0u64..=10_000u64, b in 0u64..=10_000u64) {
        let s = ArchiveEntryLength { page_length: 1024 };
        if a <= b {
            prop_assert!(s.position_count(a) <= s.position_count(b),
                "position_count({a})={} > position_count({b})={} — must be monotonic",
                s.position_count(a), s.position_count(b));
        }
    }

    /// Ceil division formula: position_count(n) == max(ceil(n / page_len), 1).
    #[test]
    fn prop_position_count_matches_ceil_formula(
        n in 0u64..=1_000_000u64,
        page_len in 1usize..=4096usize,
    ) {
        let s = ArchiveEntryLength { page_length: page_len };
        let expected = n.div_ceil(page_len as u64).max(1) as usize;
        prop_assert_eq!(s.position_count(n), expected);
    }
}

// ── CFI path ordering properties ──────────────────────────────────────────────

use epub_rs::cfi::{CfiPath, CfiStep};

/// Build a CfiPath from a list of (index, has_assertion) pairs.
fn make_path(steps: &[(u32, bool)], local: &[(u32, bool)], offset: Option<u32>) -> CfiPath {
    CfiPath {
        steps: steps
            .iter()
            .map(|(i, a)| CfiStep::new(*i, a.then(|| format!("id{i}"))))
            .collect(),
        local_steps: if local.is_empty() {
            None
        } else {
            Some(
                local
                    .iter()
                    .map(|(i, a)| CfiStep::new(*i, a.then(|| format!("lid{i}"))))
                    .collect(),
            )
        },
        character_offset: offset,
        side: None,
    ..CfiPath::default()
    }
}

proptest! {
    /// CFI ordering must be anti-symmetric: if a > b then b < a.
    #[test]
    fn prop_cfi_ordering_antisymmetric(
        steps_a in prop::collection::vec((2u32..=20u32, any::<bool>()), 1..4usize),
        steps_b in prop::collection::vec((2u32..=20u32, any::<bool>()), 1..4usize),
    ) {
        let a = make_path(&steps_a, &[], None);
        let b = make_path(&steps_b, &[], None);
        let ord_ab = a.cmp(&b);
        let ord_ba = b.cmp(&a);
        prop_assert_eq!(ord_ab, ord_ba.reverse(),
            "CfiPath ordering not antisymmetric");
    }

    /// CFI ordering must be reflexive: a == a.
    #[test]
    fn prop_cfi_ordering_reflexive(
        steps in prop::collection::vec((2u32..=20u32, any::<bool>()), 1..4usize),
    ) {
        let a = make_path(&steps, &[], None);
        prop_assert_eq!(a.cmp(&a), std::cmp::Ordering::Equal);
    }

    /// Character offset breaks ties when base and local steps are identical.
    #[test]
    fn prop_char_offset_is_tiebreaker(
        steps in prop::collection::vec((2u32..=20u32, any::<bool>()), 1..3usize),
        off_a in 0u32..=1000u32,
        off_b in 0u32..=1000u32,
    ) {
        let a = make_path(&steps, &[], Some(off_a));
        let b = make_path(&steps, &[], Some(off_b));
        let expected = off_a.cmp(&off_b);
        prop_assert_eq!(a.cmp(&b), expected,
            "char offset should be the tiebreaker when steps are equal");
    }
}

// ── normalize_path properties ─────────────────────────────────────────────────

proptest! {
    /// normalize_path must never return a path containing ".." segments.
    /// (After normalization, all parent-dir traversal should be resolved.)
    #[test]
    fn prop_normalize_path_no_dotdot_in_output(
        base in "[a-z]{1,8}(/[a-z]{1,8}){0,3}",
        rel  in "([a-z]{1,6}\\.xhtml)",
    ) {
        let result = normalize_path(&base, &rel);
        prop_assert!(
            !result.contains(".."),
            "normalize_path({base:?}, {rel:?}) = {result:?} contains '..'",
        );
    }

    /// normalize_path must never produce backslashes (EPUB uses forward slashes only).
    #[test]
    fn prop_normalize_path_no_backslashes(
        base in "[a-z]{1,8}(/[a-z]{1,8}){0,3}",
        rel  in "[a-z]{1,8}\\.[a-z]{2,4}",
    ) {
        let result = normalize_path(&base, &rel);
        prop_assert!(
            !result.contains('\\'),
            "normalize_path returned path with backslash: {result:?}",
        );
    }
}

// ── CFI parse roundtrip properties ───────────────────────────────────────────

/// A set of well-formed CFI strings to roundtrip.
/// (proptest generating arbitrary CFI strings would produce mostly invalid ones;
/// parameterising from a valid base is more productive.)
const VALID_CFIS: &[&str] = &[
    "epubcfi(/6/4!/4/2/1:5)",
    "epubcfi(/6/4[chap01]!/4/2[s01]/1:0)",
    "epubcfi(/6/4!/4,/2/1:0,/2/1:10)",
    "epubcfi(/6/10!/4/2)",
    "epubcfi(/6/4[c1]!/4[b]/10[p]/2/1:3)",
    "epubcfi(/6/4!/4/2/1:0)",
    "epubcfi(/6/4!/4/100/1:999)",
];

#[test]
fn cfi_parse_serialize_roundtrip() {
    // For every well-formed CFI: parse → to_string → parse again → equal
    for &input in VALID_CFIS {
        let cfi = epub_rs::cfi::EpubCfi::from_str(input)
            .unwrap_or_else(|e| panic!("failed to parse {input:?}: {e}"));
        let serialized = cfi.to_string();
        let reparsed = epub_rs::cfi::EpubCfi::from_str(&serialized)
            .unwrap_or_else(|e| panic!("failed to reparse {serialized:?}: {e}"));
        assert_eq!(
            cfi, reparsed,
            "CFI roundtrip not equal.\n  input:      {input}\n  serialized: {serialized}",
        );
    }
}

// ── CfiPath ordering edge cases ───────────────────────────────────────────────

#[test]
fn cfi_local_steps_break_tie_when_base_equal() {
    // Same base steps, different local steps → local steps decide
    let short_local = make_path(&[(6, false), (4, false)], &[(4, false), (2, false)], None);
    let long_local = make_path(&[(6, false), (4, false)], &[(4, false), (10, false)], None);
    // /4/2 < /4/10 because step 2 < step 10
    assert!(
        short_local < long_local,
        "shorter local step index should compare less"
    );
}

#[test]
fn cfi_none_local_steps_equals_empty_local_steps() {
    let with_none = CfiPath {
        steps: vec![CfiStep::new(6, None)],
        local_steps: None,
        character_offset: None,
        side: None,
    ..CfiPath::default()
    };
    let with_empty = CfiPath {
        steps: vec![CfiStep::new(6, None)],
        local_steps: Some(vec![]),
        character_offset: None,
        side: None,
    ..CfiPath::default()
    };
    // Both have empty local steps — should compare equal
    assert_eq!(
        with_none.cmp(&with_empty),
        std::cmp::Ordering::Equal,
        "None local_steps should equal Some(vec![])"
    );
}

#[test]
fn cfi_longer_step_sequence_is_greater() {
    // /6/4 vs /6/4/2 — the longer path refers to a deeper node → greater
    let shorter = make_path(&[(6, false), (4, false)], &[], None);
    let longer = make_path(&[(6, false), (4, false), (2, false)], &[], None);
    assert!(shorter < longer, "/6/4 should be less than /6/4/2");
}
