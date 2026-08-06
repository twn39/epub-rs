use crate::ffi::common::{epub_free_bytes, epub_last_error};
use crate::ffi::generator::{
    epub_generator_add_chapter, epub_generator_build, epub_generator_free, epub_generator_new,
    epub_generator_set_identifier, epub_generator_set_language, epub_generator_set_metadata,
    epub_generator_set_title, epub_generator_set_toc, epub_generator_validate,
};

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
fn test_generator_null_pointer_guards() {
    let handle = epub_generator_new();
    assert!(!handle.is_null());

    let mut out_len = 0;
    let null_build = unsafe { epub_generator_build(std::ptr::null_mut(), &mut out_len) };
    assert!(null_build.is_null());

    let null_build_out = unsafe { epub_generator_build(handle, std::ptr::null_mut()) };
    assert!(null_build_out.is_null());

    unsafe { epub_generator_free(handle) };
}

