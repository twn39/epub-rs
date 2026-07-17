use std::ffi::CStr;
use std::os::raw::c_char;
use std::ptr;

use crate::ffi::common::{into_c_string, to_json};
use crate::ffi_boundary;

/// Resolve a CFI string to a structured DOM location descriptor.
///
/// This is a **stateless** function — it does not require a loaded EPUB handle.
///
/// JSON shape:
/// ```json
/// {
///   "start": {
///     "spine_index": 1,
///     "steps": [{"node_type": "element", "index": 2, "id": "section1"}, ...],
///     "xpath": "id('section1')/...",
///     "xpath_ns_agnostic": "local-name() path (namespace-agnostic)",
///     "id_shortcut": "para5",
///     "character_offset": 3,
///     "is_text_node": true
///   },
///   "end": null
/// }
/// ```
///
/// The caller must free the returned string with `epub_free_string()`.
/// Returns `NULL` on failure — call `epub_last_error()` for details.
///
/// # Safety
/// `cfi_str` must be a valid null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_resolve_cfi(cfi_str: *const c_char) -> *mut c_char {
    ffi_boundary!(ptr::null_mut(), {
        if cfi_str.is_null() {
            return Err("epub_resolve_cfi: cfi_str pointer is null".into());
        }

        // SAFETY: cfi_str is a valid null-terminated C string (caller contract).
        let s = unsafe { CStr::from_ptr(cfi_str) }.to_string_lossy();

        use std::str::FromStr;
        let cfi = crate::cfi::EpubCfi::from_str(s.as_ref())?;

        match cfi.resolve() {
            Some(resolution) => Ok(to_json(&resolution)),
            None => Err("CFI has no local path (missing '!' separator)".into()),
        }
    })
}

/// Compare two CFI strings numerically per the EPUB CFI spec.
///
/// Returns: `-1` if `cfi_a < cfi_b`, `0` if equal, `1` if `cfi_a > cfi_b`.
/// On parse error returns `INT32_MIN` and sets `epub_last_error()`.
///
/// # Safety
/// Both `cfi_a` and `cfi_b` must be valid null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_compare_cfi(cfi_a: *const c_char, cfi_b: *const c_char) -> i32 {
    ffi_boundary!(i32::MIN, {
        if cfi_a.is_null() || cfi_b.is_null() {
            return Err("epub_compare_cfi: null pointer".into());
        }
        use std::str::FromStr;
        let sa = unsafe { CStr::from_ptr(cfi_a) }.to_string_lossy();
        let sb = unsafe { CStr::from_ptr(cfi_b) }.to_string_lossy();
        let a = crate::cfi::EpubCfi::from_str(sa.as_ref())?;
        let b = crate::cfi::EpubCfi::from_str(sb.as_ref())?;
        match a.cmp(&b) {
            std::cmp::Ordering::Less => Ok(-1),
            std::cmp::Ordering::Equal => Ok(0),
            std::cmp::Ordering::Greater => Ok(1),
        }
    })
}

/// Combine two Point CFIs into a spec-compliant range CFI string.
///
/// Output format: `epubcfi(shared,start_local,end_local)`.
/// The caller must free the returned string with `epub_free_string()`.
/// Returns `NULL` on failure.
///
/// # Safety
/// Both `start_cfi` and `end_cfi` must be valid null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generate_cfi_range(
    start_cfi: *const c_char,
    end_cfi: *const c_char,
) -> *mut c_char {
    ffi_boundary!(ptr::null_mut(), {
        if start_cfi.is_null() || end_cfi.is_null() {
            return Err("epub_generate_cfi_range: null pointer".into());
        }
        use std::str::FromStr;
        let ss = unsafe { CStr::from_ptr(start_cfi) }.to_string_lossy();
        let se = unsafe { CStr::from_ptr(end_cfi) }.to_string_lossy();
        let start = crate::cfi::EpubCfi::from_str(ss.as_ref())?;
        let end = crate::cfi::EpubCfi::from_str(se.as_ref())?;
        let range = crate::cfi::EpubCfi::generate_range(&start, &end)?;
        Ok(into_c_string(range))
    })
}
