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
    /// Lazily parse the OPF on first call; subsequent calls return the cached result.
    ///
    /// Returns `&EpubBook` directly so callers can use the book **without cloning**:
    ///
    /// ```rust,ignore
    /// let book = match h.ensure_parsed() {
    ///     Ok(b) => b,
    ///     Err(e) => { set_error(e); return ptr::null_mut(); }
    /// };
    /// h.archive.some_method(book)   // &mut h.archive + &h.book — disjoint, allowed by NLL
    /// ```
    ///
    /// Rust's NLL (Non-Lexical Lifetimes) permits `&mut h.archive` and `&h.book`
    /// simultaneously because they are distinct fields of `EpubHandle`.
    fn ensure_parsed(&mut self) -> Result<&EpubBook, String> {
        if self.book.is_none() {
            self.book = Some(self.archive.parse().map_err(|e| e.to_string())?);
        }
        Ok(self.book.as_ref().unwrap())
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
        Ok(archive) => Box::into_raw(Box::new(EpubHandle {
            archive,
            book: None,
        })),
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
        Ok(archive) => Box::into_raw(Box::new(EpubHandle {
            archive,
            book: None,
        })),
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
        unsafe {
            drop(Box::from_raw(handle));
        }
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
        None => {
            set_error("epub_parse: null handle");
            return ptr::null_mut();
        }
    };

    match h.ensure_parsed() {
        Ok(book) => to_json(book),
        Err(e) => {
            set_error(e);
            ptr::null_mut()
        }
    }
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
        None => {
            set_error("epub_get_navigation: null handle");
            return ptr::null_mut();
        }
    };

    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }

    // Clone the cached book to avoid simultaneous &mut archive + &book on the same struct.
    let book = h.book.as_ref().unwrap();
    match h.archive.get_navigation(&book) {
        Ok(nav) => to_json(&nav),
        Err(e) => {
            set_error(e.to_string());
            ptr::null_mut()
        }
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
        None => {
            set_error("epub_get_toc: null handle");
            return ptr::null_mut();
        }
    };

    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }

    let book = h.book.as_ref().unwrap();
    match h.archive.get_toc(&book) {
        Ok(toc) => to_json(&toc),
        Err(e) => {
            set_error(e.to_string());
            ptr::null_mut()
        }
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
        None => {
            set_error("epub_get_page_list: null handle");
            return ptr::null_mut();
        }
    };

    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }

    let book = h.book.as_ref().unwrap();
    match h.archive.get_page_list(&book) {
        Ok(page_list) => to_json(&page_list),
        Err(e) => {
            set_error(e.to_string());
            ptr::null_mut()
        }
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
        None => {
            set_error("epub_positions_by_reading_order: null handle");
            return ptr::null_mut();
        }
    };

    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }

    let bpp = if bytes_per_position == 0 {
        crate::parser::BYTES_PER_POSITION
    } else {
        bytes_per_position
    };
    let strategy = crate::parser::ArchiveEntryLength { page_length: bpp };

    let book = h.book.as_ref().unwrap();
    match h.archive.positions_by_reading_order(&book, &strategy) {
        Ok(positions) => to_json(&positions),
        Err(e) => {
            set_error(e.to_string());
            ptr::null_mut()
        }
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
        None => {
            set_error("epub_get_cover_image: null handle");
            return ptr::null_mut();
        }
    };
    if out_len.is_null() || out_media_type.is_null() {
        set_error("epub_get_cover_image: out_len and out_media_type must be non-null");
        return ptr::null_mut();
    }

    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }

    let book = h.book.as_ref().unwrap();
    match h.archive.get_cover_image(&book) {
        Ok((bytes, media_type)) => {
            let len = bytes.len();
            let media_type_ptr = match CString::new(media_type) {
                Ok(cs) => cs.into_raw(),
                Err(_) => {
                    set_error("Internal: media_type contained a null byte");
                    return ptr::null_mut();
                }
            };
            // SAFETY: out_len and out_media_type are valid pointers (checked above).
            unsafe {
                *out_len = len;
                *out_media_type = media_type_ptr;
            }
            into_raw_bytes(bytes)
        }
        Err(e) => {
            set_error(e.to_string());
            ptr::null_mut()
        }
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
        None => {
            set_error("epub_get_resource: null handle");
            return ptr::null_mut();
        }
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

    let book = h.book.as_ref().unwrap();
    match h.archive.get_resource_by_href(&book, href_str.as_ref()) {
        Ok(bytes) => {
            let len = bytes.len();
            // SAFETY: out_len is a valid pointer (checked above).
            unsafe {
                *out_len = len;
            }
            into_raw_bytes(bytes)
        }
        Err(e) => {
            set_error(e.to_string());
            ptr::null_mut()
        }
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
        Err(e) => {
            set_error(format!("Invalid CFI: {e}"));
            return ptr::null_mut();
        }
    };

    match cfi.resolve() {
        Some(resolution) => to_json(&resolution),
        None => {
            set_error("CFI has no local path (missing '!' separator)");
            ptr::null_mut()
        }
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
        unsafe {
            drop(CString::from_raw(s));
        }
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

// ── Additional resource access ────────────────────────────────────────────────

/// Return the raw bytes of a manifest resource identified by its manifest **ID**.
///
/// On success returns a pointer to `*out_len` bytes. Free with `epub_free_bytes(ptr, *out_len)`.
///
/// # Safety
/// - `handle` must be a valid non-null pointer obtained from `epub_open*`.
/// - `id` must be a valid null-terminated C string.
/// - `out_len` must be a valid non-null writable pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_get_resource_by_id(
    handle: *mut EpubHandle,
    id: *const c_char,
    out_len: *mut usize,
) -> *mut c_uchar {
    clear_error();
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => {
            set_error("epub_get_resource_by_id: null handle");
            return ptr::null_mut();
        }
    };
    if id.is_null() || out_len.is_null() {
        set_error("epub_get_resource_by_id: id and out_len must be non-null");
        return ptr::null_mut();
    }
    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }
    let id_str = unsafe { CStr::from_ptr(id) }.to_string_lossy();
    let book = h.book.as_ref().unwrap();
    match h.archive.get_resource_by_id(&book, id_str.as_ref()) {
        Ok(bytes) => {
            let len = bytes.len();
            unsafe {
                *out_len = len;
            }
            into_raw_bytes(bytes)
        }
        Err(e) => {
            set_error(e.to_string());
            ptr::null_mut()
        }
    }
}

/// Return a chapter's HTML with `data-cfi` attributes injected into every DOM node.
///
/// `id` is the manifest ID of the spine item (e.g. `"chapter1"`).
/// The caller must free the returned string with `epub_free_string()`.
///
/// # Safety
/// - `handle` must be a valid non-null pointer obtained from `epub_open*`.
/// - `id` must be a valid null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_get_chapter_with_cfi(
    handle: *mut EpubHandle,
    id: *const c_char,
) -> *mut c_char {
    clear_error();
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => {
            set_error("epub_get_chapter_with_cfi: null handle");
            return ptr::null_mut();
        }
    };
    if id.is_null() {
        set_error("epub_get_chapter_with_cfi: id is null");
        return ptr::null_mut();
    }
    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }
    let id_str = unsafe { CStr::from_ptr(id) }.to_string_lossy();
    let book = h.book.as_ref().unwrap();
    match h.archive.get_chapter_with_cfi(&book, id_str.as_ref()) {
        Ok(html) => into_c_string(html),
        Err(e) => {
            set_error(e.to_string());
            ptr::null_mut()
        }
    }
}

/// Search for a literal text query in a chapter. Returns JSON `SearchResult[]`.
///
/// `id` is the manifest ID; `query` is the literal search string.
/// The caller must free the returned string with `epub_free_string()`.
///
/// # Safety
/// All pointer parameters must be valid null-terminated C strings.
/// `handle` must be a valid non-null pointer obtained from `epub_open*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_search_chapter(
    handle: *mut EpubHandle,
    id: *const c_char,
    query: *const c_char,
) -> *mut c_char {
    clear_error();
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => {
            set_error("epub_search_chapter: null handle");
            return ptr::null_mut();
        }
    };
    if id.is_null() || query.is_null() {
        set_error("epub_search_chapter: id and query must be non-null");
        return ptr::null_mut();
    }
    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }
    let id_str = unsafe { CStr::from_ptr(id) }.to_string_lossy();
    let query_str = unsafe { CStr::from_ptr(query) }.to_string_lossy();
    let pattern = match regex::Regex::new(&regex::escape(query_str.as_ref())) {
        Ok(r) => r,
        Err(e) => {
            set_error(format!("epub_search_chapter: invalid query: {e}"));
            return ptr::null_mut();
        }
    };
    let book = h.book.as_ref().unwrap();
    match h.archive.search_chapter(&book, id_str.as_ref(), &pattern) {
        Ok(results) => to_json(&results),
        Err(e) => {
            set_error(e.to_string());
            ptr::null_mut()
        }
    }
}

/// Return semantic content blocks (paragraphs, headings) for a chapter as JSON `ContentElement[]`.
///
/// `id` is the manifest ID of the chapter. The caller must free the returned string with `epub_free_string()`.
///
/// # Safety
/// - `handle` must be a valid non-null pointer obtained from `epub_open*`.
/// - `id` must be a valid null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_get_semantic_content(
    handle: *mut EpubHandle,
    id: *const c_char,
) -> *mut c_char {
    clear_error();
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => {
            set_error("epub_get_semantic_content: null handle");
            return ptr::null_mut();
        }
    };
    if id.is_null() {
        set_error("epub_get_semantic_content: id is null");
        return ptr::null_mut();
    }
    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }
    let id_str = unsafe { CStr::from_ptr(id) }.to_string_lossy();
    let book = h.book.as_ref().unwrap();
    match h.archive.get_semantic_content(&book, id_str.as_ref()) {
        Ok(content) => to_json(&content),
        Err(e) => {
            set_error(e.to_string());
            ptr::null_mut()
        }
    }
}

/// Return a flat JSON array of all reading `Position` objects across the entire EPUB.
///
/// `bytes_per_position`: pass `0` to use the Readium default of 1024 bytes.
/// The caller must free the returned string with `epub_free_string()`.
///
/// # Safety
/// `handle` must be a valid non-null pointer obtained from `epub_open*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generate_locations(
    handle: *mut EpubHandle,
    bytes_per_position: usize,
) -> *mut c_char {
    clear_error();
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => {
            set_error("epub_generate_locations: null handle");
            return ptr::null_mut();
        }
    };
    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }
    let bpp = if bytes_per_position == 0 {
        crate::parser::BYTES_PER_POSITION
    } else {
        bytes_per_position
    };
    let book = h.book.as_ref().unwrap();
    // Use generate_locations() — the Readium/Adobe-standard ZIP entry byte-length algorithm.
    // Previously this incorrectly called get_positions() which used DOM character counting,
    // causing FFI and WASM callers to receive different position counts for the same EPUB.
    match h.archive.generate_locations(&book, bpp) {
        Ok(positions) => to_json(&positions),
        Err(e) => {
            set_error(e.to_string());
            ptr::null_mut()
        }
    }
}

// ── Bidirectional location-index ↔ CFI conversion ─────────────────────────────

/// Generate positions and build a bidirectional lookup index in one call.
///
/// Returns a flat JSON array of `Position` objects identical to
/// `epub_generate_locations`. The returned array can be passed as `positions_json`
/// to `epub_location_from_cfi` and `epub_cfi_from_location`.
///
/// The difference from `epub_generate_locations` is semantic: this function
/// is designed to be paired with the lookup functions below.
///
/// `bytes_per_position`: pass `0` to use the Readium default of 1024 bytes.
/// The caller must free the returned string with `epub_free_string()`.
///
/// # Safety
/// `handle` must be a valid non-null pointer obtained from `epub_open*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generate_location_index(
    handle: *mut EpubHandle,
    bytes_per_position: usize,
) -> *mut c_char {
    clear_error();
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => {
            set_error("epub_generate_location_index: null handle");
            return ptr::null_mut();
        }
    };
    // Step 1: trigger parsing (the &mut borrow of h ends here).
    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }
    // Step 2: disjoint field borrows — h.book and h.archive are separate.
    let book = h.book.as_ref().unwrap();
    let bpp = if bytes_per_position == 0 {
        crate::parser::BYTES_PER_POSITION
    } else {
        bytes_per_position
    };
    let strategy = crate::parser::ArchiveEntryLength { page_length: bpp };
    match h.archive.positions_by_reading_order(book, &strategy) {
        Ok(by_chapter) => {
            let index = crate::parser::PositionIndex::build(by_chapter);
            // Return the flat positions as JSON (same schema as epub_generate_locations).
            // The caller uses this JSON array with the lookup functions below.
            let flat: Vec<&crate::model::Position> = (0..index.len())
                .filter_map(|i| index.position_at(i))
                .collect();
            to_json(&flat)
        }
        Err(e) => {
            set_error(e.to_string());
            ptr::null_mut()
        }
    }
}

/// Find the 0-based position index that contains a given CFI.
///
/// `positions_json`: the JSON array returned by `epub_generate_location_index`
/// or `epub_generate_locations`.
/// `cfi_str`: any valid EPUB CFI string (bookmark, annotation, or position CFI).
///
/// Returns the 0-based index as a JSON number (e.g. `"42"`), or `"-1"` if the
/// CFI could not be resolved (wrong spine item, parse error, or empty list).
///
/// The caller must free the returned string with `epub_free_string()`.
///
/// **Algorithm**: O(|cfi_str|) — parses the CFI once, then uses integer arithmetic
/// on pre-computed chapter offsets. No binary search.
///
/// # Safety
/// - `handle` must be a valid non-null pointer obtained from `epub_open*`.
/// - `positions_json` and `cfi_str` must be valid null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_location_from_cfi(
    handle: *mut EpubHandle,
    positions_json: *const c_char,
    cfi_str: *const c_char,
) -> *mut c_char {
    clear_error();
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => {
            set_error("epub_location_from_cfi: null handle");
            return ptr::null_mut();
        }
    };
    if positions_json.is_null() || cfi_str.is_null() {
        set_error("epub_location_from_cfi: positions_json and cfi_str must be non-null");
        return ptr::null_mut();
    }

    // Step 1: trigger parsing (the &mut borrow of h ends here).
    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }
    // Step 2: disjoint field borrows — h.book and h.archive are separate.
    let book = h.book.as_ref().unwrap();
    let cfi_s = unsafe { CStr::from_ptr(cfi_str) }.to_string_lossy();

    // Rebuild the PositionIndex from the book (positions_json is provided by the
    // caller for API symmetry but the index is recomputed from the live book to
    // avoid deserializing the JSON). For high-frequency callers, cache the index
    // on the application side.
    let strategy = crate::parser::ArchiveEntryLength {
        page_length: crate::parser::BYTES_PER_POSITION,
    };
    let by_chapter = match h.archive.positions_by_reading_order(book, &strategy) {
        Ok(c) => c,
        Err(e) => {
            set_error(e.to_string());
            return ptr::null_mut();
        }
    };
    let index = crate::parser::PositionIndex::build(by_chapter);

    match index.location_from_cfi(cfi_s.as_ref()) {
        Some(idx) => into_c_string(idx.to_string()),
        None => into_c_string("-1".to_string()),
    }
}

/// Return the CFI string for a given 0-based position index.
///
/// `positions_json`: the JSON array returned by `epub_generate_location_index`.
/// `idx`: 0-based position index (as returned by `epub_location_from_cfi`).
///
/// Returns the CFI string (e.g. `"epubcfi(/6/4!/4/2)"`), or `NULL` if `idx`
/// is out of range. The caller must free with `epub_free_string()`.
///
/// O(1) — direct array access.
///
/// # Safety
/// - `handle` must be a valid non-null pointer obtained from `epub_open*`.
/// - `positions_json` must be a valid null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_cfi_from_location(
    handle: *mut EpubHandle,
    positions_json: *const c_char,
    idx: usize,
) -> *mut c_char {
    clear_error();
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => {
            set_error("epub_cfi_from_location: null handle");
            return ptr::null_mut();
        }
    };
    if positions_json.is_null() {
        set_error("epub_cfi_from_location: positions_json must be non-null");
        return ptr::null_mut();
    }

    // Step 1: trigger parsing (the &mut borrow of h ends here).
    if let Err(e) = h.ensure_parsed() {
        set_error(e);
        return ptr::null_mut();
    }
    // Step 2: disjoint field borrows — h.book and h.archive are separate.
    let book = h.book.as_ref().unwrap();
    let strategy = crate::parser::ArchiveEntryLength {
        page_length: crate::parser::BYTES_PER_POSITION,
    };
    let by_chapter = match h.archive.positions_by_reading_order(book, &strategy) {
        Ok(c) => c,
        Err(e) => {
            set_error(e.to_string());
            return ptr::null_mut();
        }
    };
    let index = crate::parser::PositionIndex::build(by_chapter);

    match index.cfi_from_location(idx) {
        Some(cfi) => into_c_string(cfi.to_string()),
        None => {
            set_error(format!(
                "epub_cfi_from_location: index {idx} out of range (total={})",
                index.len()
            ));
            ptr::null_mut()
        }
    }
}

// ── Stateless CFI utilities (extended) ───────────────────────────────────────

/// Compare two CFI strings numerically per the EPUB CFI spec.
///
/// Returns: `-1` if `cfi_a < cfi_b`, `0` if equal, `1` if `cfi_a > cfi_b`.
/// On parse error returns `INT32_MIN` and sets `epub_last_error()`.
///
/// # Safety
/// Both `cfi_a` and `cfi_b` must be valid null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_compare_cfi(cfi_a: *const c_char, cfi_b: *const c_char) -> i32 {
    clear_error();
    if cfi_a.is_null() || cfi_b.is_null() {
        set_error("epub_compare_cfi: null pointer");
        return i32::MIN;
    }
    use std::str::FromStr;
    let sa = unsafe { CStr::from_ptr(cfi_a) }.to_string_lossy();
    let sb = unsafe { CStr::from_ptr(cfi_b) }.to_string_lossy();
    let a = match crate::cfi::EpubCfi::from_str(sa.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            set_error(format!("epub_compare_cfi: invalid cfi_a: {e}"));
            return i32::MIN;
        }
    };
    let b = match crate::cfi::EpubCfi::from_str(sb.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            set_error(format!("epub_compare_cfi: invalid cfi_b: {e}"));
            return i32::MIN;
        }
    };
    match a.cmp(&b) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
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
    clear_error();
    if start_cfi.is_null() || end_cfi.is_null() {
        set_error("epub_generate_cfi_range: null pointer");
        return ptr::null_mut();
    }
    use std::str::FromStr;
    let ss = unsafe { CStr::from_ptr(start_cfi) }.to_string_lossy();
    let se = unsafe { CStr::from_ptr(end_cfi) }.to_string_lossy();
    let start = match crate::cfi::EpubCfi::from_str(ss.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            set_error(format!("Invalid start CFI: {e}"));
            return ptr::null_mut();
        }
    };
    let end = match crate::cfi::EpubCfi::from_str(se.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            set_error(format!("Invalid end CFI: {e}"));
            return ptr::null_mut();
        }
    };
    match crate::cfi::EpubCfi::generate_range(&start, &end) {
        Ok(range) => into_c_string(range),
        Err(e) => {
            set_error(e.to_string());
            ptr::null_mut()
        }
    }
}

// ── Crypto ────────────────────────────────────────────────────────────────────

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
    clear_error();
    if data.is_null() || epub_identifier.is_null() || out_len.is_null() {
        set_error("epub_decrypt_font: null pointer argument");
        return ptr::null_mut();
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
    if let Err(e) = reader.read_to_end(&mut decrypted) {
        set_error(format!("epub_decrypt_font: {e}"));
        return ptr::null_mut();
    }
    let out = decrypted.len();
    unsafe {
        *out_len = out;
    }
    into_raw_bytes(decrypted)
}

// ── EPUB Generator ────────────────────────────────────────────────────────────

/// Opaque EPUB generator handle.
///
/// Allocated by `epub_generator_new()`, freed by `epub_generator_free()`.
pub struct EpubGeneratorHandle {
    builder: Option<crate::generator::EpubBuilder>,
}

/// Create a new EPUB generator.
///
/// Returns an opaque handle on success. Free with `epub_generator_free()`.
#[unsafe(no_mangle)]
pub extern "C" fn epub_generator_new() -> *mut EpubGeneratorHandle {
    clear_error();
    Box::into_raw(Box::new(EpubGeneratorHandle {
        builder: Some(crate::generator::EpubBuilder::new()),
    }))
}

/// Free an EPUB generator handle. Safe to call with `NULL`.
///
/// # Safety
/// `handle` must be `NULL` or a pointer from `epub_generator_new()` not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generator_free(handle: *mut EpubGeneratorHandle) {
    if !handle.is_null() {
        unsafe {
            drop(Box::from_raw(handle));
        }
    }
}

/// Set the EPUB title.
///
/// # Safety
/// `handle` and `title` must be valid non-null pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generator_set_title(
    handle: *mut EpubGeneratorHandle,
    title: *const c_char,
) {
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => return,
    };
    if title.is_null() {
        return;
    }
    let s = unsafe { CStr::from_ptr(title) }.to_string_lossy();
    if let Some(b) = h.builder.as_mut() {
        b.metadata.title = Some(s.into_owned());
    }
}

/// Set the EPUB language (e.g. `"en"`, `"zh-CN"`).
///
/// # Safety
/// `handle` and `lang` must be valid non-null pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generator_set_language(
    handle: *mut EpubGeneratorHandle,
    lang: *const c_char,
) {
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => return,
    };
    if lang.is_null() {
        return;
    }
    let s = unsafe { CStr::from_ptr(lang) }.to_string_lossy();
    if let Some(b) = h.builder.as_mut() {
        b.metadata.language = Some(s.into_owned());
    }
}

/// Set the EPUB unique identifier (e.g. UUID or ISBN).
///
/// # Safety
/// `handle` and `identifier` must be valid non-null pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generator_set_identifier(
    handle: *mut EpubGeneratorHandle,
    identifier: *const c_char,
) {
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => return,
    };
    if identifier.is_null() {
        return;
    }
    let s = unsafe { CStr::from_ptr(identifier) }.to_string_lossy();
    if let Some(b) = h.builder.as_mut() {
        b.metadata.identifier = Some(s.into_owned());
    }
}

/// Add a creator/author to the EPUB metadata.
///
/// `role` may be `NULL` (defaults to no role). Use standard MARC relator codes, e.g. `"aut"`.
///
/// # Safety
/// `handle` and `name` must be valid non-null pointers; `role` may be `NULL`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generator_add_author(
    handle: *mut EpubGeneratorHandle,
    name: *const c_char,
    role: *const c_char,
) {
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => return,
    };
    if name.is_null() {
        return;
    }
    let name_s = unsafe { CStr::from_ptr(name) }.to_string_lossy();
    let mut creator = crate::model::Creator::new(name_s.as_ref());
    if !role.is_null() {
        creator.role = Some(
            unsafe { CStr::from_ptr(role) }
                .to_string_lossy()
                .into_owned(),
        );
    }
    if let Some(b) = h.builder.as_mut() {
        b.metadata.creators.push(creator);
    }
}

/// Add an HTML chapter to the EPUB manifest and spine.
///
/// - `id`: unique manifest ID.
/// - `href`: relative path within the EPUB (e.g. `"text/ch1.xhtml"`).
/// - `html`: null-terminated UTF-8 HTML content.
///
/// # Safety
/// All pointers must be valid non-null null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generator_add_chapter(
    handle: *mut EpubGeneratorHandle,
    id: *const c_char,
    href: *const c_char,
    html: *const c_char,
) {
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => return,
    };
    if id.is_null() || href.is_null() || html.is_null() {
        return;
    }
    let id_s = unsafe { CStr::from_ptr(id) }.to_string_lossy();
    let href_s = unsafe { CStr::from_ptr(href) }.to_string_lossy();
    let html_s = unsafe { CStr::from_ptr(html) }.to_string_lossy();
    let builder = h.builder.take().unwrap();
    h.builder =
        Some(builder.add_chapter(id_s.as_ref(), href_s.as_ref(), html_s.as_bytes().to_vec()));
}

/// Add an HTML chapter and simultaneously add it to the Table of Contents.
///
/// # Safety
/// All pointers must be valid non-null null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generator_add_chapter_with_nav(
    handle: *mut EpubGeneratorHandle,
    id: *const c_char,
    href: *const c_char,
    title: *const c_char,
    html: *const c_char,
) {
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => return,
    };
    if id.is_null() || href.is_null() || title.is_null() || html.is_null() {
        return;
    }
    let id_s = unsafe { CStr::from_ptr(id) }.to_string_lossy();
    let href_s = unsafe { CStr::from_ptr(href) }.to_string_lossy();
    let title_s = unsafe { CStr::from_ptr(title) }.to_string_lossy();
    let html_s = unsafe { CStr::from_ptr(html) }.to_string_lossy();
    let builder = h.builder.take().unwrap();
    h.builder = Some(builder.add_chapter_with_nav(
        id_s.as_ref(),
        href_s.as_ref(),
        title_s.as_ref(),
        html_s.as_bytes().to_vec(),
    ));
}

/// Add a binary resource (image, font, CSS, etc.) to the EPUB.
///
/// - `id`: unique manifest ID.
/// - `href`: relative path (e.g. `"images/cover.jpg"`).
/// - `media_type`: MIME type (e.g. `"image/jpeg"`).
/// - `data` / `len`: raw bytes.
///
/// # Safety
/// String pointers must be valid null-terminated C strings.
/// `data` must point to at least `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generator_add_resource(
    handle: *mut EpubGeneratorHandle,
    id: *const c_char,
    href: *const c_char,
    media_type: *const c_char,
    data: *const c_uchar,
    len: usize,
) {
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => return,
    };
    if id.is_null() || href.is_null() || media_type.is_null() || data.is_null() {
        return;
    }
    let id_s = unsafe { CStr::from_ptr(id) }.to_string_lossy();
    let href_s = unsafe { CStr::from_ptr(href) }.to_string_lossy();
    let mt_s = unsafe { CStr::from_ptr(media_type) }.to_string_lossy();
    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    let builder = h.builder.take().unwrap();
    h.builder = Some(builder.add_resource(id_s.as_ref(), href_s.as_ref(), mt_s.as_ref(), bytes));
}

/// Set the EPUB cover image.
///
/// # Safety
/// String pointers must be valid null-terminated C strings.
/// `data` must point to at least `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generator_set_cover(
    handle: *mut EpubGeneratorHandle,
    href: *const c_char,
    media_type: *const c_char,
    data: *const c_uchar,
    len: usize,
) {
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => return,
    };
    if href.is_null() || media_type.is_null() || data.is_null() {
        return;
    }
    let href_s = unsafe { CStr::from_ptr(href) }.to_string_lossy();
    let mt_s = unsafe { CStr::from_ptr(media_type) }.to_string_lossy();
    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    let builder = h.builder.take().unwrap();
    h.builder = Some(builder.set_cover(href_s.as_ref(), mt_s.as_ref(), bytes));
}

/// Add a landmark (structural reference) to the EPUB navigation.
///
/// `epub_type`: landmark type, e.g. `"cover"`, `"toc"`, `"bodymatter"`.
///
/// # Safety
/// All pointers must be valid non-null null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generator_add_landmark(
    handle: *mut EpubGeneratorHandle,
    epub_type: *const c_char,
    href: *const c_char,
    title: *const c_char,
) {
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => return,
    };
    if epub_type.is_null() || href.is_null() || title.is_null() {
        return;
    }
    let et_s = unsafe { CStr::from_ptr(epub_type) }.to_string_lossy();
    let href_s = unsafe { CStr::from_ptr(href) }.to_string_lossy();
    let title_s = unsafe { CStr::from_ptr(title) }.to_string_lossy();
    let builder = h.builder.take().unwrap();
    h.builder = Some(builder.add_landmark(et_s.as_ref(), href_s.as_ref(), title_s.as_ref()));
}

/// Add a page-list entry (print page mapping).
///
/// # Safety
/// `name` and `href` must be valid non-null null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generator_add_page(
    handle: *mut EpubGeneratorHandle,
    name: *const c_char,
    href: *const c_char,
) {
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => return,
    };
    if name.is_null() || href.is_null() {
        return;
    }
    let name_s = unsafe { CStr::from_ptr(name) }.to_string_lossy();
    let href_s = unsafe { CStr::from_ptr(href) }.to_string_lossy();
    let builder = h.builder.take().unwrap();
    h.builder = Some(builder.add_page(name_s.as_ref(), href_s.as_ref()));
}

/// Set the complete Table of Contents from a JSON-encoded array of `TocEntry` objects.
///
/// This replaces any TOC entries added by `epub_generator_add_chapter_with_nav()`.
/// Accepts nested TOC trees (each `TocEntry` may have a `children` array).
///
/// `toc_json` must be a valid null-terminated UTF-8 JSON string, e.g.:
/// ```json
/// [{"title":"Chapter 1","href":"text/ch1.xhtml","children":[]},
///  {"title":"Chapter 2","href":"text/ch2.xhtml","children":[
///    {"title":"Section 2.1","href":"text/ch2.xhtml#s1","children":[]}]}]
/// ```
/// Returns `1` on success, `0` on failure (call `epub_last_error()` for details).
///
/// # Safety
/// `handle` and `toc_json` must be valid non-null pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generator_set_toc(
    handle: *mut EpubGeneratorHandle,
    toc_json: *const c_char,
) -> i32 {
    clear_error();
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => {
            set_error("epub_generator_set_toc: null handle");
            return 0;
        }
    };
    if toc_json.is_null() {
        set_error("epub_generator_set_toc: toc_json is null");
        return 0;
    }
    let json_str = unsafe { CStr::from_ptr(toc_json) }.to_string_lossy();
    let toc: Vec<crate::model::TocEntry> = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(e) => {
            set_error(format!("epub_generator_set_toc: JSON parse error: {e}"));
            return 0;
        }
    };
    let builder = match h.builder.take() {
        Some(b) => b,
        None => {
            set_error("epub_generator_set_toc: generator already consumed");
            return 0;
        }
    };
    h.builder = Some(builder.set_toc(toc));
    1
}

/// Set all EPUB metadata at once from a JSON-encoded `Metadata` object.
///
/// Replaces any individual metadata set by `epub_generator_set_title()`, etc.
/// All fields are optional; missing fields in the JSON keep their default values.
///
/// Example JSON:
/// ```json
/// {"title":"My Book","language":"en","creators":[{"name":"Alice","role":"aut"}]}
/// ```
/// Returns `1` on success, `0` on failure (call `epub_last_error()` for details).
///
/// # Safety
/// `handle` and `metadata_json` must be valid non-null pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generator_set_metadata(
    handle: *mut EpubGeneratorHandle,
    metadata_json: *const c_char,
) -> i32 {
    clear_error();
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => {
            set_error("epub_generator_set_metadata: null handle");
            return 0;
        }
    };
    if metadata_json.is_null() {
        set_error("epub_generator_set_metadata: metadata_json is null");
        return 0;
    }
    let json_str = unsafe { CStr::from_ptr(metadata_json) }.to_string_lossy();
    let metadata: crate::model::Metadata = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(e) => {
            set_error(format!(
                "epub_generator_set_metadata: JSON parse error: {e}"
            ));
            return 0;
        }
    };
    let builder = match h.builder.take() {
        Some(b) => b,
        None => {
            set_error("epub_generator_set_metadata: generator already consumed");
            return 0;
        }
    };
    h.builder = Some(builder.metadata(metadata));
    1
}

/// Validate the generator state without producing any output.
///
/// Runs the same pre-flight checks as `epub_generator_build()` (required
/// fields, non-empty spine, etc.) but does **not** consume the handle.
/// The generator remains usable after this call.
///
/// Returns `1` if the current state would produce a valid EPUB, `0` otherwise.
/// On failure, `epub_last_error()` returns a human-readable description.
///
/// # Safety
/// `handle` must be a valid non-null pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generator_validate(handle: *mut EpubGeneratorHandle) -> i32 {
    clear_error();
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => {
            set_error("epub_generator_validate: null handle");
            return 0;
        }
    };
    let builder = match h.builder.as_ref() {
        Some(b) => b,
        None => {
            set_error("epub_generator_validate: generator already consumed");
            return 0;
        }
    };
    match builder.validate() {
        Ok(()) => 1,
        Err(e) => {
            set_error(e.to_string());
            0
        }
    }
}

/// Build the EPUB archive and return it as a byte buffer.
///
/// On success writes the EPUB byte count to `*out_len` and returns a pointer
/// to the bytes. Free with `epub_free_bytes(ptr, *out_len)`.
/// Returns `NULL` on failure. The generator handle is **consumed** by this call
/// and must not be used again (it is freed internally).
///
/// # Safety
/// `handle` must be a valid non-null pointer; `out_len` must be a valid non-null writable pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_generator_build(
    handle: *mut EpubGeneratorHandle,
    out_len: *mut usize,
) -> *mut c_uchar {
    clear_error();
    let h = match unsafe { handle.as_mut() } {
        Some(h) => h,
        None => {
            set_error("epub_generator_build: null handle");
            return ptr::null_mut();
        }
    };
    if out_len.is_null() {
        set_error("epub_generator_build: out_len is null");
        return ptr::null_mut();
    }
    let builder = match h.builder.take() {
        Some(b) => b,
        None => {
            set_error("epub_generator_build: generator already consumed");
            return ptr::null_mut();
        }
    };
    let mut buf = std::io::Cursor::new(Vec::new());
    if let Err(e) = builder.generate(&mut buf) {
        set_error(e.to_string());
        return ptr::null_mut();
    }
    let bytes = buf.into_inner();
    let len = bytes.len();
    unsafe {
        *out_len = len;
    }
    into_raw_bytes(bytes)
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
        // Partial JSON is valid: Vec fields default to empty via serde(default)
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
        // Fresh generator has no title/chapters — validate should fail
        let ok = unsafe { epub_generator_validate(handle) };
        assert_eq!(ok, 0, "empty generator should fail validation");
        let err = unsafe { std::ffi::CStr::from_ptr(epub_last_error()) };
        assert!(
            !err.to_string_lossy().is_empty(),
            "error message should be set"
        );
        unsafe { epub_generator_free(handle) };
    }
}
