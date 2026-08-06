use std::ffi::CStr;
use std::os::raw::{c_char, c_uchar};
use std::ptr;

use crate::ffi::common::into_raw_bytes;
use crate::ffi_boundary;

/// Decrypt an obfuscated font file (IDPF or Adobe algorithm).
///
/// - `data` / `len`: encrypted font bytes.
/// - `epub_identifier`: the EPUB's unique identifier string (null-terminated).
/// - `is_idpf`: `1` for IDPF algorithm, `0` for Adobe.
/// - `out_len`: receives the number of decrypted bytes.
///
/// Returns a pointer to decrypted bytes. Free with `epub_free_bytes(ptr, *out_len)`.
/// Returns `NULL` on failure.
///
/// # Safety
/// - `data` must point to at least `len` readable bytes.
/// - `epub_identifier` and must be valid null-terminated C strings.
/// - `out_len` must be a valid non-null writable pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_decrypt_font(
    data: *const c_uchar,
    len: usize,
    epub_identifier: *const c_char,
    is_idpf: i32,
    out_len: *mut usize,
) -> *mut c_uchar {
    ffi_boundary!(ptr::null_mut(), {
        if data.is_null() || epub_identifier.is_null() || out_len.is_null() {
            return Err("epub_decrypt_font: null pointer argument".into());
        }
        use std::io::Read;
        let bytes = unsafe { std::slice::from_raw_parts(data, len) };
        let identifier = unsafe { CStr::from_ptr(epub_identifier) }.to_string_lossy();
        let algo = if is_idpf != 0 {
            crate::crypto::ObfuscationAlgorithm::Idpf
        } else {
            crate::crypto::ObfuscationAlgorithm::Adobe
        };
        let cursor = std::io::Cursor::new(bytes);
        let mut reader =
            crate::crypto::DeobfuscatingReader::new(Box::new(cursor), identifier.as_ref(), algo);
        let mut decrypted = Vec::with_capacity(len);
        reader.read_to_end(&mut decrypted)?;
        let out = decrypted.len();
        unsafe {
            *out_len = out;
        }
        Ok(into_raw_bytes(decrypted))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::common::epub_free_bytes;
    use std::ffi::CString;

    #[test]
    fn decrypt_font_idpf_roundtrip() {
        let font = [0u8; 64];
        let id = CString::new("urn:uuid:12345678-1234-1234-1234-123456789abc").unwrap();
        let mut out_len: usize = 0;
        let ptr =
            unsafe { epub_decrypt_font(font.as_ptr(), font.len(), id.as_ptr(), 1, &mut out_len) };
        assert!(!ptr.is_null());
        assert_eq!(out_len, 64);
        unsafe { epub_free_bytes(ptr, out_len) };
    }

    #[test]
    fn decrypt_font_adobe_roundtrip() {
        let font = [0u8; 64];
        let id = CString::new("urn:uuid:12345678-1234-1234-1234-123456789abc").unwrap();
        let mut out_len: usize = 0;
        let ptr =
            unsafe { epub_decrypt_font(font.as_ptr(), font.len(), id.as_ptr(), 0, &mut out_len) };
        assert!(!ptr.is_null());
        assert_eq!(out_len, 64);
        unsafe { epub_free_bytes(ptr, out_len) };
    }

    #[test]
    fn decrypt_font_null_data_returns_null() {
        let id = CString::new("urn:uuid:test").unwrap();
        let mut out_len: usize = 0;
        let ptr = unsafe { epub_decrypt_font(ptr::null(), 64, id.as_ptr(), 1, &mut out_len) };
        assert!(ptr.is_null());
    }

    #[test]
    fn decrypt_font_null_identifier_returns_null() {
        let font = [0u8; 64];
        let mut out_len: usize = 0;
        let ptr =
            unsafe { epub_decrypt_font(font.as_ptr(), font.len(), ptr::null(), 1, &mut out_len) };
        assert!(ptr.is_null());
    }

    #[test]
    fn decrypt_font_null_out_len_returns_null() {
        let font = [0u8; 64];
        let id = CString::new("urn:uuid:test").unwrap();
        let ptr = unsafe {
            epub_decrypt_font(font.as_ptr(), font.len(), id.as_ptr(), 1, ptr::null_mut())
        };
        assert!(ptr.is_null());
    }
}
