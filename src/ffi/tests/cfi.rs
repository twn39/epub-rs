use crate::ffi::cfi::{epub_compare_cfi, epub_generate_cfi_range, epub_resolve_cfi};
use crate::ffi::common::{epub_free_string, epub_last_error};
use std::ffi::CString;

#[test]
fn test_epub_resolve_cfi_stateless() {
    let cfi = CString::new("epubcfi(/6/4[ch1]!/4/2/1:0)").unwrap();
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
fn test_epub_resolve_cfi_null_pointer() {
    let result = unsafe { epub_resolve_cfi(std::ptr::null()) };
    assert!(result.is_null());
    let err = unsafe { std::ffi::CStr::from_ptr(epub_last_error()) }.to_string_lossy();
    assert!(
        err.contains("null"),
        "expected null error message, got: {err}"
    );
}

#[test]
fn test_epub_compare_cfi_ffi() {
    let cfi_a = CString::new("epubcfi(/6/4!/4/2:10)").unwrap();
    let cfi_b = CString::new("epubcfi(/6/4!/4/2:20)").unwrap();
    let cfi_c = CString::new("epubcfi(/6/4!/4/2:10)").unwrap();

    let cmp_less = unsafe { epub_compare_cfi(cfi_a.as_ptr(), cfi_b.as_ptr()) };
    assert_eq!(cmp_less, -1);

    let cmp_equal = unsafe { epub_compare_cfi(cfi_a.as_ptr(), cfi_c.as_ptr()) };
    assert_eq!(cmp_equal, 0);

    let cmp_greater = unsafe { epub_compare_cfi(cfi_b.as_ptr(), cfi_a.as_ptr()) };
    assert_eq!(cmp_greater, 1);

    let null_cmp = unsafe { epub_compare_cfi(std::ptr::null(), cfi_b.as_ptr()) };
    assert_eq!(null_cmp, i32::MIN);
}

#[test]
fn test_epub_generate_cfi_range_ffi() {
    let start = CString::new("epubcfi(/6/4!/4/2:10)").unwrap();
    let end = CString::new("epubcfi(/6/4!/4/2:50)").unwrap();

    let res = unsafe { epub_generate_cfi_range(start.as_ptr(), end.as_ptr()) };
    assert!(!res.is_null());

    let range_str = unsafe { std::ffi::CStr::from_ptr(res) }.to_string_lossy();
    assert!(
        range_str.contains("epubcfi(/6/4!/4/2,:10,:50)"),
        "unexpected range: {range_str}"
    );

    unsafe { epub_free_string(res) };

    let null_res = unsafe { epub_generate_cfi_range(std::ptr::null(), end.as_ptr()) };
    assert!(null_res.is_null());
}
