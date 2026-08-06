use crate::ffi::common::epub_free_bytes;
use crate::ffi::crypto::epub_decrypt_font;
use std::ffi::CString;

#[test]
fn test_epub_decrypt_font_ffi_idpf() {
    let dummy_font_data = vec![0u8; 64];
    let identifier = CString::new("urn:uuid:12345678-1234-1234-1234-123456789abc").unwrap();
    let mut out_len: usize = 0;

    let res = unsafe {
        epub_decrypt_font(
            dummy_font_data.as_ptr(),
            dummy_font_data.len(),
            identifier.as_ptr(),
            1, // IDPF
            &mut out_len as *mut usize,
        )
    };

    assert!(!res.is_null());
    assert_eq!(out_len, 64);

    unsafe { epub_free_bytes(res, out_len) };
}

#[test]
fn test_epub_decrypt_font_ffi_adobe() {
    let dummy_font_data = vec![0u8; 64];
    let identifier = CString::new("urn:uuid:12345678-1234-1234-1234-123456789abc").unwrap();
    let mut out_len: usize = 0;

    let res = unsafe {
        epub_decrypt_font(
            dummy_font_data.as_ptr(),
            dummy_font_data.len(),
            identifier.as_ptr(),
            0, // Adobe
            &mut out_len as *mut usize,
        )
    };

    assert!(!res.is_null());
    assert_eq!(out_len, 64);

    unsafe { epub_free_bytes(res, out_len) };
}

#[test]
fn test_epub_decrypt_font_ffi_null_guards() {
    let dummy_font_data = vec![0u8; 64];
    let identifier = CString::new("urn:uuid:12345678").unwrap();
    let mut out_len: usize = 0;

    let null_data = unsafe {
        epub_decrypt_font(
            std::ptr::null(),
            dummy_font_data.len(),
            identifier.as_ptr(),
            1,
            &mut out_len as *mut usize,
        )
    };
    assert!(null_data.is_null());

    let null_ident = unsafe {
        epub_decrypt_font(
            dummy_font_data.as_ptr(),
            dummy_font_data.len(),
            std::ptr::null(),
            1,
            &mut out_len as *mut usize,
        )
    };
    assert!(null_ident.is_null());

    let null_out_len = unsafe {
        epub_decrypt_font(
            dummy_font_data.as_ptr(),
            dummy_font_data.len(),
            identifier.as_ptr(),
            1,
            std::ptr::null_mut(),
        )
    };
    assert!(null_out_len.is_null());
}
