//! Reading-system orchestration on [`super::EpubArchive`].
//!
//! Keeps chapter prepare / full-text search / first-open heuristics out of
//! `mod.rs` so the archive surface stays navigable as reader APIs grow.
//!
//! Pure transforms live in [`crate::processor`]; this module only wires I/O
//! (resource load, including font deobfuscation via `get_resource_*`) and book
//! context.

use super::EpubArchive;
use crate::error::EpubError;
use crate::model::EpubBook;
use crate::processor::{LoadedResource, PrepareChapterOptions};
use crate::provider::EpubProvider;

/// One full-text search hit across the book (spine + CFI).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct BookSearchHit {
    pub spine_index: usize,
    pub manifest_id: String,
    pub excerpt: String,
    pub cfi: String,
}

/// Where a reader should open on first launch.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ReadingStartInfo {
    pub spine_index: usize,
    /// `landmark:bodymatter` | `landmark:text` | `cover_skip` | `spine_zero`
    pub source: String,
    pub href: Option<String>,
}

/// Options for [`EpubArchive::search_book_with_options`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SearchBookOptions {
    /// Cap hits collected from a single spine item. Default 12.
    #[serde(default = "default_max_per_chapter")]
    pub max_per_chapter: usize,
    /// Cap total hits across the book. Default 80.
    #[serde(default = "default_max_total")]
    pub max_total: usize,
    /// Case-insensitive match (Unicode-aware via the `regex` crate). Default true.
    #[serde(default = "default_true")]
    pub case_insensitive: bool,
    /// Include spine items with `linear="no"`. Default false.
    #[serde(default)]
    pub include_non_linear: bool,
}

fn default_max_per_chapter() -> usize {
    12
}
fn default_max_total() -> usize {
    80
}
fn default_true() -> bool {
    true
}

impl Default for SearchBookOptions {
    fn default() -> Self {
        Self {
            max_per_chapter: default_max_per_chapter(),
            max_total: default_max_total(),
            case_insensitive: true,
            include_non_linear: false,
        }
    }
}

fn strip_href_fragment(href: &str) -> &str {
    href.split('#').next().unwrap_or(href)
}

pub(crate) fn spine_index_for_href(book: &EpubBook, href: &str) -> Option<usize> {
    let clean = strip_href_fragment(href);
    let file_name = clean.rsplit('/').next().unwrap_or(clean);
    book.spine.iter().position(|s| {
        let Some(item) = book.manifest.get(&s.idref) else {
            return false;
        };
        let mh = strip_href_fragment(&item.href);
        mh == clean
            || mh.ends_with(clean)
            || clean.ends_with(mh)
            || mh.rsplit('/').next() == Some(file_name)
    })
}

fn looks_like_cover_spine(book: &EpubBook, spine_item: &crate::model::SpineItem) -> bool {
    // EPUB 2 meta name="cover" / stored cover_id → skip that spine item when opening.
    if book
        .metadata
        .cover_id
        .as_ref()
        .is_some_and(|cid| cid == &spine_item.idref)
    {
        return true;
    }
    let id_lower = spine_item.idref.to_ascii_lowercase();
    if id_lower.contains("cover") || id_lower.contains("titlepage") || id_lower == "title" {
        return true;
    }
    if let Some(item) = book.manifest.get(&spine_item.idref) {
        let href_lower = item.href.to_ascii_lowercase();
        if href_lower.contains("cover") || href_lower.contains("titlepage") {
            return true;
        }
        // cover-image is usually on the image, not XHTML — still skip if present.
        if item
            .properties
            .iter()
            .any(|p| p == "cover-image" || p.split_whitespace().any(|t| t == "cover-image"))
        {
            return true;
        }
    }
    false
}

fn landmark_matches_role(role_attr: &str, wanted: &str) -> bool {
    role_attr.split_whitespace().any(|t| {
        let t = t.trim_start_matches("epub:");
        t == wanted || t.ends_with(&format!(":{wanted}"))
    })
}

fn match_manifest_item<'a>(
    book: &'a EpubBook,
    path: &str,
) -> Option<&'a crate::model::ManifestItem> {
    book.manifest
        .values()
        .find(|m| m.href == path || m.href.ends_with(path) || path.ends_with(&m.href))
}

impl<P: EpubProvider> EpubArchive<P> {
    /// Prepare chapter HTML for embedding in a WebView-style reader.
    ///
    /// Optionally injects `data-cfi` and rewrites local images / fonts / CSS to
    /// `data:` URIs so the document does not need a custom URL scheme.
    ///
    /// Resource loads go through [`Self::get_resource_by_href`] /
    /// [`Self::get_resource_by_id`], so **IDPF/Adobe font obfuscation is already
    /// reversed** when `encryption.xml` lists the font. Full AES/LCP content
    /// decryption is not performed here (same as other resource APIs).
    pub fn prepare_chapter(
        &mut self,
        book: &EpubBook,
        id: &str,
        options: &PrepareChapterOptions,
    ) -> Result<String, EpubError> {
        let spine_index = book
            .spine
            .iter()
            .position(|s| s.idref == id)
            .ok_or_else(|| EpubError::InvalidFormat(format!("ID {} not found in spine", id)))?;
        let item = book
            .manifest
            .get(id)
            .ok_or_else(|| EpubError::InvalidFormat(format!("manifest id {id} not found")))?;
        let chapter_path = item.href.clone();
        let base_cfi = crate::cfi::EpubCfi::generate_spine_base_cfi(spine_index, id);
        let raw = self.get_resource_by_id(book, id)?;
        let raw_html = String::from_utf8_lossy(&raw).into_owned();

        let mut load = |path: &str| -> Option<LoadedResource> {
            if let Ok(bytes) = self.get_resource_by_href(book, path) {
                let media_type = match_manifest_item(book, path).map(|m| m.media_type.clone());
                return Some(LoadedResource { bytes, media_type });
            }
            if let Some(m) = match_manifest_item(book, path)
                && let Ok(bytes) = self.get_resource_by_id(book, &m.id)
            {
                return Some(LoadedResource {
                    bytes,
                    media_type: Some(m.media_type.clone()),
                });
            }
            None
        };

        let (html, _stats) = crate::processor::prepare_chapter_html_with_stats(
            &raw_html,
            &chapter_path,
            Some(&base_cfi),
            options,
            &mut load,
        )?;
        Ok(html)
    }

    /// Search every linear spine chapter for a literal query.
    ///
    /// Convenience wrapper around [`Self::search_book_with_options`] with
    /// explicit caps (case-insensitive, linear-only).
    pub fn search_book(
        &mut self,
        book: &EpubBook,
        query: &str,
        max_per_chapter: usize,
        max_total: usize,
    ) -> Result<Vec<BookSearchHit>, EpubError> {
        self.search_book_with_options(
            book,
            query,
            &SearchBookOptions {
                max_per_chapter,
                max_total,
                ..SearchBookOptions::default()
            },
        )
    }

    /// Full-book literal search with explicit options.
    ///
    /// Best-effort: chapters that fail to load are skipped.
    pub fn search_book_with_options(
        &mut self,
        book: &EpubBook,
        query: &str,
        options: &SearchBookOptions,
    ) -> Result<Vec<BookSearchHit>, EpubError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let escaped = regex::escape(trimmed);
        let pattern = regex::RegexBuilder::new(&escaped)
            .case_insensitive(options.case_insensitive)
            .build()
            .map_err(|e| EpubError::InvalidFormat(format!("invalid search query: {e}")))?;

        let max_per = if options.max_per_chapter == 0 {
            default_max_per_chapter()
        } else {
            options.max_per_chapter
        };
        let max_total = if options.max_total == 0 {
            default_max_total()
        } else {
            options.max_total
        };

        let mut hits = Vec::new();
        for (spine_index, spine_item) in book.spine.iter().enumerate() {
            if !options.include_non_linear && !spine_item.linear {
                continue;
            }
            if hits.len() >= max_total {
                break;
            }
            let id = &spine_item.idref;
            let chapter_hits = match self.search_chapter(book, id, &pattern) {
                Ok(h) => h,
                Err(_) => continue,
            };
            for r in chapter_hits.into_iter().take(max_per) {
                hits.push(BookSearchHit {
                    spine_index,
                    manifest_id: id.clone(),
                    excerpt: r.excerpt,
                    cfi: r.cfi,
                });
                if hits.len() >= max_total {
                    break;
                }
            }
        }
        Ok(hits)
    }

    /// Preferred first-open spine index.
    ///
    /// Order:
    /// 1. EPUB 3 landmarks (`bodymatter`, `text`, `chapter`, `body`)
    /// 2. EPUB 2 OPF `<guide>` (`text`, then `start` if present)
    /// 3. First linear spine item that does not look like cover/title
    ///    (including `metadata.cover_id` match)
    /// 4. Spine index 0 fallback
    pub fn preferred_reading_start(&mut self, book: &EpubBook) -> ReadingStartInfo {
        // 1. EPUB 3 landmarks (prefer body matter / main text)
        if let Ok(nav) = self.get_navigation(book) {
            for role in ["bodymatter", "text", "chapter", "body"] {
                if let Some(entry) = nav.landmarks.iter().find(|e| {
                    e.role
                        .as_ref()
                        .map(|r| landmark_matches_role(r, role))
                        .unwrap_or(false)
                }) && let Some(idx) = spine_index_for_href(book, &entry.href)
                {
                    return ReadingStartInfo {
                        spine_index: idx,
                        source: format!("landmark:{role}"),
                        href: Some(entry.href.clone()),
                    };
                }
            }
        }

        // 2. EPUB 2 guide references (type="text" is the main content start).
        for wanted in ["text", "start"] {
            if let Some(g) = book
                .guide
                .iter()
                .find(|g| g.ref_type.eq_ignore_ascii_case(wanted))
                && let Some(idx) = spine_index_for_href(book, &g.href)
            {
                return ReadingStartInfo {
                    spine_index: idx,
                    source: format!("guide:{wanted}"),
                    href: Some(g.href.clone()),
                };
            }
        }

        // 3. Skip leading cover-like spine items (id/href/properties/cover_id).
        for (idx, spine_item) in book.spine.iter().enumerate() {
            if !spine_item.linear {
                continue;
            }
            if looks_like_cover_spine(book, spine_item) {
                continue;
            }
            let href = book.manifest.get(&spine_item.idref).map(|m| m.href.clone());
            return ReadingStartInfo {
                spine_index: idx,
                source: if idx == 0 {
                    "spine_zero".into()
                } else {
                    "cover_skip".into()
                },
                href,
            };
        }

        // 4. Absolute fallback (may still be cover if the book only has one item).
        ReadingStartInfo {
            spine_index: 0,
            source: "spine_zero".into(),
            href: book
                .spine
                .first()
                .and_then(|s| book.manifest.get(&s.idref))
                .map(|m| m.href.clone()),
        }
    }

    /// Like [`Self::prepare_chapter`] but also returns inlining statistics.
    pub fn prepare_chapter_with_stats(
        &mut self,
        book: &EpubBook,
        id: &str,
        options: &PrepareChapterOptions,
    ) -> Result<(String, crate::processor::PrepareStats), EpubError> {
        let spine_index = book
            .spine
            .iter()
            .position(|s| s.idref == id)
            .ok_or_else(|| EpubError::InvalidFormat(format!("ID {} not found in spine", id)))?;
        let item = book
            .manifest
            .get(id)
            .ok_or_else(|| EpubError::InvalidFormat(format!("manifest id {id} not found")))?;
        let chapter_path = item.href.clone();
        let base_cfi = crate::cfi::EpubCfi::generate_spine_base_cfi(spine_index, id);
        let raw = self.get_resource_by_id(book, id)?;
        let raw_html = String::from_utf8_lossy(&raw).into_owned();

        let mut load = |path: &str| -> Option<LoadedResource> {
            if let Ok(bytes) = self.get_resource_by_href(book, path) {
                let media_type = match_manifest_item(book, path).map(|m| m.media_type.clone());
                return Some(LoadedResource { bytes, media_type });
            }
            if let Some(m) = match_manifest_item(book, path)
                && let Ok(bytes) = self.get_resource_by_id(book, &m.id)
            {
                return Some(LoadedResource {
                    bytes,
                    media_type: Some(m.media_type.clone()),
                });
            }
            None
        };

        crate::processor::prepare_chapter_html_with_stats(
            &raw_html,
            &chapter_path,
            Some(&base_cfi),
            options,
            &mut load,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ManifestItem, Metadata, SpineItem};
    use std::collections::HashMap;

    fn book_with_cover_then_text() -> EpubBook {
        let mut manifest = HashMap::new();
        manifest.insert(
            "cover".into(),
            ManifestItem {
                id: "cover".into(),
                href: "cover.xhtml".into(),
                media_type: "application/xhtml+xml".into(),
                properties: vec![],
                media_overlay: None,
            },
        );
        manifest.insert(
            "c1".into(),
            ManifestItem {
                id: "c1".into(),
                href: "ch1.xhtml".into(),
                media_type: "application/xhtml+xml".into(),
                properties: vec![],
                media_overlay: None,
            },
        );
        EpubBook {
            metadata: Metadata::default(),
            manifest,
            spine: vec![
                SpineItem {
                    idref: "cover".into(),
                    linear: true,
                    layout_override: None,
                    page_spread: None,
                },
                SpineItem {
                    idref: "c1".into(),
                    linear: true,
                    layout_override: None,
                    page_spread: None,
                },
            ],
            opf_dir: String::new(),
            toc_id: None,
            guide: Vec::new(),
            encryptions: HashMap::new(),
        }
    }

    #[test]
    fn cover_heuristic_matches_id() {
        let book = book_with_cover_then_text();
        assert!(looks_like_cover_spine(&book, &book.spine[0]));
        assert!(!looks_like_cover_spine(&book, &book.spine[1]));
    }

    #[test]
    fn search_options_default_caps() {
        let o = SearchBookOptions::default();
        assert_eq!(o.max_per_chapter, 12);
        assert_eq!(o.max_total, 80);
        assert!(o.case_insensitive);
        assert!(!o.include_non_linear);
    }

    #[test]
    fn cover_id_marks_spine_cover() {
        let mut book = book_with_cover_then_text();
        book.metadata.cover_id = Some("cover".into());
        assert!(looks_like_cover_spine(&book, &book.spine[0]));
    }

    #[test]
    fn integration_search_prepare_reading_start() {
        use crate::generator::EpubBuilder;
        use crate::model::Metadata;
        use crate::processor::PrepareChapterOptions;
        use std::io::Cursor;

        let metadata = Metadata {
            title: Some("Reading API".into()),
            language: Some("en".into()),
            identifier: Some("urn:uuid:reading-api".into()),
            ..Default::default()
        };
        let mut buf = Cursor::new(Vec::new());
        EpubBuilder::new()
            .metadata(metadata)
            .add_chapter(
                "cover",
                "cover.xhtml",
                br#"<html><body><h1>Cover</h1></body></html>"#.to_vec(),
            )
            .add_chapter(
                "ch1",
                "ch1.xhtml",
                br#"<html><body><p>Hello WORLD search target</p>
                    <img src="pic.png" alt="x"/></body></html>"#
                    .to_vec(),
            )
            .add_resource("img1", "pic.png", "image/png", b"\x89PNG\r\n".to_vec())
            .generate(&mut buf)
            .unwrap();

        let bytes = buf.into_inner();
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();

        let start = archive.preferred_reading_start(&book);
        // Cover chapter is first; heuristics should skip to ch1 when id/href contain cover.
        assert_eq!(start.spine_index, 1, "expected cover skip, got {start:?}");
        assert_eq!(start.source, "cover_skip");

        let hits = archive
            .search_book_with_options(
                &book,
                "world",
                &SearchBookOptions {
                    max_per_chapter: 5,
                    max_total: 20,
                    case_insensitive: true,
                    include_non_linear: false,
                },
            )
            .unwrap();
        assert!(
            !hits.is_empty(),
            "case-insensitive search should find WORLD"
        );
        assert!(hits[0].cfi.contains("epubcfi") || hits[0].cfi.starts_with('/'));

        let (html, stats) = archive
            .prepare_chapter_with_stats(
                &book,
                "ch1",
                &PrepareChapterOptions {
                    inject_cfi: false,
                    inline_resources: true,
                    max_inline_bytes: 1024,
                },
            )
            .unwrap();
        assert!(html.contains("data:image/png;base64,") || stats.inlined >= 1);
    }

    #[test]
    fn test_search_book_no_hits_and_caps() {
        use crate::generator::EpubBuilder;
        use crate::model::Metadata;
        use std::io::Cursor;

        let metadata = Metadata {
            title: Some("Search Limits".into()),
            language: Some("en".into()),
            identifier: Some("urn:uuid:search-limits".into()),
            ..Default::default()
        };
        let mut buf = Cursor::new(Vec::new());
        EpubBuilder::new()
            .metadata(metadata)
            .add_chapter(
                "ch1",
                "ch1.xhtml",
                b"<html><body><p>Hello World</p></body></html>".to_vec(),
            )
            .generate(&mut buf)
            .unwrap();

        let mut archive = EpubArchive::new(Cursor::new(buf.into_inner())).unwrap();
        let book = archive.parse().unwrap();

        let hits = archive
            .search_book(&book, "NonexistentTerm", 5, 10)
            .unwrap();
        assert!(hits.is_empty(), "expected no hits for nonexistent term");
    }
}
