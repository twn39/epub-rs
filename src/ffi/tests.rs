use super::common::*;
use super::parser::*;
use super::generator::*;
use super::cfi::*;
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
