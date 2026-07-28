//! C FFI for package path / href resolution and resource lookup.
//!
//! Single-sources EPUB path semantics (chapter-relative joins, OPF-relative
//! probing, manifest media types, TOC title lookup) so hosts never
//! re-implement them per-platform.

use std::ffi::CStr;
use std::os::raw::{c_char, c_uchar};
use std::ptr;

use crate::ffi::common::{EpubHandle, into_c_string, into_raw_bytes};
use crate::ffi_boundary;

/// Read a required C string argument as an owned `String`.
///
/// # Safety
/// `ptr` must be null or a valid null-terminated C string.
unsafe fn read_required<'a>(ptr: *const c_char, name: &str) -> Result<std::borrow::Cow<'a, str>, crate::ffi::common::FfiError> {
    if ptr.is_null() {
        return Err(format!("{name} is null").into());
    }
    // SAFETY: caller guarantees a valid null-terminated C string.
    Ok(unsafe { CStr::from_ptr(ptr) }.to_string_lossy())
}

/// Read an optional C string (NULL → `None`, empty → `None`).
///
/// # Safety
/// `ptr` must be null or a valid null-terminated C string.
unsafe fn read_optional(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees a valid null-terminated C string.
    let s = unsafe { CStr::from_ptr(ptr) }.to_string_lossy();
    if s.is_empty() {
        None
    } else {
        Some(s.into_owned())
    }
}

/// Normalize a resource reference against a base directory (pure, no handle).
///
/// Strips query/fragment, percent-decodes, and resolves `.` / `..` segments.
/// The caller must free the returned string with `epub_free_string()`.
///
/// # Safety
/// Both pointers must be null or valid null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_normalize_path(
    base_dir: *const c_char,
    rel_path: *const c_char,
) -> *mut c_char {
    ffi_boundary!(ptr::null_mut(), {
        let base = unsafe { read_optional(base_dir) }.unwrap_or_default();
        let rel = unsafe { read_required(rel_path, "epub_normalize_path: rel_path")? };
        Ok(into_c_string(crate::path::normalize_path(&base, rel.as_ref())))
    })
}

/// Resolve `rel_href` against the chapter document it appears in (pure, no handle).
///
/// Semantics:
/// - external URLs (`http:`, `data:`, …) and fragment-only refs pass through unchanged
/// - empty refs resolve to the chapter path itself
/// - root-absolute (`/x/y`) resolves package-root-relative
/// - otherwise the ref is joined against the chapter's parent directory
///
/// Query strings and fragments are stripped (resource references cannot point
/// into fragments). The caller must free the result with `epub_free_string()`.
///
/// # Safety
/// Both pointers must be null or valid null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_resolve_chapter_href(
    chapter_href: *const c_char,
    rel_href: *const c_char,
) -> *mut c_char {
    ffi_boundary!(ptr::null_mut(), {
        let chapter = unsafe { read_optional(chapter_href) }.unwrap_or_default();
        let rel = unsafe { read_required(rel_href, "epub_resolve_chapter_href: rel_href")? };
        Ok(into_c_string(resolve_chapter_href(&chapter, rel.as_ref())))
    })
}

/// Shared implementation for [`epub_resolve_chapter_href`] (unit-testable).
fn resolve_chapter_href(chapter_href: &str, rel_href: &str) -> String {
    if rel_href.starts_with('#') || crate::path::is_external_url(rel_href) {
        return rel_href.to_string();
    }
    let clean = rel_href.split(['?', '#']).next().unwrap_or(rel_href);
    if clean.is_empty() {
        return chapter_href
            .split(['?', '#'])
            .next()
            .unwrap_or(chapter_href)
            .to_string();
    }
    if let Some(root_abs) = clean.strip_prefix('/') {
        return crate::path::normalize_path("", root_abs);
    }
    let base = match chapter_href.rfind('/') {
        Some(i) => &chapter_href[..i],
        None => "",
    };
    crate::path::normalize_path(base, clean)
}

/// Extension-based media type guess (pure, no handle).
///
/// Falls back to `application/octet-stream` for unknown extensions. Prefer
/// [`epub_resource_media_type`] when a handle is available (manifest wins).
/// The caller must free the result with `epub_free_string()`.
///
/// # Safety
/// `path` must be null or a valid null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_mime_for_path(path: *const c_char) -> *mut c_char {
    ffi_boundary!(ptr::null_mut(), {
        let p = unsafe { read_required(path, "epub_mime_for_path: path")? };
        Ok(into_c_string(crate::mime::mime_for_path(p.as_ref()).to_string()))
    })
}

/// Resolve a resource reference to the canonical package path that exists in
/// the archive (chapter-relative → OPF-relative → root-relative → manifest
/// fuzzy). `chapter_href` may be NULL or empty.
///
/// Returns NULL when no candidate exists (check `epub_last_error()` — it
/// stays empty for a plain miss). The caller must free a non-NULL result with
/// `epub_free_string()`.
///
/// # Safety
/// `handle` must be a valid non-null pointer from `epub_open*`; string
/// arguments must be null or valid null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_resolve_resource_path(
    handle: *mut EpubHandle,
    chapter_href: *const c_char,
    rel_path: *const c_char,
) -> *mut c_char {
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_resolve_resource_path: null handle")?;
        let rel = unsafe { read_required(rel_path, "epub_resolve_resource_path: rel_path")? };
        let chapter = unsafe { read_optional(chapter_href) };
        h.ensure_parsed()?;
        let book = h.book.as_book().expect("book ready after ensure_parsed");
        match h.archive.resolve_resource_path(book, chapter.as_deref(), rel.as_ref()) {
            Some(resolved) => Ok(into_c_string(resolved.zip_path)),
            None => Ok(ptr::null_mut()),
        }
    })
}

/// Media type for a manifest or package path (manifest first, extension table
/// fallback). Never returns NULL for a valid handle.
///
/// The caller must free the returned string with `epub_free_string()`.
///
/// # Safety
/// `handle` must be a valid non-null pointer from `epub_open*`; `path` must
/// be a valid null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_resource_media_type(
    handle: *mut EpubHandle,
    path: *const c_char,
) -> *mut c_char {
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_resource_media_type: null handle")?;
        let p = unsafe { read_required(path, "epub_resource_media_type: path")? };
        h.ensure_parsed()?;
        let book = h.book.as_book().expect("book ready after ensure_parsed");
        Ok(into_c_string(h.archive.resource_media_type(book, p.as_ref())))
    })
}

/// Resolve + load a resource in one call.
///
/// On success the return value points to `*out_len` bytes (free with
/// `epub_free_bytes(ptr, *out_len)`) and `*out_media_type` is a media-type
/// string (free with `epub_free_string()`). Returns NULL when the reference
/// cannot be resolved or read.
///
/// # Safety
/// `handle` must be a valid non-null pointer from `epub_open*`; `out_len` and
/// `out_media_type` must be valid non-null writable pointers; string arguments
/// must be null or valid null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_get_resolved_resource(
    handle: *mut EpubHandle,
    chapter_href: *const c_char,
    rel_path: *const c_char,
    out_len: *mut usize,
    out_media_type: *mut *mut c_char,
) -> *mut c_uchar {
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_get_resolved_resource: null handle")?;
        if out_len.is_null() || out_media_type.is_null() {
            return Err("epub_get_resolved_resource: out_len and out_media_type must be non-null".into());
        }
        let rel = unsafe { read_required(rel_path, "epub_get_resolved_resource: rel_path")? };
        let chapter = unsafe { read_optional(chapter_href) };
        h.ensure_parsed()?;
        let book = h.book.as_book().expect("book ready after ensure_parsed");
        let (bytes, media_type) =
            h.archive.get_resolved_resource(book, chapter.as_deref(), rel.as_ref())?;
        let media_type_ptr = std::ffi::CString::new(media_type)
            .map_err(|_| "Internal: media_type contained a null byte")?
            .into_raw();
        // SAFETY: out_len and out_media_type are valid pointers checked above.
        unsafe {
            *out_len = bytes.len();
            *out_media_type = media_type_ptr;
        }
        Ok(into_raw_bytes(bytes))
    })
}

/// TOC title for a nav href (verbatim → normalized → suffix matching).
///
/// Returns NULL when no entry matches. The caller must free a non-NULL result
/// with `epub_free_string()`.
///
/// # Safety
/// `handle` must be a valid non-null pointer from `epub_open*`; `href` must
/// be a valid null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_toc_title_for_href(
    handle: *mut EpubHandle,
    href: *const c_char,
) -> *mut c_char {
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_toc_title_for_href: null handle")?;
        let href = unsafe { read_required(href, "epub_toc_title_for_href: href")? };
        h.ensure_parsed()?;
        let book = h.book.as_book().expect("book ready after ensure_parsed");
        match h.archive.toc_title_for_href(book, href.as_ref()) {
            Some(title) => Ok(into_c_string(title)),
            None => Ok(ptr::null_mut()),
        }
    })
}

/// Absolute 0-based spine index for a nav/TOC href, or `-1` when no spine
/// item matches.
///
/// # Safety
/// `handle` must be a valid non-null pointer from `epub_open*`; `href` must
/// be a valid null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_spine_index_for_href(
    handle: *mut EpubHandle,
    href: *const c_char,
) -> isize {
    ffi_boundary!(-1, {
        let h = unsafe { handle.as_mut() }.ok_or("epub_spine_index_for_href: null handle")?;
        let href = unsafe { read_required(href, "epub_spine_index_for_href: href")? };
        h.ensure_parsed()?;
        let book = h.book.as_book().expect("book ready after ensure_parsed");
        Ok(match h.archive.spine_index_for_toc_href(book, href.as_ref()) {
            Some(idx) => idx as isize,
            None => -1,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapter_href_join() {
        assert_eq!(
            resolve_chapter_href("OEBPS/Text/ch1.xhtml", "../Images/a.jpg"),
            "OEBPS/Images/a.jpg"
        );
        assert_eq!(
            resolve_chapter_href("Text/ch1.xhtml", "img.png?v=1#x"),
            "Text/img.png"
        );
    }

    #[test]
    fn chapter_href_passthrough() {
        assert_eq!(
            resolve_chapter_href("OEBPS/ch1.xhtml", "https://example.com/a.png"),
            "https://example.com/a.png"
        );
        assert_eq!(resolve_chapter_href("OEBPS/ch1.xhtml", "#frag"), "#frag");
        assert_eq!(
            resolve_chapter_href("OEBPS/ch1.xhtml", "data:image/png;base64,xx"),
            "data:image/png;base64,xx"
        );
    }

    #[test]
    fn chapter_href_root_absolute_and_empty() {
        assert_eq!(resolve_chapter_href("OEBPS/ch1.xhtml", "/OEBPS/a.png"), "OEBPS/a.png");
        assert_eq!(resolve_chapter_href("OEBPS/ch1.xhtml", ""), "OEBPS/ch1.xhtml");
    }

    #[test]
    fn chapter_href_chapter_at_root() {
        assert_eq!(resolve_chapter_href("ch1.xhtml", "img/a.png"), "img/a.png");
    }
}
