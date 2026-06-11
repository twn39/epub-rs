use std::cell::RefCell;
use std::ffi::CString;
use std::io::Cursor;
use std::os::raw::{c_char, c_uchar};
use std::ptr;

use crate::model::EpubBook;
use crate::parser::EpubArchive;
use crate::provider::ZipProvider;

// ── Thread-local error storage ────────────────────────────────────────────────

thread_local! {
    pub(crate) static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

pub(crate) fn set_error(msg: impl AsRef<str>) {
    LAST_ERROR.with(|cell| {
        *cell.borrow_mut() = CString::new(msg.as_ref()).ok();
    });
}

pub(crate) fn clear_error() {
    LAST_ERROR.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

// ── Opaque handles ────────────────────────────────────────────────────────────

/// Opaque EPUB archive handle.
///
/// Allocated by `epub_open` / `epub_open_file`, freed by `epub_free`.
/// C code must never attempt to inspect or copy the pointed-to memory.
pub struct EpubHandle {
    pub(crate) archive: EpubArchive<ZipProvider<Cursor<Vec<u8>>>>,
    /// Lazily parsed on the first call to any API that needs book metadata.
    pub(crate) book: Option<EpubBook>,
}

impl EpubHandle {
    /// Lazily parse the OPF on first call; subsequent calls return the cached result.
    pub(crate) fn ensure_parsed(&mut self) -> Result<(), String> {
        if self.book.is_none() {
            self.book = Some(self.archive.parse().map_err(|e| e.to_string())?);
        }
        Ok(())
    }
}

/// Opaque EPUB generator handle.
///
/// Allocated by `epub_generator_new()`, freed by `epub_generator_free()`.
pub struct EpubGeneratorHandle {
    pub(crate) builder: Option<crate::generator::EpubBuilder>,
}

// ── Allocation helpers ────────────────────────────────────────────────────────

/// Move a Rust `String` into a heap-allocated, null-terminated `char *`.
///
/// The returned pointer must eventually be passed to `epub_free_string()`.
/// Returns `NULL` and sets the thread-local error if the string contains an
/// interior null byte (which is theoretically impossible for valid JSON/UTF-8).
pub(crate) fn into_c_string(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(cs) => cs.into_raw(),
        Err(e) => {
            set_error(format!("Internal error: failed to create C string: {e}"));
            ptr::null_mut()
        }
    }
}

/// Serialise `value` to JSON and return it as a heap-allocated `char *`.
pub(crate) fn to_json<T: serde::Serialize>(value: &T) -> *mut c_char {
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
pub(crate) fn into_raw_bytes(bytes: Vec<u8>) -> *mut c_uchar {
    let mut boxed: Box<[u8]> = bytes.into_boxed_slice();
    let ptr = boxed.as_mut_ptr();
    // Leak the box — ownership is transferred to the C caller.
    std::mem::forget(boxed);
    ptr
}

// ── Memory Management & Handle Lifecycle FFI ──────────────────────────────────

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
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(buf, len)));
        }
    }
}
