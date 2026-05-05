//! C FFI layer for epub-rs.
//!
//! Exposes an opaque-handle, JSON-centric C API so that any language with a
//! C FFI bridge (Swift, Python/ctypes, Go/cgo, Kotlin/JNI, etc.) can use the
//! library without Rust toolchain knowledge.
//!
//! ## Design principles
//! - **Opaque handle**: `EpubHandle` is never `#[repr(C)]`; C code holds only
//!   a `*mut EpubHandle` pointer. Internal layout is invisible.
//! - **JSON for complex data**: All structured return values (EpubBook, TOC,
//!   positions) are serialised to a JSON `char *`. This is the single biggest
//!   simplification for cross-language use — every caller can parse JSON.
//! - **Explicit ownership**: every heap allocation has exactly one paired free
//!   function. The rules are documented in `epub_rs.h`.
//! - **Thread-local errors**: any failure stores a description in a thread-local
//!   variable, retrievable via `epub_last_error()`. This lets stateless helpers
//!   (e.g. `epub_resolve_cfi`) report errors without a handle parameter.
//! - **No panics at the boundary**: `[profile.release] panic = "abort"` in
//!   `Cargo.toml` prevents any Rust panic from crossing the FFI boundary as UB.

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::io::Cursor;
use std::os::raw::{c_char, c_uchar};
use std::ptr;

use crate::model::EpubBook;
use crate::parser::EpubArchive;
use crate::provider::ZipProvider;

// ── Thread-local error storage ────────────────────────────────────────────────
//
// Using thread-local storage (rather than a field on EpubHandle) means that
// stateless helpers such as `epub_resolve_cfi` can also report errors.

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = RefCell::new(None);
}

fn set_error(msg: impl AsRef<str>) {
    LAST_ERROR.with(|cell| {
        *cell.borrow_mut() = CString::new(msg.as_ref()).ok();
    });
}

fn clear_error() {
    LAST_ERROR.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

// ── Opaque handle ─────────────────────────────────────────────────────────────

/// Opaque EPUB archive handle.
///
/// Allocated by `epub_open` / `epub_open_file`, freed by `epub_free`.
/// C code must never attempt to inspect or copy the pointed-to memory.
pub struct EpubHandle {
    archive: EpubArchive<ZipProvider<Cursor<Vec<u8>>>>,
    /// Lazily parsed on the first call to any API that needs book metadata.
    book: Option<EpubBook>,
}

impl EpubHandle {
    /// Ensure the OPF has been parsed. Subsequent calls are free (cached).
    fn ensure_parsed(&mut self) -> Result<(), String> {
        if self.book.is_none() {
            let book = self.archive.parse().map_err(|e| e.to_string())?;
            self.book = Some(book);
        }
        Ok(())
    }

    /// Panics if `ensure_parsed()` has not been called successfully yet.
    fn book(&self) -> &EpubBook {
        self.book.as_ref().expect("book must be parsed before access")
    }
}

// ── Allocation helpers ────────────────────────────────────────────────────────

/// Move a Rust `String` into a heap-allocated, null-terminated `char *`.
///
/// The returned pointer must eventually be passed to `epub_free_string()`.
/// Returns `NULL` and sets the thread-local error if the string contains an
/// interior null byte (which is theoretically impossible for valid JSON/UTF-8).
fn into_c_string(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(cs) => cs.into_raw(),
        Err(e) => {
            set_error(format!("Internal error: failed to create C string: {e}"));
            ptr::null_mut()
        }
    }
}

/// Serialise `value` to JSON and return it as a heap-allocated `char *`.
fn to_json<T: serde::Serialize>(value: &T) -> *mut c_char {
    match serde_json::to_string(value) {
        Ok(json) => into_c_string(json),
        Err(e) => {
            set_error(format!("JSON serialisation error: {e}"));
            ptr::null_mut()
        }
    }
}

/// Convert a `Vec<u8>` into a raw `*mut u8` (caller receives ownership).
///
/// The returned pointer must be freed with `epub_free_bytes(ptr, len)`.
fn into_raw_bytes(bytes: Vec<u8>) -> *mut c_uchar {
    let mut boxed: Box<[u8]> = bytes.into_boxed_slice();
    let ptr = boxed.as_mut_ptr();
    // Leak the box — ownership is transferred to the C caller.
    std::mem::forget(boxed);
    ptr
}

// ── Lifecycle ─────────────────────────────────────────────────────────────────

/// Open an EPUB from a byte buffer.
///
/// `data` must point to at least `len` bytes of a valid ZIP/EPUB file.
/// Returns an opaque handle on success, `NULL` on failure.
/// The caller is responsible for freeing the handle with `epub_free()`.
///
/// # Safety
/// `data` must be a valid pointer to `len` contiguous, readable bytes.
/// The pointer does not need to remain valid after this function returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_open(data: *const c_uchar, len: usize) -> *mut EpubHandle {
    clear_error();

    if data.is_null() {
        set_error("epub_open: data pointer is null");
        return ptr::null_mut();
    }
    if len == 0 {
        set_error("epub_open: len is 0");
        return ptr::null_mut();
    }

    // SAFETY: Caller guarantees data points to `len` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    let cursor = Cursor::new(bytes);

    match EpubArchive::new(cursor) {
        Ok(archive) => Box::into_raw(Box::new(EpubHandle { archive, book: None })),
        Err(e) => {
            set_error(e.to_string());
            ptr::null_mut()
        }
    }
}

/// Open an EPUB from a file-system path (UTF-8 encoded, null-terminated).
///
/// Returns an opaque handle on success, `NULL` on failure.
/// The caller is responsible for freeing the handle with `epub_free()`.
///
/// # Safety
/// `path` must be a valid null-terminated C string.
#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub unsafe extern "C" fn epub_open_file(path: *const c_char) -> *mut EpubHandle {
    clear_error();

    if path.is_null() {
        set_error("epub_open_file: path pointer is null");
        return ptr::null_mut();
    }

    // SAFETY: Caller guarantees path is a valid null-terminated C string.
    let path_str = unsafe { CStr::from_ptr(path) }.to_string_lossy();

    let bytes = match std::fs::read(path_str.as_ref()) {
        Ok(b) => b,
        Err(e) => {
            set_error(format!("epub_open_file: failed to read '{path_str}': {e}"));
            return ptr::null_mut();
        }
    };

    let cursor = Cursor::new(bytes);
    match EpubArchive::new(cursor) {
        Ok(archive) => Box::into_raw(Box::new(EpubHandle { archive, book: None })),
        Err(e) => {
            set_error(e.to_string());
            ptr::null_mut()
        }
    }
}

/// Free an EPUB handle.
///
/// Safe to call with `NULL` (no-op). After this call the pointer is invalid.
///
/// # Safety
/// `handle` must either be `NULL` or a pointer previously returned by an
/// `epub_open*` function that has not yet been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_free(handle: *mut EpubHandle) {
    if !handle.is_null() {
        // SAFETY: Caller guarantees this pointer came from Box::into_raw.
        unsafe { drop(Box::from_raw(handle)); }
    }
}

// ── Parsing & metadata ────────────────────────────────────────────────────────

/// Parse the EPUB and return book metadata as a JSON string.
///
/// After the first call the result is cached; subsequent calls are cheap.
/// The caller must free the returned string with `epub_free_string()`.
/// Returns `NULL` on failure — call `epub_last_error()` for details.
///
/// JSON shape: `EpubBook` — see `src/model.rs` for field documentation.
///
/// # Safety
/// `handle` must be a valid non-null pointer obtained from `epub_open*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_parse(handle: *mut EpubHandle) -> *mut c_char {
    clear_error();

    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => { set_error("epub_parse: null handle"); return ptr::null_mut(); }
    };

    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }

    to_json(h.book())
}

// ── Navigation ────────────────────────────────────────────────────────────────

/// Return all navigation data (TOC + page-list + landmarks) as a JSON string.
///
/// The EPUB navigation file is read and parsed only once; results are returned
/// directly without an internal cache (navigation is rarely called multiple times).
///
/// JSON shape:
/// ```json
/// {
///   "toc":       [{"title":"Ch1","href":"ch1.xhtml","children":[...]}, ...],
///   "page_list": [{"title":"1","href":"ch1.xhtml#p1","children":[]}, ...],
///   "landmarks": [{"title":"Begin Reading","href":"ch1.xhtml","children":[]}, ...]
/// }
/// ```
///
/// The caller must free the returned string with `epub_free_string()`.
/// Returns `NULL` on failure.
///
/// # Safety
/// `handle` must be a valid non-null pointer obtained from `epub_open*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_get_navigation(handle: *mut EpubHandle) -> *mut c_char {
    clear_error();

    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => { set_error("epub_get_navigation: null handle"); return ptr::null_mut(); }
    };

    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }

    // Clone the cached book to avoid simultaneous &mut archive + &book on the same struct.
    let book = h.book.clone().unwrap();
    match h.archive.get_navigation(&book) {
        Ok(nav) => to_json(&nav),
        Err(e) => { set_error(e.to_string()); ptr::null_mut() }
    }
}

/// Return the Table of Contents as a JSON array of `TocEntry` objects.
///
/// Convenience wrapper around `epub_get_navigation`. Prefer
/// `epub_get_navigation` when you also need page-list or landmarks,
/// to avoid re-reading the navigation file.
///
/// The caller must free the returned string with `epub_free_string()`.
///
/// # Safety
/// `handle` must be a valid non-null pointer obtained from `epub_open*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_get_toc(handle: *mut EpubHandle) -> *mut c_char {
    clear_error();

    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => { set_error("epub_get_toc: null handle"); return ptr::null_mut(); }
    };

    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }

    let book = h.book.clone().unwrap();
    match h.archive.get_toc(&book) {
        Ok(toc) => to_json(&toc),
        Err(e) => { set_error(e.to_string()); ptr::null_mut() }
    }
}

/// Return the page list as a JSON array of `TocEntry` objects.
///
/// Each entry has `title` = print page label (`"1"`, `"42"`, `"xii"`) and
/// `href` = document position (typically with a fragment: `"ch3.xhtml#p42"`).
///
/// Returns a JSON empty array `[]` if no page list is present.
/// The caller must free the returned string with `epub_free_string()`.
///
/// # Safety
/// `handle` must be a valid non-null pointer obtained from `epub_open*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_get_page_list(handle: *mut EpubHandle) -> *mut c_char {
    clear_error();

    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => { set_error("epub_get_page_list: null handle"); return ptr::null_mut(); }
    };

    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }

    let book = h.book.clone().unwrap();
    match h.archive.get_page_list(&book) {
        Ok(page_list) => to_json(&page_list),
        Err(e) => { set_error(e.to_string()); ptr::null_mut() }
    }
}

// ── Positions ─────────────────────────────────────────────────────────────────

/// Return reading positions grouped by spine item as a JSON 2-D array.
///
/// `bytes_per_position`: granularity of reflowable positions in bytes.
/// Pass `0` to use the Readium/Adobe default of 1024 bytes.
///
/// JSON shape: `[[{Position}, ...], ...]`  — outer index = spine item index.
/// The caller must free the returned string with `epub_free_string()`.
///
/// # Safety
/// `handle` must be a valid non-null pointer obtained from `epub_open*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_positions_by_reading_order(
    handle: *mut EpubHandle,
    bytes_per_position: usize,
) -> *mut c_char {
    clear_error();

    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => { set_error("epub_positions_by_reading_order: null handle"); return ptr::null_mut(); }
    };

    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }

    let bpp = if bytes_per_position == 0 { crate::parser::BYTES_PER_POSITION } else { bytes_per_position };
    let strategy = crate::parser::ArchiveEntryLength { page_length: bpp };

    let book = h.book.clone().unwrap();
    match h.archive.positions_by_reading_order(&book, &strategy) {
        Ok(positions) => to_json(&positions),
        Err(e) => { set_error(e.to_string()); ptr::null_mut() }
    }
}

// ── Resource access ───────────────────────────────────────────────────────────

/// Return the cover image bytes.
///
/// On success:
/// - The return value is a pointer to `*out_len` bytes of image data.
///   Free with `epub_free_bytes(ptr, *out_len)`.
/// - `*out_media_type` is a null-terminated media-type string (e.g. `"image/jpeg"`).
///   Free with `epub_free_string(*out_media_type)`.
///
/// Returns `NULL` on failure; `out_len` and `out_media_type` are left unchanged.
///
/// # Safety
/// - `handle` must be a valid non-null pointer obtained from `epub_open*`.
/// - `out_len` and `out_media_type` must be valid non-null writable pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_get_cover_image(
    handle: *mut EpubHandle,
    out_len: *mut usize,
    out_media_type: *mut *mut c_char,
) -> *mut c_uchar {
    clear_error();

    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => { set_error("epub_get_cover_image: null handle"); return ptr::null_mut(); }
    };
    if out_len.is_null() || out_media_type.is_null() {
        set_error("epub_get_cover_image: out_len and out_media_type must be non-null");
        return ptr::null_mut();
    }

    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }

    let book = h.book.clone().unwrap();
    match h.archive.get_cover_image(&book) {
        Ok((bytes, media_type)) => {
            let len = bytes.len();
            let media_type_ptr = match CString::new(media_type) {
                Ok(cs) => cs.into_raw(),
                Err(_) => { set_error("Internal: media_type contained a null byte"); return ptr::null_mut(); }
            };
            // SAFETY: out_len and out_media_type are valid pointers (checked above).
            unsafe {
                *out_len = len;
                *out_media_type = media_type_ptr;
            }
            into_raw_bytes(bytes)
        }
        Err(e) => { set_error(e.to_string()); ptr::null_mut() }
    }
}

/// Return the raw bytes of a manifest resource identified by its `href`.
///
/// `href` is the relative href as it appears in the EPUB manifest
/// (e.g. `"OEBPS/chapter1.xhtml"` or just `"chapter1.xhtml"`).
///
/// On success the return value points to `*out_len` bytes. Free with
/// `epub_free_bytes(ptr, *out_len)`. Returns `NULL` on failure.
///
/// # Safety
/// - `handle` must be a valid non-null pointer obtained from `epub_open*`.
/// - `href` must be a valid null-terminated C string.
/// - `out_len` must be a valid non-null writable pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_get_resource(
    handle: *mut EpubHandle,
    href: *const c_char,
    out_len: *mut usize,
) -> *mut c_uchar {
    clear_error();

    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => { set_error("epub_get_resource: null handle"); return ptr::null_mut(); }
    };
    if href.is_null() || out_len.is_null() {
        set_error("epub_get_resource: href and out_len must be non-null");
        return ptr::null_mut();
    }

    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }

    // SAFETY: href is a valid null-terminated C string (caller contract).
    let href_str = unsafe { CStr::from_ptr(href) }.to_string_lossy();

    let book = h.book.clone().unwrap();
    match h.archive.get_resource_by_href(&book, href_str.as_ref()) {
        Ok(bytes) => {
            let len = bytes.len();
            // SAFETY: out_len is a valid pointer (checked above).
            unsafe { *out_len = len; }
            into_raw_bytes(bytes)
        }
        Err(e) => { set_error(e.to_string()); ptr::null_mut() }
    }
}

// ── Stateless CFI utilities ───────────────────────────────────────────────────

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
///     "xpath": "//*[@id='section1']/...",
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
    clear_error();

    if cfi_str.is_null() {
        set_error("epub_resolve_cfi: cfi_str pointer is null");
        return ptr::null_mut();
    }

    // SAFETY: cfi_str is a valid null-terminated C string (caller contract).
    let s = unsafe { CStr::from_ptr(cfi_str) }.to_string_lossy();

    use std::str::FromStr;
    let cfi = match crate::cfi::EpubCfi::from_str(s.as_ref()) {
        Ok(c) => c,
        Err(e) => { set_error(format!("Invalid CFI: {e}")); return ptr::null_mut(); }
    };

    match cfi.resolve() {
        Some(resolution) => to_json(&resolution),
        None => { set_error("CFI has no local path (missing '!' separator)"); ptr::null_mut() }
    }
}

// ── Error & memory management ─────────────────────────────────────────────────

/// Return the error message from the most recent failed call on this thread.
///
/// Returns `NULL` if the last call succeeded.
/// The returned pointer is valid until the next FFI call on the same thread.
/// Do **not** free this pointer — it is thread-local storage.
#[unsafe(no_mangle)]
pub extern "C" fn epub_last_error() -> *const c_char {
    LAST_ERROR.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|cs| cs.as_ptr())
            .unwrap_or(ptr::null())
    })
}

/// Free a `char *` returned by any epub-rs function.
///
/// Safe to call with `NULL` (no-op).
///
/// # Safety
/// `s` must either be `NULL` or a pointer previously returned by an epub-rs
/// function that returns `char *`, and must not have been freed already.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_free_string(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: s was created by CString::into_raw in this module.
        unsafe { drop(CString::from_raw(s)); }
    }
}

/// Free a byte buffer returned by any epub-rs function.
///
/// `buf` must be the pointer and `len` must be the exact length originally
/// written to the `out_len` parameter. Safe to call with a `NULL` `buf` (no-op).
///
/// # Safety
/// `buf` must either be `NULL` or a pointer previously returned by an epub-rs
/// function that returns `uint8_t *`, paired with the matching `len`.
/// Must not have been freed already.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_free_bytes(buf: *mut c_uchar, len: usize) {
    if !buf.is_null() {
        // SAFETY: buf + len describe the original Box<[u8]> slice we leaked.
        unsafe {
            drop(Box::from_raw(std::slice::from_raw_parts_mut(buf, len)));
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
            .add_chapter("ch1", "text/ch1.xhtml", b"<html><body><p>Hello FFI</p></body></html>".to_vec())
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
        assert!(!json_ptr.is_null(), "epub_parse returned NULL: {:?}",
            unsafe { std::ffi::CStr::from_ptr(epub_last_error()) });

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
        assert!(!err.is_null(), "epub_last_error should be set after failure");
    }

    #[test]
    fn test_epub_resolve_cfi_stateless() {
        let cfi = std::ffi::CString::new("epubcfi(/6/4[ch1]!/4/2/1:0)").unwrap();
        let result = unsafe { epub_resolve_cfi(cfi.as_ptr()) };
        assert!(!result.is_null(), "epub_resolve_cfi returned NULL: {:?}",
            unsafe { std::ffi::CStr::from_ptr(epub_last_error()) });

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
}
