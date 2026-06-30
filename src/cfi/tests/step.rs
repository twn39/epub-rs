use crate::cfi::model::{CfiStep, NodeType};

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
