use super::common::make_epub_bytes;
use crate::ffi::common::{epub_free, epub_free_bytes, epub_free_string};
use crate::ffi::parser::epub_open;
use crate::ffi::path::*;
use std::ffi::{CStr, CString};
use std::ptr;

fn make_nav_epub_bytes() -> Vec<u8> {
    use crate::generator::EpubBuilder;
    use crate::model::Metadata;
    use std::io::Cursor;

    let metadata = Metadata {
        title: Some("FFI Nav Test".to_string()),
        language: Some("en".to_string()),
        identifier: Some("urn:uuid:ffi-nav-test".to_string()),
        ..Default::default()
    };
    let mut buf = Cursor::new(Vec::new());
    EpubBuilder::new()
        .metadata(metadata)
        .add_chapter_with_nav(
            "ch1",
            "text/ch1.xhtml",
            "Chapter One",
            b"<html><body><p>Hello nav</p></body></html>".to_vec(),
        )
        .generate(&mut buf)
        .expect("test EPUB generation failed");
    buf.into_inner()
}

unsafe fn read_and_free(s: *mut std::os::raw::c_char) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let out = unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned();
    unsafe { epub_free_string(s) };
    Some(out)
}

#[test]
fn normalize_path_pure() {
    let base = CString::new("OEBPS/Text").unwrap();
    let rel = CString::new("../Images/a.jpg").unwrap();
    let out = unsafe { epub_normalize_path(base.as_ptr(), rel.as_ptr()) };
    assert_eq!(
        unsafe { read_and_free(out) }.as_deref(),
        Some("OEBPS/Images/a.jpg")
    );
}

#[test]
fn resolve_chapter_href_pure() {
    let ch = CString::new("OEBPS/Text/ch1.xhtml").unwrap();
    let rel = CString::new("../Images/a.jpg").unwrap();
    let out = unsafe { epub_resolve_chapter_href(ch.as_ptr(), rel.as_ptr()) };
    assert_eq!(
        unsafe { read_and_free(out) }.as_deref(),
        Some("OEBPS/Images/a.jpg")
    );

    let ext = CString::new("https://example.com/a.png").unwrap();
    let out = unsafe { epub_resolve_chapter_href(ch.as_ptr(), ext.as_ptr()) };
    assert_eq!(
        unsafe { read_and_free(out) }.as_deref(),
        Some("https://example.com/a.png")
    );
}

#[test]
fn mime_for_path_pure() {
    let p = CString::new("img/pic.PNG").unwrap();
    let out = unsafe { epub_mime_for_path(p.as_ptr()) };
    assert_eq!(unsafe { read_and_free(out) }.as_deref(), Some("image/png"));
}

#[test]
fn resolve_resource_path_and_media_type() {
    let bytes = make_epub_bytes();
    let handle = unsafe { epub_open(bytes.as_ptr(), bytes.len()) };
    assert!(!handle.is_null());

    let rel = CString::new("text/ch1.xhtml").unwrap();
    let out = unsafe { epub_resolve_resource_path(handle, ptr::null(), rel.as_ptr()) };
    assert_eq!(
        unsafe { read_and_free(out) }.as_deref(),
        Some("OEBPS/text/ch1.xhtml")
    );

    let mt = unsafe { epub_resource_media_type(handle, rel.as_ptr()) };
    assert_eq!(
        unsafe { read_and_free(mt) }.as_deref(),
        Some("application/xhtml+xml")
    );

    // Miss returns NULL without an error message.
    let miss = CString::new("nope/none.png").unwrap();
    let out = unsafe { epub_resolve_resource_path(handle, ptr::null(), miss.as_ptr()) };
    assert!(out.is_null());

    unsafe { epub_free(handle) };
}

#[test]
fn get_resolved_resource_bytes() {
    let bytes = make_epub_bytes();
    let handle = unsafe { epub_open(bytes.as_ptr(), bytes.len()) };
    assert!(!handle.is_null());

    let rel = CString::new("text/ch1.xhtml").unwrap();
    let mut len: usize = 0;
    let mut mt: *mut std::os::raw::c_char = ptr::null_mut();
    let data =
        unsafe { epub_get_resolved_resource(handle, ptr::null(), rel.as_ptr(), &mut len, &mut mt) };
    assert!(!data.is_null());
    let body = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    assert!(String::from_utf8_lossy(&body).contains("Hello FFI"));
    unsafe {
        epub_free_bytes(data, len);
        assert_eq!(
            CStr::from_ptr(mt).to_string_lossy().as_ref(),
            "application/xhtml+xml"
        );
        epub_free_string(mt);
        epub_free(handle);
    }
}

#[test]
fn spine_index_for_href_ffi() {
    let bytes = make_epub_bytes();
    let handle = unsafe { epub_open(bytes.as_ptr(), bytes.len()) };
    assert!(!handle.is_null());

    let opf_rel = CString::new("text/ch1.xhtml").unwrap();
    assert_eq!(
        unsafe { epub_spine_index_for_href(handle, opf_rel.as_ptr()) },
        0
    );

    let root_rel = CString::new("OEBPS/text/ch1.xhtml").unwrap();
    assert_eq!(
        unsafe { epub_spine_index_for_href(handle, root_rel.as_ptr()) },
        0
    );

    let miss = CString::new("nope.xhtml").unwrap();
    assert_eq!(
        unsafe { epub_spine_index_for_href(handle, miss.as_ptr()) },
        -1
    );

    unsafe { epub_free(handle) };
}

#[test]
fn toc_title_for_href_ffi() {
    let bytes = make_nav_epub_bytes();
    let handle = unsafe { epub_open(bytes.as_ptr(), bytes.len()) };
    assert!(!handle.is_null());

    let opf_rel = CString::new("text/ch1.xhtml").unwrap();
    let out = unsafe { epub_toc_title_for_href(handle, opf_rel.as_ptr()) };
    assert_eq!(
        unsafe { read_and_free(out) }.as_deref(),
        Some("Chapter One")
    );

    let miss = CString::new("nope.xhtml").unwrap();
    let out = unsafe { epub_toc_title_for_href(handle, miss.as_ptr()) };
    assert!(out.is_null());

    unsafe { epub_free(handle) };
}

#[test]
fn null_arguments_are_safe() {
    assert!(unsafe { epub_normalize_path(ptr::null(), ptr::null()) }.is_null());
    assert!(unsafe { epub_resolve_chapter_href(ptr::null(), ptr::null()) }.is_null());
    assert!(unsafe { epub_mime_for_path(ptr::null()) }.is_null());
    assert!(
        unsafe { epub_resolve_resource_path(ptr::null_mut(), ptr::null(), ptr::null()) }.is_null()
    );
    assert!(unsafe { epub_resource_media_type(ptr::null_mut(), ptr::null()) }.is_null());
    assert_eq!(
        unsafe { epub_spine_index_for_href(ptr::null_mut(), ptr::null()) },
        -1
    );
    assert!(unsafe { epub_toc_title_for_href(ptr::null_mut(), ptr::null()) }.is_null());
    let mut len: usize = 0;
    let mut mt: *mut std::os::raw::c_char = ptr::null_mut();
    assert!(
        unsafe {
            epub_get_resolved_resource(ptr::null_mut(), ptr::null(), ptr::null(), &mut len, &mut mt)
        }
        .is_null()
    );
}
