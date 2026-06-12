use super::model::*;
use super::parser::*;
use std::str::FromStr;

#[test]
fn test_spine_base_cfi_format() {
    // Verify format: /6/<even_index>[id]!
    assert_eq!(EpubCfi::generate_spine_base_cfi(0, "ch1"), "/6/2[ch1]!");
    assert_eq!(EpubCfi::generate_spine_base_cfi(1, "ch2"), "/6/4[ch2]!");
    assert_eq!(EpubCfi::generate_spine_base_cfi(4, "ch5"), "/6/10[ch5]!");
}

#[test]
fn test_cfi_step_numeric_ordering() {
    let s2 = CfiStep::new(2, None);
    let s10 = CfiStep::new(10, None);
    // Numeric: 10 > 2, not lexicographic
    assert!(s10 > s2);
    assert!(s2 < s10);
}

#[test]
fn test_cfi_path_ordering() {
    let p4 = CfiPath {
        steps: vec![CfiStep::new(6, None), CfiStep::new(4, None)],
        local_steps: None,
        character_offset: None,
        side: None,
        ..CfiPath::default()
    };
    let p10 = CfiPath {
        steps: vec![CfiStep::new(6, None), CfiStep::new(10, None)],
        local_steps: None,
        character_offset: None,
        side: None,
        ..CfiPath::default()
    };
    assert!(p4 < p10);
}

#[test]
fn test_cfi_point_comparison() {
    let a = EpubCfi::from_str("epubcfi(/6/2!/4/2:5)").unwrap();
    let b = EpubCfi::from_str("epubcfi(/6/2!/4/10:1)").unwrap();
    assert!(a < b);

    let a = EpubCfi::from_str("epubcfi(/6/2!/4/2:5)").unwrap();
    let b = EpubCfi::from_str("epubcfi(/6/4!/4/2:0)").unwrap();
    assert!(a < b); // chapter 2 comes before chapter 4
}

#[test]
fn test_generate_range_same_document() {
    let start = EpubCfi::from_str("epubcfi(/6/4[chap01]!/4/2:5)").unwrap();
    let end = EpubCfi::from_str("epubcfi(/6/4[chap01]!/4/2:10)").unwrap();
    let range = EpubCfi::generate_range(&start, &end).unwrap();
    // Both in same document, same local path → parent = /6/4[chap01]!/4/2
    assert_eq!(range, "epubcfi(/6/4[chap01]!/4/2,:5,:10)");
}

#[test]
fn test_generate_range_cross_document() {
    let start = EpubCfi::from_str("epubcfi(/6/2[ch1]!/4/2:0)").unwrap();
    let end = EpubCfi::from_str("epubcfi(/6/4[ch2]!/4/2:0)").unwrap();
    let range = EpubCfi::generate_range(&start, &end).unwrap();
    // Different base paths → shared = /6
    assert_eq!(range, "epubcfi(/6,/2[ch1]!/4/2:0,/4[ch2]!/4/2:0)");
}

#[test]
fn test_generate_range_requires_points() {
    let range_cfi = EpubCfi::from_str("epubcfi(/6/4,/2:1,/2:5)").unwrap();
    let point = EpubCfi::from_str("epubcfi(/6/4!/4/2:5)").unwrap();
    assert!(EpubCfi::generate_range(&range_cfi, &point).is_err());
}

// ── CfiStep helpers ──────────────────────────────────────────────────────

#[test]
fn test_is_text_node() {
    // Even CFI index → element
    assert!(!CfiStep::new(2, None).is_text_node());
    assert!(!CfiStep::new(4, None).is_text_node());
    // Odd CFI index → text
    assert!(CfiStep::new(1, None).is_text_node());
    assert!(CfiStep::new(3, None).is_text_node());
}

#[test]
fn test_child_index_element() {
    // epub.js formula: index = cfi_step / 2 - 1
    assert_eq!(CfiStep::new(2, None).child_index(), 0); // first element child
    assert_eq!(CfiStep::new(4, None).child_index(), 1); // second element child
    assert_eq!(CfiStep::new(10, None).child_index(), 4); // fifth element child
}

#[test]
fn test_child_index_text() {
    // epub.js formula: index = (cfi_step - 1) / 2
    assert_eq!(CfiStep::new(1, None).child_index(), 0); // first text node
    assert_eq!(CfiStep::new(3, None).child_index(), 1); // second text node
    assert_eq!(CfiStep::new(5, None).child_index(), 2); // third text node
}

#[test]
fn test_to_resolved_step_element_with_id() {
    let step = CfiStep::new(4, Some("body01".into()));
    let resolved = step.to_resolved_step();
    assert_eq!(resolved.node_type, NodeType::Element);
    assert_eq!(resolved.index, 1);
    assert_eq!(resolved.id, Some("body01".into()));
}

#[test]
fn test_to_resolved_step_text() {
    let step = CfiStep::new(3, None);
    let resolved = step.to_resolved_step();
    assert_eq!(resolved.node_type, NodeType::Text);
    assert_eq!(resolved.index, 1);
    assert_eq!(resolved.id, None);
}

// ── CfiPath::resolve ─────────────────────────────────────────────────────

#[test]
fn test_resolve_no_local_steps_returns_none() {
    // A base-only CFI (no '!') has no local path → None
    let path = CfiPath {
        steps: vec![CfiStep::new(6, None), CfiStep::new(4, Some("ch1".into()))],
        local_steps: None,
        character_offset: None,
        side: None,
        ..CfiPath::default()
    };
    assert!(path.resolve(0).is_none());
}

#[test]
fn test_resolve_element_steps_xpath() {
    // epubcfi(/6/4[chap01]!/4[body01]/10[para05]/2)
    // local steps: /4[body01] /10[para05] /2
    let path = CfiPath {
        steps: vec![
            CfiStep::new(6, None),
            CfiStep::new(4, Some("chap01".into())),
        ],
        local_steps: Some(vec![
            CfiStep::new(4, Some("body01".into())),
            CfiStep::new(10, Some("para05".into())),
            CfiStep::new(2, None),
        ]),
        character_offset: None,
        side: None,
        ..CfiPath::default()
    };
    let resolved = path.resolve(1).unwrap();

    // XPath: epub.js stepsToXpath verbatim
    assert_eq!(
        resolved.xpath,
        "./*/*[position()=2 and @id='body01']/*[position()=5 and @id='para05']/*[1]"
    );
    assert_eq!(
        resolved.xpath_ns_agnostic,
        "./*[local-name()]/*[local-name()][position()=2 and @id='body01']/*[local-name()][position()=5 and @id='para05']/*[local-name()][1]"
    );
    // id_shortcut: deepest id (para05, not body01)
    assert_eq!(resolved.id_shortcut, Some("para05".into()));
    assert!(!resolved.is_text_node);
    assert_eq!(resolved.character_offset, None);
    assert_eq!(resolved.spine_index, 1);

    // steps: three decoded steps
    assert_eq!(resolved.steps.len(), 3);
    assert_eq!(resolved.steps[0].node_type, NodeType::Element);
    assert_eq!(resolved.steps[0].index, 1); // step=4 → children[1]
    assert_eq!(resolved.steps[2].node_type, NodeType::Element);
    assert_eq!(resolved.steps[2].index, 0); // step=2 → children[0]
}

#[test]
fn test_resolve_text_step_and_offset() {
    // epubcfi(/6/4[chap01]!/4/2/1:3)
    // local: /4 /2 /1  offset=3
    let path = CfiPath {
        steps: vec![
            CfiStep::new(6, None),
            CfiStep::new(4, Some("chap01".into())),
        ],
        local_steps: Some(vec![
            CfiStep::new(4, None), // element, index=1
            CfiStep::new(2, None), // element, index=0
            CfiStep::new(1, None), // TEXT,    index=0
        ]),
        character_offset: Some(3),
        side: None,
        ..CfiPath::default()
    };
    let resolved = path.resolve(1).unwrap();

    // XPath ends with text()[1]
    assert!(resolved.xpath.ends_with("/text()[1]"));
    assert!(resolved.xpath_ns_agnostic.ends_with("/text()[1]"));
    // is_text_node must be true
    assert!(resolved.is_text_node);
    // character_offset propagated
    assert_eq!(resolved.character_offset, Some(3));
    // No id anywhere → no shortcut
    assert_eq!(resolved.id_shortcut, None);
    // Last step is text
    assert_eq!(resolved.steps.last().unwrap().node_type, NodeType::Text);
    assert_eq!(resolved.steps.last().unwrap().index, 0);
}

#[test]
fn test_resolve_id_shortcut_deepest_wins() {
    // Two element steps both carrying ids; deepest should be the shortcut
    let path = CfiPath {
        steps: vec![CfiStep::new(6, None), CfiStep::new(4, None)],
        local_steps: Some(vec![
            CfiStep::new(4, Some("section1".into())),
            CfiStep::new(6, Some("para99".into())),
        ]),
        character_offset: None,
        side: None,
        ..CfiPath::default()
    };
    let resolved = path.resolve(0).unwrap();
    assert_eq!(resolved.id_shortcut, Some("para99".into())); // deepest wins
}

// ── EpubCfi::resolve ──────────────────────────────────────────────────────

#[test]
fn test_epubcfi_resolve_point() {
    // epubcfi(/6/4[chap01]!/4[body01]/10[para05]/2/1:3)
    let cfi = EpubCfi::from_str("epubcfi(/6/4[chap01]!/4[body01]/10[para05]/2/1:3)").unwrap();
    let resolution = cfi.resolve().unwrap();

    assert!(resolution.end.is_none());
    let start = &resolution.start;
    assert_eq!(start.spine_index, 1); // /6/4 → step=4 → child_index=1
    assert_eq!(start.character_offset, Some(3));
    assert!(start.is_text_node); // /1 is odd
    assert_eq!(start.id_shortcut, Some("para05".into()));
}

#[test]
fn test_epubcfi_resolve_spine_index() {
    // spine_index 0 → /6/2, spine_index 2 → /6/6
    let cfi0 = EpubCfi::from_str("epubcfi(/6/2[ch1]!/4/2)").unwrap();
    let cfi2 = EpubCfi::from_str("epubcfi(/6/6[ch3]!/4/2)").unwrap();
    assert_eq!(cfi0.resolve().unwrap().start.spine_index, 0);
    assert_eq!(cfi2.resolve().unwrap().start.spine_index, 2);
}

#[test]
fn test_epubcfi_resolve_range() {
    // epubcfi(/6/4[chap01]!/4/2,/1:5,/3:10)
    // Parse structure:
    //   parent local_steps = [/4, /2]  (elements — the shared ancestor path)
    //   start  steps       = [/1]      (no '!', not local_steps; offset=5)
    //   end    relative    = [/3]      (no '!', not local_steps; offset=10)
    let cfi = EpubCfi::from_str("epubcfi(/6/4[chap01]!/4/2,/1:5,/3:10)").unwrap();
    let resolution = cfi.resolve().unwrap();

    // Range → both start and end present
    assert!(resolution.end.is_some());
    let start = &resolution.start;
    let end = resolution.end.as_ref().unwrap();

    // character_offset from the half-path (start.character_offset)
    assert_eq!(start.character_offset, Some(5));
    assert_eq!(end.character_offset, Some(10));

    // Last combined step is /2 (even → element, not text)
    assert!(!start.is_text_node);
    assert!(!end.is_text_node);

    // Both endpoints are in the same spine item
    assert_eq!(start.spine_index, end.spine_index);

    // Combined local steps: [/4(elem,idx=1), /2(elem,idx=0)]
    assert_eq!(start.steps.len(), 2);
    assert_eq!(start.steps[0].node_type, NodeType::Element);
    assert_eq!(start.steps[0].index, 1); // step=4 → children[1]
    assert_eq!(start.steps[1].node_type, NodeType::Element);
    assert_eq!(start.steps[1].index, 0); // step=2 → children[0]
}

#[test]
fn test_epubcfi_resolve_range_with_text_steps() {
    // A Range CFI where the shared parent local path ends at a text step.
    let path = CfiPath {
        steps: vec![
            CfiStep::new(6, None),
            CfiStep::new(4, Some("chap01".into())),
        ],
        local_steps: Some(vec![
            CfiStep::new(4, None), // element
            CfiStep::new(2, None), // element
            CfiStep::new(1, None), // TEXT ← last in parent local
        ]),
        character_offset: None,
        side: None,
        ..CfiPath::default()
    };
    // Simulate start half: local = [] + extra text step
    let start_half = CfiPath {
        steps: vec![],
        local_steps: Some(vec![CfiStep::new(3, None)]), // text
        character_offset: Some(7),
        side: None,
        ..CfiPath::default()
    };
    let cfi = EpubCfi::Range {
        parent: path.clone(),
        start: start_half.clone(),
        end: CfiPath {
            steps: vec![],
            local_steps: None,
            character_offset: Some(12),
            side: None,
            ..CfiPath::default()
        },
    };

    let resolution = cfi.resolve().unwrap();
    let start = &resolution.start;
    let end = resolution.end.as_ref().unwrap();

    // start combined: [/4,/2,/1(TEXT),/3(TEXT)] → last is text
    assert!(start.is_text_node);
    assert_eq!(start.character_offset, Some(7));

    // end combined: [/4,/2,/1(TEXT)] + [] → last is /1 (text)
    assert!(end.is_text_node);
    assert_eq!(end.character_offset, Some(12));
}

#[test]
fn test_epubcfi_resolve_no_local_path_returns_none() {
    // A base-only CFI without '!' should return None from resolve()
    let path = CfiPath {
        steps: vec![CfiStep::new(6, None), CfiStep::new(4, None)],
        local_steps: None,
        character_offset: None,
        side: None,
        ..CfiPath::default()
    };
    let cfi = EpubCfi::Point(path);
    assert!(cfi.resolve().is_none());
}

// ── Edge-case: assertion-comma must not split Range (P0) ─────────────────

#[test]
fn test_cfi_range_assertion_comma_not_split() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[chap,v2]!/4/2,/1:5,/3:10)").unwrap();
    assert!(
        matches!(cfi, EpubCfi::Range { .. }),
        "Expected Range, got: {cfi:?}"
    );
    if let EpubCfi::Range { parent, .. } = &cfi {
        assert_eq!(
            parent.steps[1].assertion.as_deref(),
            Some("chap,v2"),
            "Assertion must be preserved intact"
        );
    }
}

#[test]
fn test_cfi_range_multiple_commas_in_assertions() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[a,b,c]!/4/2,/1:0,/3:5)").unwrap();
    assert!(matches!(cfi, EpubCfi::Range { .. }));
    if let EpubCfi::Range { parent, .. } = &cfi {
        assert_eq!(parent.steps[1].assertion.as_deref(), Some("a,b,c"));
    }
}

#[test]
fn test_cfi_point_assertion_comma_preserved() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[foo,bar]!/4/2:3)").unwrap();
    if let EpubCfi::Point(path) = &cfi {
        assert_eq!(path.steps[1].assertion.as_deref(), Some("foo,bar"));
    }
}

// ── Edge-case: :before / :after side bias (P1) ───────────────────────────

#[test]
fn test_cfi_side_before_parsed() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2/1:5:before)").unwrap();
    if let EpubCfi::Point(path) = &cfi {
        assert_eq!(path.character_offset, Some(5));
        assert_eq!(path.side, Some(CfiSide::Before));
    } else {
        panic!("Expected Point CFI");
    }
}

#[test]
fn test_cfi_side_after_parsed() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2/1:3:after)").unwrap();
    if let EpubCfi::Point(path) = &cfi {
        assert_eq!(path.character_offset, Some(3));
        assert_eq!(path.side, Some(CfiSide::After));
    } else {
        panic!("Expected Point CFI");
    }
}

#[test]
fn test_cfi_no_side_is_none() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2/1:5)").unwrap();
    if let EpubCfi::Point(path) = &cfi {
        assert_eq!(path.character_offset, Some(5));
        assert_eq!(path.side, None);
    } else {
        panic!("Expected Point CFI");
    }
}

#[test]
fn test_cfi_side_round_trips_via_display() {
    let original = "epubcfi(/6/4[ch1]!/4/2/1:5:before)";
    let cfi = EpubCfi::from_str(original).unwrap();
    let rendered = cfi.to_string();
    assert_eq!(rendered, original);

    let reparsed = EpubCfi::from_str(&rendered).unwrap();
    if let EpubCfi::Point(path) = &reparsed {
        assert_eq!(path.side, Some(CfiSide::Before));
    }

    let original_after = "epubcfi(/6/4[ch1]!/4/2/1:3:after)";
    let cfi_after = EpubCfi::from_str(original_after).unwrap();
    assert_eq!(cfi_after.to_string(), original_after);
}

#[test]
fn test_cfi_side_propagated_to_resolved() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2/1:5:before)").unwrap();
    let resolution = cfi.resolve().unwrap();
    assert_eq!(resolution.start.side, Some(CfiSide::Before));
    assert_eq!(resolution.start.character_offset, Some(5));
}

// ── split_cfi_top_level unit tests ────────────────────────────────────────

#[test]
fn test_split_top_level_point() {
    let parts = split_cfi_top_level("/6/4[ch1]!/4/2:5");
    assert_eq!(parts, vec!["/6/4[ch1]!/4/2:5"]);
}

#[test]
fn test_split_top_level_range_no_assertion() {
    let parts = split_cfi_top_level("/6/4!/4/2,/1:5,/3:10");
    assert_eq!(parts, vec!["/6/4!/4/2", "/1:5", "/3:10"]);
}

// ── Temporal offset (~) ──────────────────────────────────────────────────

#[test]
fn test_temporal_offset_parsed() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2~23.5)").unwrap();
    if let EpubCfi::Point(path) = &cfi {
        assert_eq!(path.temporal_offset, Some(23.5));
        assert_eq!(path.spatial_offset, None);
        assert_eq!(path.character_offset, None);
    } else {
        panic!("Expected Point");
    }
}

#[test]
fn test_temporal_offset_integer() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2~5)").unwrap();
    if let EpubCfi::Point(p) = &cfi {
        assert_eq!(p.temporal_offset, Some(5.0));
    } else {
        panic!();
    }
}

#[test]
fn test_temporal_offset_sub_one() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2~0.5)").unwrap();
    if let EpubCfi::Point(p) = &cfi {
        assert_eq!(p.temporal_offset, Some(0.5));
    } else {
        panic!();
    }
}

// ── Spatial offset (@) ───────────────────────────────────────────────────

#[test]
fn test_spatial_offset_parsed() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2@50:75)").unwrap();
    if let EpubCfi::Point(p) = &cfi {
        let s = p.spatial_offset.as_ref().unwrap();
        assert_eq!(s.x, 50.0);
        assert_eq!(s.y, 75.0);
    } else {
        panic!();
    }
}

#[test]
fn test_spatial_offset_fractional() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2@5.75:97.6)").unwrap();
    if let EpubCfi::Point(p) = &cfi {
        let s = p.spatial_offset.as_ref().unwrap();
        assert!((s.x - 5.75).abs() < 1e-9);
        assert!((s.y - 97.6).abs() < 1e-9);
    } else {
        panic!();
    }
}

// ── Temporal + spatial ───────────────────────────────────────────────────

#[test]
fn test_temporal_spatial_combined() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2~23.5@5.75:97.6)").unwrap();
    if let EpubCfi::Point(p) = &cfi {
        assert_eq!(p.temporal_offset, Some(23.5));
        let s = p.spatial_offset.as_ref().unwrap();
        assert!((s.x - 5.75).abs() < 1e-9);
        assert!((s.y - 97.6).abs() < 1e-9);
    } else {
        panic!();
    }
}

// ── Display round-trip ───────────────────────────────────────────────────

#[test]
fn test_temporal_display_integer() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2~23)").unwrap();
    assert_eq!(cfi.to_string(), "epubcfi(/6/4[ch1]!/4/2~23)");
}

#[test]
fn test_temporal_display_fractional() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2~23.5)").unwrap();
    assert_eq!(cfi.to_string(), "epubcfi(/6/4[ch1]!/4/2~23.5)");
}

#[test]
fn test_spatial_display_round_trip() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2@5.75:97.6)").unwrap();
    assert_eq!(cfi.to_string(), "epubcfi(/6/4[ch1]!/4/2@5.75:97.6)");
}

#[test]
fn test_temporal_spatial_display_round_trip() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2~23.5@5.75:97.6)").unwrap();
    assert_eq!(cfi.to_string(), "epubcfi(/6/4[ch1]!/4/2~23.5@5.75:97.6)");
}

// ── Sorting rules (§3.2) ─────────────────────────────────────────────────

#[test]
fn test_sort_no_temporal_before_temporal() {
    let a = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2)").unwrap();
    let b = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2~1)").unwrap();
    assert!(a < b);
}

#[test]
fn test_sort_temporal_natural_order() {
    let a = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2~1.0)").unwrap();
    let b = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2~23.5)").unwrap();
    assert!(a < b);
}

#[test]
fn test_sort_temporal_dominates_spatial() {
    let a = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2~1.0@99:99)").unwrap();
    let b = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2~2.0@0:0)").unwrap();
    assert!(
        a < b,
        "lower temporal must be less even with higher spatial"
    );
}

#[test]
fn test_sort_y_before_x() {
    let a = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2@50:1)").unwrap();
    let b = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2@50:2)").unwrap();
    assert!(a < b);
}

#[test]
fn test_sort_no_spatial_before_spatial() {
    let a = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2~1.0)").unwrap();
    let b = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2~1.0@0:0)").unwrap();
    assert!(a < b);
}

// ── CfiResolved propagation ───────────────────────────────────────────────

#[test]
fn test_temporal_propagated_to_resolved() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2~23.5)").unwrap();
    let res = cfi.resolve().unwrap();
    assert_eq!(res.start.temporal_offset, Some(23.5));
}

#[test]
fn test_spatial_propagated_to_resolved() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2@50:75)").unwrap();
    let res = cfi.resolve().unwrap();
    let s = res.start.spatial_offset.as_ref().unwrap();
    assert_eq!(s.x, 50.0);
    assert_eq!(s.y, 75.0);
}

// ── Assertion comparison semantics (§3.2 Rule 2) ────────────────────────

#[test]
fn test_cfi_step_eq_ignores_assertion() {
    let a = CfiStep::new(4, Some("chap01".into()));
    let b = CfiStep::new(4, Some("old-chap01".into()));
    let c = CfiStep::new(4, None);
    assert_eq!(a, b);
    assert_eq!(a, c);
}

#[test]
fn test_cfi_step_neq_different_index() {
    let a = CfiStep::new(4, Some("same-id".into()));
    let b = CfiStep::new(6, Some("same-id".into()));
    assert_ne!(a, b);
}

#[test]
fn test_cfi_step_hash_consistent_with_eq() {
    use std::collections::HashSet;
    let mut set = HashSet::new();
    set.insert(CfiStep::new(4, Some("chap01".into())));
    set.insert(CfiStep::new(4, Some("old-chap01".into())));
    assert_eq!(set.len(), 1, "equal steps must hash identically");
}

#[test]
fn test_epub_cfi_eq_ignores_assertions() {
    let a = EpubCfi::from_str("epubcfi(/6/4[chap01]!/4/2:3)").unwrap();
    let b = EpubCfi::from_str("epubcfi(/6/4[old-chapter]!/4/2:3)").unwrap();
    assert_eq!(a, b);
}

#[test]
fn test_split_top_level_range_with_assertion_comma() {
    let parts = split_cfi_top_level("/6/4[a,b]!/4/2,/1:5,/3:10");
    assert_eq!(parts, vec!["/6/4[a,b]!/4/2", "/1:5", "/3:10"]);
}
