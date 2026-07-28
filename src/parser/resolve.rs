//! Package resource resolution: canonical path + media type lookup.
//!
//! Reading systems need to turn an href found in a chapter / TOC entry into
//! bytes and a media type. Hosts historically re-implemented the probing
//! (chapter-relative join, OPF-dir prefix variants, fuzzy manifest suffix
//! matching) per-platform; single-sourcing it here keeps archive truth (ZIP
//! entries) and OPF truth (manifest hrefs / media-types) aligned, and lets the
//! tolerant provider lookup (case-insensitive, extension aliases) apply
//! uniformly.

use super::EpubArchive;
use crate::error::EpubError;
use crate::model::{EpubBook, TocEntry};
use crate::path::{is_external_url, join_epub_path, normalize_path};
use crate::provider::EpubProvider;

/// A resource located inside the package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedResource {
    /// Package-root-relative path suitable for provider reads (no leading `/`).
    pub zip_path: String,
    /// Manifest media type when the path matches an OPF item; otherwise a
    /// best-effort guess from the file extension.
    pub media_type: String,
}

/// Comparable forms of an href: package-root-relative and OPF-dir-stripped.
///
/// Both are fragment/query-stripped and percent-decoded. Navigation hrefs
/// produced by the engine are package-root-relative while manifest hrefs are
/// OPF-relative, so matching always considers both forms of both sides.
fn comparable_forms(book: &EpubBook, href: &str) -> [String; 2] {
    let clean = href.split(['?', '#']).next().unwrap_or(href);
    let decoded = percent_encoding::percent_decode_str(clean).decode_utf8_lossy();
    let full = decoded.trim_start_matches('/');
    let opf_prefix = if book.opf_dir.is_empty() {
        String::new()
    } else {
        format!("{}/", book.opf_dir.trim_matches('/'))
    };
    let stripped = if opf_prefix.is_empty() {
        full
    } else {
        full.strip_prefix(&opf_prefix).unwrap_or(full)
    };
    [full.to_string(), stripped.to_string()]
}

/// Exact normalized equality (either comparable form may match).
fn hrefs_equal(book: &EpubBook, a: &str, b: &str) -> bool {
    let [a_full, a_strip] = comparable_forms(book, a);
    let [b_full, b_strip] = comparable_forms(book, b);
    a_full == b_full || a_strip == b_strip || a_full == b_strip || a_strip == b_full
}

/// Fuzzy match for mis-authored hrefs: normalized equality, then suffix
/// rules, then last-resort filename equality.
///
/// Filename equality can be ambiguous when a package reuses file names across
/// directories; callers that need determinism should run an exact pass first
/// (see [`EpubArchive::toc_title_for_href`]).
fn hrefs_match(book: &EpubBook, a: &str, b: &str) -> bool {
    if hrefs_equal(book, a, b) {
        return true;
    }
    let [a_full, a_strip] = comparable_forms(book, a);
    let [b_full, b_strip] = comparable_forms(book, b);
    if a_full.ends_with(&format!("/{b_strip}")) || b_full.ends_with(&format!("/{a_strip}")) {
        return true;
    }
    let file_a = a_strip.rsplit('/').next().unwrap_or(&a_strip);
    let file_b = b_strip.rsplit('/').next().unwrap_or(&b_strip);
    !file_a.is_empty() && file_a == file_b
}

impl<P: EpubProvider> EpubArchive<P> {
    /// Manifest media type for a package path, preferring exact matches.
    fn manifest_media_type(book: &EpubBook, path: &str) -> Option<String> {
        if let Some(item) = book.manifest.values().find(|m| hrefs_equal(book, &m.href, path)) {
            return Some(item.media_type.clone());
        }
        book.manifest
            .values()
            .find(|m| hrefs_match(book, &m.href, path))
            .map(|m| m.media_type.clone())
    }

    /// Media type for an OPF-relative or package-relative path (no I/O).
    ///
    /// Manifest first; extension table as the floor (`application/octet-stream`
    /// when nothing matches), so `data:` URI construction always has a value.
    pub fn resource_media_type(&self, book: &EpubBook, path: &str) -> String {
        Self::manifest_media_type(book, path)
            .unwrap_or_else(|| crate::mime::mime_for_path(path).to_string())
    }

    /// Resolve `rel_path` (as referenced from `chapter_href`) to a package
    /// path that exists in the archive, plus its media type.
    ///
    /// Candidate order:
    /// 1. chapter-relative join (when `chapter_href` is given)
    /// 2. OPF-relative join (manifest href semantics)
    /// 3. package-root-relative
    /// 4. manifest suffix / filename match (mis-authored references)
    ///
    /// Returns `None` for external URLs, empty refs, and references with no
    /// matching archive entry or manifest item.
    pub fn resolve_resource_path(
        &mut self,
        book: &EpubBook,
        chapter_href: Option<&str>,
        rel_path: &str,
    ) -> Option<ResolvedResource> {
        if rel_path.is_empty() || is_external_url(rel_path) {
            return None;
        }
        let clean = rel_path.split(['?', '#']).next().unwrap_or(rel_path);
        let decoded = percent_encoding::percent_decode_str(clean).decode_utf8_lossy();
        let decoded = decoded.as_ref();
        let opf_dir = book.opf_dir.trim_matches('/');

        let mut candidates: Vec<String> = Vec::with_capacity(3);
        let mut push = |p: String| {
            let p = p.trim_start_matches('/').to_string();
            if !p.is_empty() && !candidates.contains(&p) {
                candidates.push(p);
            }
        };

        if let Some(root_abs) = decoded.strip_prefix('/') {
            // Root-absolute reference (`/OEBPS/images/a.jpg`).
            push(root_abs.to_string());
        } else {
            // 1. Chapter-relative (HTML href semantics: relative to the
            //    referencing document's own directory).
            if let Some(ch) = chapter_href {
                let ch_clean = ch.split(['?', '#']).next().unwrap_or(ch).trim_start_matches('/');
                let ch_pkg = if !opf_dir.is_empty() && !ch_clean.starts_with(&format!("{opf_dir}/"))
                {
                    join_epub_path(opf_dir, ch_clean)
                } else {
                    ch_clean.to_string()
                };
                let base = match ch_pkg.rfind('/') {
                    Some(i) => &ch_pkg[..i],
                    None => "",
                };
                push(normalize_path(base, decoded));
            }
            // 2. OPF-relative (manifest href semantics).
            push(normalize_path(opf_dir, decoded));
            // 3. Package-root-relative.
            push(decoded.to_string());
        }

        for cand in &candidates {
            if self.provider.entry_length(cand).is_ok() {
                let media_type = Self::manifest_media_type(book, cand)
                    .unwrap_or_else(|| crate::mime::mime_for_path(cand).to_string());
                return Some(ResolvedResource {
                    zip_path: cand.clone(),
                    media_type,
                });
            }
        }

        // 4. Manifest fuzzy match — the OPF lists the file, but none of the
        //    naive joins produced it (e.g. href spelled with different
        //    percent-encoding or directory casing than the ZIP entry).
        if let Some(item) = book
            .manifest
            .values()
            .find(|m| hrefs_match(book, &m.href, decoded))
        {
            return Some(ResolvedResource {
                zip_path: normalize_path(opf_dir, &item.href),
                media_type: item.media_type.clone(),
            });
        }
        None
    }

    /// Resolve + read bytes in one call (cache- and decryption-aware).
    pub fn get_resolved_resource(
        &mut self,
        book: &EpubBook,
        chapter_href: Option<&str>,
        rel_path: &str,
    ) -> Result<(Vec<u8>, String), EpubError> {
        let resolved = self
            .resolve_resource_path(book, chapter_href, rel_path)
            .ok_or_else(|| EpubError::InvalidFormat(format!("resource not found: {rel_path}")))?;
        let bytes = self.get_resource_by_zip_path(book, &resolved.zip_path)?;
        Ok((bytes, resolved.media_type))
    }

    /// TOC title whose href matches `href`, or `None`.
    ///
    /// Three passes for determinism:
    /// 0. verbatim equality — distinguishes fragment-addressed entries (a
    ///    nested section link into the same file as its chapter entry)
    /// 1. normalized equality (fragment/query-stripped, percent-decoded,
    ///    OPF-prefix-insensitive) across the whole tree
    /// 2. suffix / filename fallback for mis-authored hrefs
    pub fn toc_title_for_href(&mut self, book: &EpubBook, href: &str) -> Option<String> {
        let toc = self.get_toc(book).ok()?;
        if let Some(t) = Self::find_toc_title_verbatim(&toc, href) {
            return Some(t);
        }
        Self::find_toc_title(book, &toc, href, false)
            .or_else(|| Self::find_toc_title(book, &toc, href, true))
    }

    fn find_toc_title_verbatim(entries: &[TocEntry], href: &str) -> Option<String> {
        let wanted = href.trim();
        for entry in entries {
            if entry.href.trim() == wanted {
                return Some(entry.title.clone());
            }
            if let Some(t) = Self::find_toc_title_verbatim(&entry.children, href) {
                return Some(t);
            }
        }
        None
    }

    fn find_toc_title(book: &EpubBook, entries: &[TocEntry], href: &str, fuzzy: bool) -> Option<String> {
        for entry in entries {
            let matched = if fuzzy {
                hrefs_match(book, &entry.href, href)
            } else {
                hrefs_equal(book, &entry.href, href)
            };
            if matched {
                return Some(entry.title.clone());
            }
            if let Some(t) = Self::find_toc_title(book, &entry.children, href, fuzzy) {
                return Some(t);
            }
        }
        None
    }

    /// 0-based **absolute** spine index for a nav/TOC href, or `None`.
    ///
    /// Callers presenting a linear-only reading list must map the absolute
    /// index through their own spine filter (non-linear items return `None`
    /// from that mapping, matching legacy host behavior).
    pub fn spine_index_for_toc_href(&self, book: &EpubBook, href: &str) -> Option<usize> {
        super::reading::spine_index_for_href(book, href)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ManifestItem, SpineItem};
    use crate::provider::ZipProvider;
    use std::collections::HashMap;
    use std::io::{Cursor, Write};

    fn mock_zip() -> Vec<u8> {
        let mut buf = Vec::new();
        let mut writer = zip::ZipWriter::new(Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        writer.start_file("OEBPS/Text/ch1.xhtml", opts).unwrap();
        writer.write_all(b"<html><body><p>one</p></body></html>").unwrap();
        writer.start_file("OEBPS/Text/ch2.xhtml", opts).unwrap();
        writer.write_all(b"<html><body><p>two</p></body></html>").unwrap();
        writer.start_file("OEBPS/Images/cover.jpg", opts).unwrap();
        writer.write_all(b"fake-jpeg").unwrap();
        writer.start_file("OEBPS/Images/my pic.png", opts).unwrap();
        writer.write_all(b"fake-png").unwrap();
        writer.start_file("OEBPS/Styles/main.css", opts).unwrap();
        writer.write_all(b"body {}").unwrap();
        writer
            .start_file("OEBPS/nav.xhtml", opts)
            .unwrap();
        writer
            .write_all(
                br#"<html xmlns:epub="http://www.idpf.org/2007/ops"><body>
<nav epub:type="toc"><ol>
<li><a href="Text/ch1.xhtml">Chapter 1</a></li>
<li><a href="Text/ch2.xhtml">Chapter 2</a><ol>
<li><a href="Text/ch2.xhtml#s1">Section 1</a></li>
</ol></li>
</ol></nav>
</body></html>"#,
            )
            .unwrap();
        writer.finish().unwrap();
        buf
    }

    fn item(id: &str, href: &str, media_type: &str, properties: &[&str]) -> ManifestItem {
        ManifestItem {
            id: id.to_string(),
            href: href.to_string(),
            media_type: media_type.to_string(),
            properties: properties.iter().map(|s| s.to_string()).collect(),
            media_overlay: None,
        }
    }

    fn mock_book() -> EpubBook {
        let mut manifest = HashMap::new();
        for it in [
            item("ch1", "Text/ch1.xhtml", "application/xhtml+xml", &[]),
            item("ch2", "Text/ch2.xhtml", "application/xhtml+xml", &[]),
            item("img", "Images/cover.jpg", "image/jpeg", &[]),
            item("pic", "Images/my pic.png", "image/png", &[]),
            item("css", "Styles/main.css", "text/css", &[]),
            item("nav", "nav.xhtml", "application/xhtml+xml", &["nav"]),
        ] {
            manifest.insert(it.id.clone(), it);
        }
        EpubBook {
            manifest,
            spine: vec![SpineItem::new("ch1"), SpineItem::new("ch2")],
            opf_dir: "OEBPS".into(),
            ..Default::default()
        }
    }

    fn mock_archive() -> EpubArchive<ZipProvider<Cursor<Vec<u8>>>> {
        EpubArchive::new(Cursor::new(mock_zip())).unwrap()
    }

    // ── resolve_resource_path ────────────────────────────────────────────────

    #[test]
    fn resolves_chapter_relative() {
        let mut archive = mock_archive();
        let book = mock_book();
        let r = archive
            .resolve_resource_path(&book, Some("Text/ch1.xhtml"), "../Images/cover.jpg")
            .unwrap();
        assert_eq!(r.zip_path, "OEBPS/Images/cover.jpg");
        assert_eq!(r.media_type, "image/jpeg");
    }

    #[test]
    fn resolves_opf_relative_without_chapter() {
        let mut archive = mock_archive();
        let book = mock_book();
        let r = archive
            .resolve_resource_path(&book, None, "Images/cover.jpg")
            .unwrap();
        assert_eq!(r.zip_path, "OEBPS/Images/cover.jpg");
    }

    #[test]
    fn resolves_root_absolute() {
        let mut archive = mock_archive();
        let book = mock_book();
        let r = archive
            .resolve_resource_path(&book, Some("Text/ch1.xhtml"), "/OEBPS/Styles/main.css")
            .unwrap();
        assert_eq!(r.zip_path, "OEBPS/Styles/main.css");
        assert_eq!(r.media_type, "text/css");
    }

    #[test]
    fn strips_query_fragment_and_percent_decodes() {
        let mut archive = mock_archive();
        let book = mock_book();
        let r = archive
            .resolve_resource_path(&book, Some("Text/ch1.xhtml"), "../Images/my%20pic.png?v=2#frag")
            .unwrap();
        assert_eq!(r.zip_path, "OEBPS/Images/my pic.png");
        assert_eq!(r.media_type, "image/png");
    }

    #[test]
    fn manifest_fallback_when_join_misses() {
        let mut archive = mock_archive();
        let book = mock_book();
        // Only the filename matches; manifest suffix rule must find it.
        let r = archive
            .resolve_resource_path(&book, None, "cover.jpg")
            .unwrap();
        assert_eq!(r.zip_path, "OEBPS/Images/cover.jpg");
        assert_eq!(r.media_type, "image/jpeg");
    }

    #[test]
    fn rejects_external_and_missing() {
        let mut archive = mock_archive();
        let book = mock_book();
        assert!(
            archive
                .resolve_resource_path(&book, None, "https://example.com/a.png")
                .is_none()
        );
        assert!(archive.resolve_resource_path(&book, None, "nope/none.png").is_none());
    }

    #[test]
    fn get_resolved_resource_returns_bytes_and_type() {
        let mut archive = mock_archive();
        let book = mock_book();
        let (bytes, media_type) = archive
            .get_resolved_resource(&book, Some("Text/ch1.xhtml"), "../Images/cover.jpg")
            .unwrap();
        assert_eq!(bytes, b"fake-jpeg");
        assert_eq!(media_type, "image/jpeg");
    }

    // ── resource_media_type ──────────────────────────────────────────────────

    #[test]
    fn media_type_prefers_manifest() {
        let archive = mock_archive();
        let book = mock_book();
        assert_eq!(archive.resource_media_type(&book, "Images/cover.jpg"), "image/jpeg");
        assert_eq!(
            archive.resource_media_type(&book, "OEBPS/Images/cover.jpg"),
            "image/jpeg"
        );
        // Extension-table floor for unlisted / unknown paths.
        assert_eq!(
            archive.resource_media_type(&book, "anything/unknown.xyz"),
            "application/octet-stream"
        );
        assert_eq!(archive.resource_media_type(&book, "a/b/font.woff2"), "font/woff2");
    }

    // ── toc_title_for_href ───────────────────────────────────────────────────

    #[test]
    fn toc_title_matches_opf_relative_input() {
        let mut archive = mock_archive();
        let book = mock_book();
        // Manifest href form (OPF-relative) must match the engine's
        // package-root-relative TOC href.
        assert_eq!(
            archive.toc_title_for_href(&book, "Text/ch1.xhtml").as_deref(),
            Some("Chapter 1")
        );
    }

    #[test]
    fn toc_title_strips_fragment_and_walks_children() {
        let mut archive = mock_archive();
        let book = mock_book();
        assert_eq!(
            archive
                .toc_title_for_href(&book, "OEBPS/Text/ch2.xhtml#s1")
                .as_deref(),
            Some("Section 1")
        );
    }

    #[test]
    fn toc_title_none_for_unknown_href() {
        let mut archive = mock_archive();
        let book = mock_book();
        assert!(archive.toc_title_for_href(&book, "Nope/none.xhtml").is_none());
    }

    // ── spine_index_for_toc_href ─────────────────────────────────────────────

    #[test]
    fn spine_index_matches_nav_href() {
        let archive = mock_archive();
        let book = mock_book();
        assert_eq!(archive.spine_index_for_toc_href(&book, "OEBPS/Text/ch2.xhtml"), Some(1));
        assert_eq!(archive.spine_index_for_toc_href(&book, "Text/ch1.xhtml"), Some(0));
        assert_eq!(archive.spine_index_for_toc_href(&book, "none.xhtml"), None);
    }
}
