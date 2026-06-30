use crate::cfi::model::{CfiSide, EpubCfi};
use crate::cfi::parser::split_cfi_top_level;
use std::str::FromStr;

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
fn test_split_top_level_point() {
    let parts = split_cfi_top_level("/6/4[ch1]!/4/2:5");
    assert_eq!(parts, vec!["/6/4[ch1]!/4/2:5"]);
}

#[test]
fn test_split_top_level_range_no_assertion() {
    let parts = split_cfi_top_level("/6/4!/4/2,/1:5,/3:10");
    assert_eq!(parts, vec!["/6/4!/4/2", "/1:5", "/3:10"]);
}

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

#[test]
fn test_split_top_level_range_with_assertion_comma() {
    let parts = split_cfi_top_level("/6/4[a,b]!/4/2,/1:5,/3:10");
    assert_eq!(parts, vec!["/6/4[a,b]!/4/2", "/1:5", "/3:10"]);
}
