use std::ffi::CStr;
use std::os::raw::{c_char, c_uchar};
use std::ptr;

use crate::ffi::common::{
    clear_error, into_raw_bytes, set_error, EpubGeneratorHandle,
};

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
