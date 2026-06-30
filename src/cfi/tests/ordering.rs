use crate::cfi::model::{CfiPath, CfiStep, EpubCfi};
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

#[test]
fn test_epub_cfi_eq_ignores_assertions() {
    let a = EpubCfi::from_str("epubcfi(/6/4[chap01]!/4/2:3)").unwrap();
    let b = EpubCfi::from_str("epubcfi(/6/4[old-chapter]!/4/2:3)").unwrap();
    assert_eq!(a, b);
}
