use std::ffi::{CStr, CString};
use std::io::Cursor;
use std::os::raw::{c_char, c_uchar};
use std::ptr;

use crate::ffi::common::{EpubHandle, into_c_string, into_raw_bytes, to_json};
use crate::ffi_boundary;
use crate::parser::EpubArchive;

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
    ffi_boundary!(ptr::null_mut(), {
        if data.is_null() {
            return Err("epub_open: data pointer is null".into());
        }
        if len == 0 {
            return Err("epub_open: len is 0".into());
        }

        // SAFETY: Caller guarantees data points to `len` readable bytes.
        let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
        let cursor = Cursor::new(bytes);
        let archive = EpubArchive::new(cursor)?;
        Ok(Box::into_raw(Box::new(EpubHandle {
            archive,
            book: None,
        })))
    })
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
    ffi_boundary!(ptr::null_mut(), {
        if path.is_null() {
            return Err("epub_open_file: path pointer is null".into());
        }

        // SAFETY: Caller guarantees path is a valid null-terminated C string.
        let path_str = unsafe { CStr::from_ptr(path) }.to_string_lossy();
        let bytes = std::fs::read(path_str.as_ref())?;
        let cursor = Cursor::new(bytes);
        let archive = EpubArchive::new(cursor)?;
        Ok(Box::into_raw(Box::new(EpubHandle {
            archive,
            book: None,
        })))
    })
}

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
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_parse: null handle")?;
        h.ensure_parsed()?;
        Ok(to_json(h.book.as_ref().unwrap()))
    })
}

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
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_get_navigation: null handle")?;
        h.ensure_parsed()?;
        let book = h.book.as_ref().unwrap();
        let nav = h.archive.get_navigation(book)?;
        Ok(to_json(&nav))
    })
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
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_get_toc: null handle")?;
        h.ensure_parsed()?;
        let book = h.book.as_ref().unwrap();
        let toc = h.archive.get_toc(book)?;
        Ok(to_json(&toc))
    })
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
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_get_page_list: null handle")?;
        h.ensure_parsed()?;
        let book = h.book.as_ref().unwrap();
        let pages = h.archive.get_page_list(book)?;
        Ok(to_json(&pages))
    })
}

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
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_positions_by_reading_order: null handle")?;
        h.ensure_parsed()?;
        let book = h.book.as_ref().unwrap();
        let page_len = if bytes_per_position == 0 {
            crate::parser::BYTES_PER_POSITION
        } else {
            bytes_per_position
        };
        let strategy = crate::parser::positions::ArchiveEntryLength {
            page_length: page_len,
        };
        let positions = h.archive.positions_by_reading_order(book, &strategy)?;
        Ok(to_json(&positions))
    })
}

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
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_get_cover_image: null handle")?;
        if out_len.is_null() || out_media_type.is_null() {
            return Err("epub_get_cover_image: out_len and out_media_type must be non-null".into());
        }
        h.ensure_parsed()?;
        let book = h.book.as_ref().unwrap();
        let (bytes, media_type) = h.archive.get_cover_image(book)?;
        let media_type_ptr = CString::new(media_type)
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
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_get_resource: null handle")?;
        if href.is_null() || out_len.is_null() {
            return Err("epub_get_resource: href and out_len must be non-null".into());
        }
        h.ensure_parsed()?;
        let href_str = unsafe { CStr::from_ptr(href) }.to_string_lossy();
        let book = h.book.as_ref().unwrap();
        let bytes = h.archive.get_resource_by_href(book, href_str.as_ref())?;
        unsafe {
            *out_len = bytes.len();
        }
        Ok(into_raw_bytes(bytes))
    })
}

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
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_get_resource_by_id: null handle")?;
        if id.is_null() || out_len.is_null() {
            return Err("epub_get_resource_by_id: id and out_len must be non-null".into());
        }
        h.ensure_parsed()?;
        let id_str = unsafe { CStr::from_ptr(id) }.to_string_lossy();
        let book = h.book.as_ref().unwrap();
        let bytes = h.archive.get_resource_by_id(book, id_str.as_ref())?;
        unsafe {
            *out_len = bytes.len();
        }
        Ok(into_raw_bytes(bytes))
    })
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
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_get_chapter_with_cfi: null handle")?;
        if id.is_null() {
            return Err("epub_get_chapter_with_cfi: id is null".into());
        }
        h.ensure_parsed()?;
        let id_str = unsafe { CStr::from_ptr(id) }.to_string_lossy();
        let book = h.book.as_ref().unwrap();
        let html = h.archive.get_chapter_with_cfi(book, id_str.as_ref())?;
        Ok(into_c_string(html))
    })
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
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_search_chapter: null handle")?;
        if id.is_null() || query.is_null() {
            return Err("epub_search_chapter: id and query must be non-null".into());
        }
        h.ensure_parsed()?;
        let id_str = unsafe { CStr::from_ptr(id) }.to_string_lossy();
        let query_str = unsafe { CStr::from_ptr(query) }.to_string_lossy();
        let pattern = regex::Regex::new(&regex::escape(query_str.as_ref()))
            .map_err(|e| format!("epub_search_chapter: invalid query: {e}"))?;
        let book = h.book.as_ref().unwrap();
        let results = h.archive.search_chapter(book, id_str.as_ref(), &pattern)?;
        Ok(to_json(&results))
    })
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
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_get_semantic_content: null handle")?;
        if id.is_null() {
            return Err("epub_get_semantic_content: id is null".into());
        }
        h.ensure_parsed()?;
        let id_str = unsafe { CStr::from_ptr(id) }.to_string_lossy();
        let book = h.book.as_ref().unwrap();
        let content = h.archive.get_semantic_content(book, id_str.as_ref())?;
        Ok(to_json(&content))
    })
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
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_generate_locations: null handle")?;
        h.ensure_parsed()?;
        let bpp = if bytes_per_position == 0 {
            crate::parser::BYTES_PER_POSITION
        } else {
            bytes_per_position
        };
        let book = h.book.as_ref().unwrap();
        let positions = h.archive.generate_locations(book, bpp)?;
        Ok(to_json(&positions))
    })
}

/// Generate positions and build a bidirectional lookup index in one call.
///
/// Returns a flat JSON array of `Position` objects identical to
/// `epub_generate_locations`. The returned array can be passed as `positions_json`
/// to `epub_location_from_cfi` and `epub_cfi_from_location`.
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
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_generate_location_index: null handle")?;
        h.ensure_parsed()?;
        let book = h.book.as_ref().unwrap();
        let bpp = if bytes_per_position == 0 {
            crate::parser::BYTES_PER_POSITION
        } else {
            bytes_per_position
        };
        let strategy = crate::parser::ArchiveEntryLength { page_length: bpp };
        let by_chapter = h.archive.positions_by_reading_order(book, &strategy)?;
        let index = crate::parser::PositionIndex::build(by_chapter);
        let flat: Vec<&crate::model::Position> = (0..index.len())
            .filter_map(|i| index.position_at(i))
            .collect();
        Ok(to_json(&flat))
    })
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
/// # Safety
/// - `handle` must be a valid non-null pointer obtained from `epub_open*`.
/// - `positions_json` and `cfi_str` must be valid null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_location_from_cfi(
    handle: *mut EpubHandle,
    positions_json: *const c_char,
    cfi_str: *const c_char,
) -> *mut c_char {
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_location_from_cfi: null handle")?;
        if positions_json.is_null() || cfi_str.is_null() {
            return Err(
                "epub_location_from_cfi: positions_json and cfi_str must be non-null".into(),
            );
        }

        h.ensure_parsed()?;
        let book = h.book.as_ref().unwrap();
        let cfi_s = unsafe { CStr::from_ptr(cfi_str) }.to_string_lossy();

        let strategy = crate::parser::ArchiveEntryLength {
            page_length: crate::parser::BYTES_PER_POSITION,
        };
        let by_chapter = h.archive.positions_by_reading_order(book, &strategy)?;
        let index = crate::parser::PositionIndex::build(by_chapter);

        let res = match index.location_from_cfi(cfi_s.as_ref()) {
            Some(idx) => idx.to_string(),
            None => "-1".to_string(),
        };
        Ok(into_c_string(res))
    })
}

/// Return the CFI string for a given 0-based position index.
///
/// `positions_json`: the JSON array returned by `epub_generate_location_index`.
/// `idx`: 0-based position index (as returned by `epub_location_from_cfi`).
///
/// Returns the CFI string (e.g. `"epubcfi(/6/4!/4/2)"`), or `NULL` if `idx`
/// is out of range. The caller must free with `epub_free_string()`.
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
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_cfi_from_location: null handle")?;
        if positions_json.is_null() {
            return Err("epub_cfi_from_location: positions_json must be non-null".into());
        }

        h.ensure_parsed()?;
        let book = h.book.as_ref().unwrap();
        let strategy = crate::parser::ArchiveEntryLength {
            page_length: crate::parser::BYTES_PER_POSITION,
        };
        let by_chapter = h.archive.positions_by_reading_order(book, &strategy)?;
        let index = crate::parser::PositionIndex::build(by_chapter);

        match index.cfi_from_location(idx) {
            Some(cfi) => Ok(into_c_string(cfi.to_string())),
            None => Err(format!(
                "epub_cfi_from_location: index {idx} out of range (total={})",
                index.len()
            )
            .into()),
        }
    })
}
