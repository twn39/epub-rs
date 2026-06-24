use super::cfi::*;
use super::common::*;
use super::generator::*;
use super::parser::*;
use std::ptr;

/// Minimal valid EPUB 2 fixture as a ZIP for smoke-testing the C API.
/// Sourced from the existing generator tests.
fn make_epub_bytes() -> Vec<u8> {
    use crate::generator::EpubBuilder;
    use crate::model::Metadata;
    use std::io::Cursor;

    let metadata = Metadata {
        title: Some("FFI Test".to_string()),
        language: Some("en".to_string()),
        identifier: Some("urn:uuid:ffi-test".to_string()),
        ..Default::default()
    };
    let mut buf = Cursor::new(Vec::new());
    EpubBuilder::new()
        .metadata(metadata)
        .add_chapter(
            "ch1",
            "text/ch1.xhtml",
            b"<html><body><p>Hello FFI</p></body></html>".to_vec(),
        )
        .generate(&mut buf)
        .expect("test EPUB generation failed");
    buf.into_inner()
}

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
fn test_epub_resolve_cfi_stateless() {
    let cfi = std::ffi::CString::new("epubcfi(/6/4[ch1]!/4/2/1:0)").unwrap();
    let result = unsafe { epub_resolve_cfi(cfi.as_ptr()) };
    assert!(
        !result.is_null(),
        "epub_resolve_cfi returned NULL: {:?}",
        unsafe { std::ffi::CStr::from_ptr(epub_last_error()) }
    );

    let json = unsafe { std::ffi::CStr::from_ptr(result) }.to_string_lossy();
    assert!(json.contains("spine_index"), "unexpected CFI JSON: {json}");

    unsafe { epub_free_string(result) };
}

#[test]
fn test_free_string_null_is_safe() {
    // Must not crash
    unsafe { epub_free_string(ptr::null_mut()) };
}

#[test]
fn test_free_bytes_null_is_safe() {
    // Must not crash
    unsafe { epub_free_bytes(ptr::null_mut(), 0) };
}

// ── epub_generator_set_toc ────────────────────────────────────────────────

#[test]
fn test_generator_set_toc_valid_json() {
    let handle = epub_generator_new();
    assert!(!handle.is_null());

    // Set title + language so validate() passes
    let title = std::ffi::CString::new("Test").unwrap();
    let lang = std::ffi::CString::new("en").unwrap();
    unsafe { epub_generator_set_title(handle, title.as_ptr()) };
    unsafe { epub_generator_set_language(handle, lang.as_ptr()) };

    let toc_json =
        std::ffi::CString::new(r#"[{"title":"Ch1","href":"text/ch1.xhtml","children":[]}]"#)
            .unwrap();
    let ok = unsafe { epub_generator_set_toc(handle, toc_json.as_ptr()) };
    assert_eq!(ok, 1, "set_toc should succeed; error: {:?}", unsafe {
        std::ffi::CStr::from_ptr(epub_last_error())
    });

    unsafe { epub_generator_free(handle) };
}

#[test]
fn test_generator_set_toc_invalid_json_returns_0() {
    let handle = epub_generator_new();
    let bad_json = std::ffi::CString::new("not-json").unwrap();
    let ok = unsafe { epub_generator_set_toc(handle, bad_json.as_ptr()) };
    assert_eq!(ok, 0, "set_toc with bad JSON should return 0");
    let err = unsafe { std::ffi::CStr::from_ptr(epub_last_error()) };
    assert!(
        !err.to_string_lossy().is_empty(),
        "error message should be set"
    );
    unsafe { epub_generator_free(handle) };
}

// ── epub_generator_set_metadata ───────────────────────────────────────────

#[test]
fn test_generator_set_metadata_valid_json() {
    let handle = epub_generator_new();
    let json = std::ffi::CString::new(
        r#"{"title":"Meta Book","language":"zh","identifier":"urn:isbn:0"}"#,
    )
    .unwrap();
    let ok = unsafe { epub_generator_set_metadata(handle, json.as_ptr()) };
    assert_eq!(ok, 1, "set_metadata should succeed; error: {:?}", unsafe {
        std::ffi::CStr::from_ptr(epub_last_error())
    });
    unsafe { epub_generator_free(handle) };
}

#[test]
fn test_generator_set_metadata_invalid_json_returns_0() {
    let handle = epub_generator_new();
    let bad = std::ffi::CString::new("{invalid").unwrap();
    let ok = unsafe { epub_generator_set_metadata(handle, bad.as_ptr()) };
    assert_eq!(ok, 0);
    unsafe { epub_generator_free(handle) };
}

// ── epub_generator_validate ───────────────────────────────────────────────

#[test]
fn test_generator_validate_passes_when_ready() {
    let handle = epub_generator_new();
    assert!(!handle.is_null());

    let title = std::ffi::CString::new("Valid Book").unwrap();
    let lang = std::ffi::CString::new("en").unwrap();
    let id = std::ffi::CString::new("urn:uuid:test").unwrap();
    let href = std::ffi::CString::new("text/ch1.xhtml").unwrap();
    let ch_id = std::ffi::CString::new("ch1").unwrap();
    let body = std::ffi::CString::new("<html><body><p>Hello</p></body></html>").unwrap();

    unsafe { epub_generator_set_title(handle, title.as_ptr()) };
    unsafe { epub_generator_set_language(handle, lang.as_ptr()) };
    unsafe { epub_generator_set_identifier(handle, id.as_ptr()) };
    unsafe { epub_generator_add_chapter(handle, ch_id.as_ptr(), href.as_ptr(), body.as_ptr()) };

    let ok = unsafe { epub_generator_validate(handle) };
    assert_eq!(ok, 1, "validate should pass; error: {:?}", unsafe {
        std::ffi::CStr::from_ptr(epub_last_error())
    });

    // Handle should still be usable after validate
    let ok2 = unsafe { epub_generator_validate(handle) };
    assert_eq!(ok2, 1, "second validate call should also pass");

    unsafe { epub_generator_free(handle) };
}

#[test]
fn test_generator_validate_fails_when_empty() {
    let handle = epub_generator_new();
    let ok = unsafe { epub_generator_validate(handle) };
    assert_eq!(ok, 0, "empty generator should fail validation");
    let err = unsafe { std::ffi::CStr::from_ptr(epub_last_error()) };
    assert!(
        !err.to_string_lossy().is_empty(),
        "error message should be set"
    );
    unsafe { epub_generator_free(handle) };
}

// ── FFI Error and Panic Safety Tests ──────────────────────────────────────────

#[test]
fn test_catch_ffi_result_normal_error() {
    let default_val = 999;
    let res = catch_ffi_result(default_val, || -> Result<i32, FfiError> {
        Err(FfiError::Str("explicit error message"))
    });
    assert_eq!(res, default_val);
    let last_err_ptr = epub_last_error();
    assert!(!last_err_ptr.is_null());
    let last_err = unsafe { std::ffi::CStr::from_ptr(last_err_ptr) }.to_string_lossy();
    assert_eq!(last_err, "explicit error message");
}

#[test]
fn test_catch_ffi_result_panic_safety() {
    let default_val = 999;
    let res = catch_ffi_result(default_val, || -> Result<i32, FfiError> {
        panic!("something went wrong!");
    });
    assert_eq!(res, default_val);
    let last_err_ptr = epub_last_error();
    assert!(!last_err_ptr.is_null());
    let last_err = unsafe { std::ffi::CStr::from_ptr(last_err_ptr) }.to_string_lossy();
    assert!(
        last_err.contains("Panic: something went wrong!"),
        "expected panic message, got: {last_err}"
    );
}

#[test]
fn test_catch_ffi_void_normal_error() {
    catch_ffi_void(|| -> Result<(), FfiError> { Err(FfiError::Str("explicit void error")) });
    let last_err_ptr = epub_last_error();
    assert!(!last_err_ptr.is_null());
    let last_err = unsafe { std::ffi::CStr::from_ptr(last_err_ptr) }.to_string_lossy();
    assert_eq!(last_err, "explicit void error");
}

#[test]
fn test_catch_ffi_void_panic_safety() {
    catch_ffi_void(|| -> Result<(), FfiError> {
        panic!("void panic!");
    });
    let last_err_ptr = epub_last_error();
    assert!(!last_err_ptr.is_null());
    let last_err = unsafe { std::ffi::CStr::from_ptr(last_err_ptr) }.to_string_lossy();
    assert!(
        last_err.contains("Panic: void panic!"),
        "expected panic message, got: {last_err}"
    );
}

#[test]
fn test_generator_consumed_errors() {
    let handle = epub_generator_new();
    assert!(!handle.is_null());

    let title = std::ffi::CString::new("Valid Title").unwrap();
    let lang = std::ffi::CString::new("en").unwrap();
    let id = std::ffi::CString::new("urn:uuid:test").unwrap();
    let href = std::ffi::CString::new("text/ch1.xhtml").unwrap();
    let ch_id = std::ffi::CString::new("ch1").unwrap();
    let body = std::ffi::CString::new("<html><body><p>Hello</p></body></html>").unwrap();

    unsafe { epub_generator_set_title(handle, title.as_ptr()) };
    unsafe { epub_generator_set_language(handle, lang.as_ptr()) };
    unsafe { epub_generator_set_identifier(handle, id.as_ptr()) };
    unsafe { epub_generator_add_chapter(handle, ch_id.as_ptr(), href.as_ptr(), body.as_ptr()) };

    let mut out_len = 0;
    let bytes_ptr = unsafe { epub_generator_build(handle, &mut out_len) };
    assert!(!bytes_ptr.is_null());
    assert!(out_len > 0);

    // Now the generator has been consumed. Any subsequent call on `handle` should return an error gracefully.
    let ok = unsafe { epub_generator_validate(handle) };
    assert_eq!(ok, 0, "validate on consumed handle should return 0");
    let err_ptr = epub_last_error();
    assert!(!err_ptr.is_null());
    let err_str = unsafe { std::ffi::CStr::from_ptr(err_ptr) }.to_string_lossy();
    assert!(err_str.contains("generator already consumed"));

    // Set title on consumed handle should also report error instead of panicking
    unsafe { epub_generator_set_title(handle, title.as_ptr()) };
    let err_ptr2 = epub_last_error();
    assert!(!err_ptr2.is_null());
    let err_str2 = unsafe { std::ffi::CStr::from_ptr(err_ptr2) }.to_string_lossy();
    assert!(err_str2.contains("generator already consumed"));

    unsafe { epub_free_bytes(bytes_ptr, out_len) };
    unsafe { epub_generator_free(handle) };
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
