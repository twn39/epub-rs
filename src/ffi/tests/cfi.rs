use crate::ffi::cfi::epub_resolve_cfi;
use crate::ffi::common::{epub_free_string, epub_last_error};

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
