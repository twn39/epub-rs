use crate::ffi::common::{
    FfiError, catch_ffi_result, catch_ffi_void, epub_free_bytes, epub_free_string, epub_last_error,
};
use std::ptr;

pub(crate) fn make_epub_bytes() -> Vec<u8> {
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
fn test_free_string_null_is_safe() {
    // Must not crash
    unsafe { epub_free_string(ptr::null_mut()) };
}

#[test]
fn test_free_bytes_null_is_safe() {
    // Must not crash
    unsafe { epub_free_bytes(ptr::null_mut(), 0) };
}

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
