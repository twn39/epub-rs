use crate::cfi::model::{CfiPath, CfiStep, CfiSide, EpubCfi, NodeType};
use std::str::FromStr;

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

#[test]
fn test_cfi_side_propagated_to_resolved() {
    let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2/1:5:before)").unwrap();
    let resolution = cfi.resolve().unwrap();
    assert_eq!(resolution.start.side, Some(CfiSide::Before));
    assert_eq!(resolution.start.character_offset, Some(5));
}

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
