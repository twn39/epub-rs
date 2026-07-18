use std::ffi::{CStr, CString};
use std::io::Cursor;
use std::os::raw::{c_char, c_int, c_uchar};
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
            book: Default::default(),
            position_index: None,
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
            book: Default::default(),
            position_index: None,
        })))
    })
}

/// Parse the EPUB and return book metadata as a JSON string.
///
/// After the first call the result is cached; subsequent calls are cheap.
/// Uses the **default** rendition (first `rootfile`). The caller must free the
/// returned string with `epub_free_string()`.
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
        Ok(to_json(
            h.book.as_book().expect("book ready after ensure_parsed"),
        ))
    })
}

/// Return all renditions from `META-INF/container.xml` as a JSON array.
///
/// Index 0 is always the default rendition. Each element is a `RenditionInfo`
/// (`opf_path`, optional `layout`, `language`, `label`, …).
///
/// The caller must free the returned string with `epub_free_string()`.
///
/// # Safety
/// `handle` must be a valid non-null pointer obtained from `epub_open*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_get_renditions(handle: *mut EpubHandle) -> *mut c_char {
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_get_renditions: null handle")?;
        let renditions = h.archive.get_renditions()?;
        Ok(to_json(&renditions))
    })
}

/// Parse a specific rendition by 0-based index and return `EpubBook` JSON.
///
/// Replaces any previously cached OPF and position index on the handle.
///
/// # Safety
/// `handle` must be a valid non-null pointer obtained from `epub_open*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_parse_by_index(handle: *mut EpubHandle, index: usize) -> *mut c_char {
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_parse_by_index: null handle")?;
        let result = h.archive.parse_by_index(index).map_err(|e| e.to_string());
        h.store_parsed_book(result);
        h.ensure_parsed()?;
        Ok(to_json(
            h.book.as_book().expect("book ready after ensure_parsed"),
        ))
    })
}

/// Parse the best-matching rendition for layout / language preferences.
///
/// `layout` / `language` may be `NULL` or empty to ignore that criterion.
/// Layout match outweighs language (same scoring as the native API).
/// Replaces any previously cached OPF and position index.
///
/// # Safety
/// - `handle` must be a valid non-null pointer from `epub_open*`.
/// - `layout` and `language` must be null or valid C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_parse_best_for(
    handle: *mut EpubHandle,
    layout: *const c_char,
    language: *const c_char,
) -> *mut c_char {
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_parse_best_for: null handle")?;
        let layout = if layout.is_null() {
            None
        } else {
            let s = unsafe { CStr::from_ptr(layout) }.to_string_lossy();
            if s.is_empty() {
                None
            } else {
                Some(s.into_owned())
            }
        };
        let language = if language.is_null() {
            None
        } else {
            let s = unsafe { CStr::from_ptr(language) }.to_string_lossy();
            if s.is_empty() {
                None
            } else {
                Some(s.into_owned())
            }
        };
        let result = h
            .archive
            .parse_best_for(layout.as_deref(), language.as_deref())
            .map_err(|e| e.to_string());
        h.store_parsed_book(result);
        h.ensure_parsed()?;
        Ok(to_json(
            h.book.as_book().expect("book ready after ensure_parsed"),
        ))
    })
}

/// Returns `1` if any manifest item has a media overlay, else `0`.
///
/// # Safety
/// `handle` must be a valid non-null pointer obtained from `epub_open*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_has_media_overlays(handle: *mut EpubHandle) -> c_int {
    ffi_boundary!(0, {
        let h = unsafe { handle.as_mut() }.ok_or("epub_has_media_overlays: null handle")?;
        h.ensure_parsed()?;
        let book = h.book.as_book().expect("book ready after ensure_parsed");
        Ok(if h.archive.has_media_overlays(book) {
            1
        } else {
            0
        })
    })
}

/// Return SMIL media overlay JSON for a content document href, or `"null"`.
///
/// `content_href` is the manifest href (EPUB-root-relative path as in the book model).
/// The caller must free the returned string with `epub_free_string()`.
///
/// # Safety
/// - `handle` must be a valid non-null pointer from `epub_open*`.
/// - `content_href` must be a valid null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_get_media_overlay(
    handle: *mut EpubHandle,
    content_href: *const c_char,
) -> *mut c_char {
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_get_media_overlay: null handle")?;
        if content_href.is_null() {
            return Err("epub_get_media_overlay: content_href is null".into());
        }
        h.ensure_parsed()?;
        let href = unsafe { CStr::from_ptr(content_href) }.to_string_lossy();
        let book = h.book.as_book().expect("book ready after ensure_parsed");
        let doc = h.archive.get_media_overlay(book, href.as_ref())?;
        Ok(to_json(&doc))
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
        let book = h.book.as_book().expect("book ready after ensure_parsed");
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
        let book = h.book.as_book().expect("book ready after ensure_parsed");
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
        let book = h.book.as_book().expect("book ready after ensure_parsed");
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
        let book = h.book.as_book().expect("book ready after ensure_parsed");
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
        let book = h.book.as_book().expect("book ready after ensure_parsed");
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
        let book = h.book.as_book().expect("book ready after ensure_parsed");
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
        let book = h.book.as_book().expect("book ready after ensure_parsed");
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
        let book = h.book.as_book().expect("book ready after ensure_parsed");
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
        let book = h.book.as_book().expect("book ready after ensure_parsed");
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
        let book = h.book.as_book().expect("book ready after ensure_parsed");
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
        h.ensure_index_built(bytes_per_position)?;
        let index = h.position_index.as_ref().unwrap();
        let flat: Vec<&crate::model::Position> = (0..index.len())
            .filter_map(|i| index.position_at(i))
            .collect();
        Ok(to_json(&flat))
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
        h.ensure_index_built(bytes_per_position)?;
        let index = h.position_index.as_ref().unwrap();
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

        h.ensure_index_built(0)?;
        let cfi_s = unsafe { CStr::from_ptr(cfi_str) }.to_string_lossy();

        let index = h.position_index.as_ref().unwrap();
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

        h.ensure_index_built(0)?;
        let index = h.position_index.as_ref().unwrap();

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

/// Find the 0-based position index that contains a given CFI (fast path).
///
/// Bypasses JSON serialization/deserialization and allocation for the return value.
/// Returns the 0-based index directly, or `-1` if the CFI could not be resolved.
///
/// # Safety
/// - `handle` must be a valid non-null pointer obtained from `epub_open*`.
/// - `cfi_str` must be a valid null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_location_from_cfi_fast(
    handle: *mut EpubHandle,
    cfi_str: *const c_char,
) -> isize {
    ffi_boundary!(-1, {
        let h = unsafe { handle.as_mut() }.ok_or("epub_location_from_cfi_fast: null handle")?;
        if cfi_str.is_null() {
            return Err("epub_location_from_cfi_fast: cfi_str must be non-null".into());
        }

        h.ensure_index_built(0)?;
        let cfi_s = unsafe { CStr::from_ptr(cfi_str) }.to_string_lossy();
        let index = h.position_index.as_ref().unwrap();

        let res = match index.location_from_cfi(cfi_s.as_ref()) {
            Some(idx) => idx as isize,
            None => -1,
        };
        Ok(res)
    })
}

/// Return the CFI string for a given 0-based position index (fast path).
///
/// Bypasses the redundant `positions_json` parameter. The caller must free
/// the returned C-string with `epub_free_string()`.
///
/// Returns `NULL` if `idx` is out of range.
///
/// # Safety
/// `handle` must be a valid non-null pointer obtained from `epub_open*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_cfi_from_location_fast(
    handle: *mut EpubHandle,
    idx: usize,
) -> *mut c_char {
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_cfi_from_location_fast: null handle")?;
        h.ensure_index_built(0)?;
        let index = h.position_index.as_ref().unwrap();

        match index.cfi_from_location(idx) {
            Some(cfi) => Ok(into_c_string(cfi.to_string())),
            None => Err(format!(
                "epub_cfi_from_location_fast: index {idx} out of range (total={})",
                index.len()
            )
            .into()),
        }
    })
}

/// Query the metrics of a virtual position at a given 0-based index.
///
/// Direct primitive getters avoiding JSON serialization. Writes the inner values
/// of `Position` (spine_index, chapter_progression, total_progression) into the
/// provided output pointers.
///
/// Returns `1` on success, `0` on failure (e.g. index out of range or null pointers).
///
/// # Safety
/// - `handle` must be a valid non-null pointer obtained from `epub_open*`.
/// - `out_spine_index`, `out_chapter_progression`, and `out_total_progression`
///   must be valid, non-null writable pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_get_position_info(
    handle: *mut EpubHandle,
    idx: usize,
    out_spine_index: *mut usize,
    out_chapter_progression: *mut f32,
    out_total_progression: *mut f32,
) -> c_int {
    ffi_boundary!(0, {
        let h = unsafe { handle.as_mut() }.ok_or("epub_get_position_info: null handle")?;
        if out_spine_index.is_null()
            || out_chapter_progression.is_null()
            || out_total_progression.is_null()
        {
            return Err("epub_get_position_info: output pointers must be non-null".into());
        }

        h.ensure_index_built(0)?;
        let index = h.position_index.as_ref().unwrap();
        if let Some(pos) = index.position_at(idx) {
            unsafe {
                *out_spine_index = pos.spine_index;
                *out_chapter_progression = pos.chapter_progression;
                *out_total_progression = pos.total_progression;
            }
            Ok(1)
        } else {
            Err(format!(
                "epub_get_position_info: index {idx} out of range (total={})",
                index.len()
            )
            .into())
        }
    })
}

/// Prepare a chapter for WebView embedding (optional CFI + resource inlining).
///
/// `options_json` may be `NULL` or empty for defaults (`inline_resources=true`,
/// `inject_cfi=false`). Example:
/// ```json
/// {"inject_cfi":true,"inline_resources":true,"max_inline_bytes":4194304}
/// ```
///
/// The caller must free the returned string with `epub_free_string()`.
///
/// # Safety
/// - `handle` must be a valid non-null pointer from `epub_open*`.
/// - `id` must be a valid null-terminated C string.
/// - `options_json` may be null or a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_prepare_chapter(
    handle: *mut EpubHandle,
    id: *const c_char,
    options_json: *const c_char,
) -> *mut c_char {
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_prepare_chapter: null handle")?;
        if id.is_null() {
            return Err("epub_prepare_chapter: id is null".into());
        }
        h.ensure_parsed()?;
        let id_str = unsafe { CStr::from_ptr(id) }.to_string_lossy();
        let options = if options_json.is_null() {
            crate::processor::PrepareChapterOptions::default()
        } else {
            let raw = unsafe { CStr::from_ptr(options_json) }.to_string_lossy();
            if raw.trim().is_empty() {
                crate::processor::PrepareChapterOptions::default()
            } else {
                serde_json::from_str(raw.as_ref())
                    .map_err(|e| format!("epub_prepare_chapter: invalid options JSON: {e}"))?
            }
        };
        let book = h.book.as_book().expect("book ready after ensure_parsed");
        let html = h.archive.prepare_chapter(book, id_str.as_ref(), &options)?;
        Ok(into_c_string(html))
    })
}

/// Search the entire book for a literal query. Returns JSON `BookSearchHit[]`.
///
/// `max_per_chapter` / `max_total`: pass `0` for defaults (12 / 80).
///
/// # Safety
/// All pointer parameters must be valid; `handle` from `epub_open*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_search_book(
    handle: *mut EpubHandle,
    query: *const c_char,
    max_per_chapter: usize,
    max_total: usize,
) -> *mut c_char {
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_search_book: null handle")?;
        if query.is_null() {
            return Err("epub_search_book: query is null".into());
        }
        h.ensure_parsed()?;
        let q = unsafe { CStr::from_ptr(query) }.to_string_lossy();
        let per = if max_per_chapter == 0 {
            12
        } else {
            max_per_chapter
        };
        let total = if max_total == 0 { 80 } else { max_total };
        let book = h.book.as_book().expect("book ready after ensure_parsed");
        let hits = h.archive.search_book(book, q.as_ref(), per, total)?;
        Ok(to_json(&hits))
    })
}

/// Preferred first-open spine index as JSON `ReadingStartInfo`.
///
/// Example: `{"spine_index":1,"source":"cover_skip","href":"OEBPS/ch1.xhtml"}`.
///
/// # Safety
/// `handle` must be a valid non-null pointer from `epub_open*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epub_preferred_reading_start(handle: *mut EpubHandle) -> *mut c_char {
    ffi_boundary!(ptr::null_mut(), {
        let h = unsafe { handle.as_mut() }.ok_or("epub_preferred_reading_start: null handle")?;
        h.ensure_parsed()?;
        let book = h.book.as_book().expect("book ready after ensure_parsed");
        let info = h.archive.preferred_reading_start(book);
        Ok(to_json(&info))
    })
}
