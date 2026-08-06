use super::common::make_epub_bytes;
use crate::ffi::common::{epub_free, epub_free_string, epub_last_error};
use crate::ffi::parser::{
    epub_cfi_from_location_fast, epub_generate_location_index, epub_get_position_info,
    epub_get_toc, epub_location_from_cfi_fast, epub_open, epub_parse, epub_preferred_reading_start,
    epub_prepare_chapter, epub_prepare_chapter_with_stats, epub_search_book,
    epub_search_book_with_options,
};
use std::ffi::CString;
use std::ptr;

#[test]
fn test_epub_open_and_parse() {
    let bytes = make_epub_bytes();
    let handle = unsafe { epub_open(bytes.as_ptr(), bytes.len()) };
    assert!(!handle.is_null(), "epub_open returned NULL");

    let json_ptr = unsafe { epub_parse(handle) };
    assert!(
        !json_ptr.is_null(),
        "epub_parse returned NULL: {:?}",
        unsafe { std::ffi::CStr::from_ptr(epub_last_error()) }
    );

    let json = unsafe { std::ffi::CStr::from_ptr(json_ptr) }
        .to_string_lossy()
        .into_owned();
    assert!(json.contains("FFI Test"), "title not found in JSON: {json}");

    unsafe { epub_free_string(json_ptr) };
    unsafe { epub_free(handle) };
}

#[test]
fn test_epub_get_toc() {
    let bytes = make_epub_bytes();
    let handle = unsafe { epub_open(bytes.as_ptr(), bytes.len()) };
    assert!(!handle.is_null());

    let toc_ptr = unsafe { epub_get_toc(handle) };
    assert!(!toc_ptr.is_null());

    let toc = unsafe { std::ffi::CStr::from_ptr(toc_ptr) }.to_string_lossy();
    // Should be a JSON array
    assert!(toc.starts_with('['), "TOC is not a JSON array: {toc}");

    unsafe { epub_free_string(toc_ptr) };
    unsafe { epub_free(handle) };
}

#[test]
fn test_epub_open_null_returns_null() {
    let handle = unsafe { epub_open(ptr::null(), 0) };
    assert!(handle.is_null());
    let err = epub_last_error();
    assert!(
        !err.is_null(),
        "epub_last_error should be set after failure"
    );
}

#[test]
fn test_ffi_fast_path_positions_and_caching() {
    let bytes = make_epub_bytes();
    let handle = unsafe { epub_open(bytes.as_ptr(), bytes.len()) };
    assert!(!handle.is_null());

    // 1. Verify initially the index is None
    unsafe {
        let h = handle.as_mut().unwrap();
        assert!(h.position_index.is_none());
    }

    // 2. Generate location index (should build and cache the index)
    let json_ptr = unsafe { epub_generate_location_index(handle, 0) };
    assert!(!json_ptr.is_null());
    unsafe { epub_free_string(json_ptr) };

    // 3. Verify the index is now Cached
    unsafe {
        let h = handle.as_mut().unwrap();
        assert!(h.position_index.is_some());
    }

    // 4. Test epub_location_from_cfi_fast
    // The book has one chapter, so its position CFI should map to index 0.
    // Let's resolve the CFI for the first position to index and vice versa.
    let cfi_str = std::ffi::CString::new("epubcfi(/6/2!/4/2/1:0)").unwrap();
    let idx = unsafe { epub_location_from_cfi_fast(handle, cfi_str.as_ptr()) };
    // Let's assert idx is valid (since the book has at least one position, it should resolve).
    // Note: Since this is a simple generated book, let's make sure it doesn't return -1.
    // If the CFI is valid for spine index 0, it will map to a valid index.
    assert!(idx >= 0, "fast location cfi resolution failed: {idx}");

    // 5. Test epub_cfi_from_location_fast
    let cfi_ptr = unsafe { epub_cfi_from_location_fast(handle, idx as usize) };
    assert!(!cfi_ptr.is_null());
    let cfi_resolved = unsafe { std::ffi::CStr::from_ptr(cfi_ptr) }.to_string_lossy();
    assert!(
        cfi_resolved.starts_with("epubcfi("),
        "unexpected CFI: {cfi_resolved}"
    );
    unsafe { epub_free_string(cfi_ptr) };

    // 6. Test epub_get_position_info
    let mut spine_index = 0;
    let mut chap_prog = 0.0f32;
    let mut total_prog = 0.0f32;
    let ok = unsafe {
        epub_get_position_info(
            handle,
            idx as usize,
            &mut spine_index,
            &mut chap_prog,
            &mut total_prog,
        )
    };
    assert_eq!(ok, 1, "failed to get position info");
    assert_eq!(spine_index, 0); // first spine item
    assert!((0.0..=1.0).contains(&chap_prog));
    assert!((0.0..=1.0).contains(&total_prog));

    // Test out of range index
    let ok_oob = unsafe {
        epub_get_position_info(
            handle,
            idx as usize + 100,
            &mut spine_index,
            &mut chap_prog,
            &mut total_prog,
        )
    };
    assert_eq!(ok_oob, 0, "OOB index should return 0");

    unsafe { epub_free(handle) };
}

#[test]
fn test_ffi_prepare_search_reading_start() {
    let bytes = make_epub_bytes();
    let handle = unsafe { epub_open(bytes.as_ptr(), bytes.len()) };
    assert!(!handle.is_null());

    let start_ptr = unsafe { epub_preferred_reading_start(handle) };
    assert!(!start_ptr.is_null(), "preferred_reading_start null");
    let start_json = unsafe { std::ffi::CStr::from_ptr(start_ptr) }.to_string_lossy();
    assert!(
        start_json.contains("spine_index"),
        "unexpected: {start_json}"
    );
    unsafe { epub_free_string(start_ptr) };

    let id = CString::new("ch1").unwrap();
    let prep = unsafe { epub_prepare_chapter(handle, id.as_ptr(), ptr::null()) };
    assert!(!prep.is_null(), "prepare_chapter null: {:?}", unsafe {
        std::ffi::CStr::from_ptr(epub_last_error())
    });
    let html = unsafe { std::ffi::CStr::from_ptr(prep) }.to_string_lossy();
    assert!(html.contains("Hello") || html.contains("html"), "{html}");
    unsafe { epub_free_string(prep) };

    let prep_stats = unsafe { epub_prepare_chapter_with_stats(handle, id.as_ptr(), ptr::null()) };
    assert!(
        !prep_stats.is_null(),
        "prepare_chapter_with_stats null: {:?}",
        unsafe { std::ffi::CStr::from_ptr(epub_last_error()) }
    );
    let stats_json = unsafe { std::ffi::CStr::from_ptr(prep_stats) }.to_string_lossy();
    assert!(
        stats_json.contains("\"html\"") && stats_json.contains("\"stats\""),
        "{stats_json}"
    );
    unsafe { epub_free_string(prep_stats) };

    let q = CString::new("Hello").unwrap();
    let hits = unsafe { epub_search_book(handle, q.as_ptr(), 5, 20) };
    assert!(!hits.is_null());
    let hits_json = unsafe { std::ffi::CStr::from_ptr(hits) }.to_string_lossy();
    assert!(hits_json.starts_with('['), "{hits_json}");
    unsafe { epub_free_string(hits) };

    let opts = CString::new(r#"{"case_insensitive":true,"max_total":10}"#).unwrap();
    let hits2 = unsafe { epub_search_book_with_options(handle, q.as_ptr(), opts.as_ptr()) };
    assert!(!hits2.is_null());
    unsafe { epub_free_string(hits2) };

    unsafe { epub_free(handle) };
}

#[test]
fn test_ffi_renditions_and_navigation() {
    use crate::ffi::parser::{
        epub_get_navigation, epub_get_page_list, epub_get_renditions, epub_parse_best_for,
        epub_parse_by_index,
    };

    let bytes = make_epub_bytes();
    let handle = unsafe { epub_open(bytes.as_ptr(), bytes.len()) };
    assert!(!handle.is_null());

    let renditions_ptr = unsafe { epub_get_renditions(handle) };
    assert!(!renditions_ptr.is_null());
    let renditions_json = unsafe { std::ffi::CStr::from_ptr(renditions_ptr) }.to_string_lossy();
    assert!(renditions_json.contains("opf"), "{renditions_json}");
    unsafe { epub_free_string(renditions_ptr) };

    let parse_idx_ptr = unsafe { epub_parse_by_index(handle, 0) };
    assert!(!parse_idx_ptr.is_null());
    unsafe { epub_free_string(parse_idx_ptr) };

    let layout = CString::new("reflowable").unwrap();
    let lang = CString::new("en").unwrap();
    let parse_best_ptr = unsafe { epub_parse_best_for(handle, layout.as_ptr(), lang.as_ptr()) };
    assert!(!parse_best_ptr.is_null());
    unsafe { epub_free_string(parse_best_ptr) };

    let nav_ptr = unsafe { epub_get_navigation(handle) };
    assert!(!nav_ptr.is_null());
    let nav_json = unsafe { std::ffi::CStr::from_ptr(nav_ptr) }.to_string_lossy();
    assert!(nav_json.contains("toc"), "{nav_json}");
    unsafe { epub_free_string(nav_ptr) };

    let plist_ptr = unsafe { epub_get_page_list(handle) };
    assert!(!plist_ptr.is_null());
    unsafe { epub_free_string(plist_ptr) };

    unsafe { epub_free(handle) };
}

#[test]
fn test_ffi_resources_cfi_and_semantic_content() {
    use crate::ffi::common::epub_free_bytes;
    use crate::ffi::parser::{
        epub_cfi_from_location, epub_get_chapter_with_cfi, epub_get_resource_by_id,
        epub_get_semantic_content, epub_has_media_overlays, epub_location_from_cfi,
        epub_positions_by_reading_order, epub_search_chapter,
    };

    let bytes = make_epub_bytes();
    let handle = unsafe { epub_open(bytes.as_ptr(), bytes.len()) };
    assert!(!handle.is_null());

    let ch_id = CString::new("ch1").unwrap();

    let mut out_len: usize = 0;
    let res_ptr = unsafe { epub_get_resource_by_id(handle, ch_id.as_ptr(), &mut out_len) };
    assert!(!res_ptr.is_null());
    assert!(out_len > 0);
    unsafe { epub_free_bytes(res_ptr, out_len) };

    let cfi_html = unsafe { epub_get_chapter_with_cfi(handle, ch_id.as_ptr()) };
    assert!(!cfi_html.is_null());
    let html_str = unsafe { std::ffi::CStr::from_ptr(cfi_html) }.to_string_lossy();
    assert!(html_str.contains("Hello"), "{html_str}");
    unsafe { epub_free_string(cfi_html) };

    let semantic = unsafe { epub_get_semantic_content(handle, ch_id.as_ptr()) };
    assert!(!semantic.is_null());
    let semantic_json = unsafe { std::ffi::CStr::from_ptr(semantic) }.to_string_lossy();
    assert!(semantic_json.starts_with('['), "{semantic_json}");
    unsafe { epub_free_string(semantic) };

    let q = CString::new("Hello").unwrap();
    let search_res = unsafe { epub_search_chapter(handle, ch_id.as_ptr(), q.as_ptr()) };
    assert!(!search_res.is_null());
    unsafe { epub_free_string(search_res) };

    let pos_ptr = unsafe { epub_positions_by_reading_order(handle, 1024) };
    assert!(!pos_ptr.is_null());

    let cfi = CString::new("epubcfi(/6/2!/4/2/1:0)").unwrap();
    let pos_json = unsafe { epub_location_from_cfi(handle, pos_ptr, cfi.as_ptr()) };
    assert!(!pos_json.is_null());
    unsafe { epub_free_string(pos_json) };

    let reverse_cfi = unsafe { epub_cfi_from_location(handle, pos_ptr, 0) };
    assert!(!reverse_cfi.is_null());
    unsafe { epub_free_string(reverse_cfi) };

    unsafe { epub_free_string(pos_ptr) };

    let has_mo = unsafe { epub_has_media_overlays(handle) };
    assert_eq!(has_mo, 0);

    unsafe { epub_free(handle) };
}
