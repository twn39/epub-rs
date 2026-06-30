use super::common::make_epub_bytes;
use crate::ffi::common::{epub_free, epub_free_string, epub_last_error};
use crate::ffi::parser::{
    epub_cfi_from_location_fast, epub_generate_location_index, epub_get_position_info,
    epub_get_toc, epub_location_from_cfi_fast, epub_open, epub_parse,
};
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
