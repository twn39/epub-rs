//! EPUB parser module.
//!
//! Split into focused submodules:
//! - [`opf`]        — `META-INF/container.xml`, `encryption.xml`, and OPF package parsing
//! - [`positions`]  — Position computation strategy and reading-order locations
//! - [`navigation`] — TOC, page-list, and landmarks from `nav.xhtml` or `.ncx`

mod navigation;
mod opf;
pub mod positions;
mod smil;

use crate::error::EpubError;
use crate::model::EpubBook;
#[cfg(not(target_arch = "wasm32"))]
use crate::provider::DirProvider;
use crate::provider::{EpubProvider, ZipProvider};
use std::io::{Read, Seek};

// Re-export public types that were previously in parser.rs top-level
pub use positions::{
    ArchiveEntryLength, BYTES_PER_POSITION, OriginalLength, PositionIndex, ReflowableStrategy,
    recommended_reflowable_strategy,
};

// ── Core struct ───────────────────────────────────────────────────────────────

/// A struct that handles unpacking and parsing EPUB files.
pub struct EpubArchive<P: EpubProvider> {
    pub provider: P,
}

impl<R: Read + Seek> EpubArchive<ZipProvider<R>> {
    /// Create a new `EpubArchive` from a generic reader containing a ZIP file.
    pub fn new(reader: R) -> Result<Self, EpubError> {
        let provider = ZipProvider::new(reader)?;
        Ok(Self { provider })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl EpubArchive<DirProvider> {
    /// Create a new `EpubArchive` from an unzipped local directory.
    pub fn from_dir<P: AsRef<std::path::Path>>(path: P) -> Self {
        let provider = DirProvider::new(path);
        Self { provider }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

impl<P: EpubProvider> EpubArchive<P> {
    /// Returns all renditions declared in `META-INF/container.xml`.
    ///
    /// Per OCF §3.5.1 the **first** entry is the *default rendition* and must be processed
    /// by every conformant Reading System.  Additional entries carry optional selection
    /// attributes (`rendition:layout`, `rendition:language`, etc.) defined by the EPUB
    /// Multiple-Rendition Publications specification.
    ///
    /// # Usage
    /// ```no_run
    /// # use epub_rs::parser::EpubArchive;
    /// # use std::io::Cursor;
    /// # let data: Vec<u8> = vec![];
    /// let mut archive = EpubArchive::new(Cursor::new(data)).unwrap();
    /// let renditions = archive.get_renditions().unwrap();
    /// // renditions[0] is always the default
    /// for r in &renditions {
    ///     println!("{} — layout: {:?}", r.opf_path, r.layout);
    /// }
    /// ```
    pub fn get_renditions(&mut self) -> Result<Vec<crate::model::RenditionInfo>, EpubError> {
        self.parse_container()
    }

    /// Parse the EPUB's **default rendition** (the first `<rootfile>` in `container.xml`).
    ///
    /// This is the standard entry point for single-rendition EPUBs (the vast majority of
    /// publications).  For multi-rendition containers, prefer [`parse_best_for`] or
    /// [`parse_by_index`] to select the most appropriate version.
    pub fn parse(&mut self) -> Result<EpubBook, EpubError> {
        let renditions = self.parse_container()?;
        // Per OCF §3.5.1, the default rendition is always at index 0.
        self.parse_rendition(&renditions[0].opf_path)
    }

    /// Parse a specific rendition selected by its 0-based index in `container.xml`.
    ///
    /// Returns `Err(EpubError::InvalidFormat)` if `index` is out of range.
    ///
    /// # Example
    /// ```no_run
    /// # use epub_rs::parser::EpubArchive;
    /// # use std::io::Cursor;
    /// # let data: Vec<u8> = vec![];
    /// let mut archive = EpubArchive::new(Cursor::new(data)).unwrap();
    /// // Parse the second rendition (index 1) — e.g. the reflowable text edition
    /// let book = archive.parse_by_index(1).unwrap();
    /// ```
    pub fn parse_by_index(&mut self, index: usize) -> Result<EpubBook, EpubError> {
        let renditions = self.parse_container()?;
        let info = renditions.into_iter().nth(index).ok_or_else(|| {
            EpubError::InvalidFormat(format!(
                "Rendition index {index} is out of range (container has fewer rootfiles)"
            ))
        })?;
        self.parse_rendition(&info.opf_path)
    }

    /// Parse the best-matching rendition for the given preferences.
    ///
    /// Selection strategy (first match wins, falls back to default rendition):
    ///
    /// 1. Both `layout` and `language` match → exact match
    /// 2. Only `layout` matches
    /// 3. Only `language` matches  
    /// 4. Default rendition (index 0)
    ///
    /// Pass `None` for any preference you don't care about.
    ///
    /// # Example — prefer the reflowable text edition in Traditional Chinese
    /// ```no_run
    /// # use epub_rs::parser::EpubArchive;
    /// # use std::io::Cursor;
    /// # let data: Vec<u8> = vec![];
    /// let mut archive = EpubArchive::new(Cursor::new(data)).unwrap();
    /// let book = archive.parse_best_for(Some("reflowable"), Some("zh-Hant")).unwrap();
    /// ```
    pub fn parse_best_for(
        &mut self,
        layout: Option<&str>,
        language: Option<&str>,
    ) -> Result<EpubBook, EpubError> {
        let renditions = self.parse_container()?;

        // Layout match outweighs language match (weight 2 vs 1) because choosing the
        // wrong layout (e.g. pre-paginated on a small screen) is a rendering failure,
        // while a wrong language edition is merely inconvenient.
        let scored: Option<&crate::model::RenditionInfo> = renditions
            .iter()
            .max_by_key(|r| {
                let mut score: u8 = 0;
                if let Some(want_layout) = layout
                    && (r.layout.as_deref() == Some(want_layout)
                        || (want_layout == "reflowable" && r.layout.is_none()))
                {
                    score += 2;
                }
                if let Some(want_lang) = language
                    && r.language.as_deref() == Some(want_lang)
                {
                    score += 1;
                }
                score
            })
            .filter(|r| {
                // max_by_key always returns *some* element, even when every score is 0.
                // The filter gates on at least one criterion matching so we never
                // accidentally return a non-default rendition when nothing matched.
                let want_layout = layout.map(|l| {
                    r.layout.as_deref() == Some(l) || (l == "reflowable" && r.layout.is_none())
                });
                let want_lang = language.map(|l| r.language.as_deref() == Some(l));
                want_layout.unwrap_or(false) || want_lang.unwrap_or(false)
            });

        let opf_path = match scored {
            Some(r) => r.opf_path.clone(),
            // Nothing satisfied the caller's constraints; OCF requires every Reading
            // System to be able to process the first rootfile, so it is always safe.
            None => renditions[0].opf_path.clone(),
        };

        self.parse_rendition(&opf_path)
    }

    /// Parse a specific rendition by its OPF path.
    ///
    /// `encryption.xml` is loaded unconditionally because there is no cheap way to
    /// know whether a given rendition contains encrypted resources before parsing
    /// the manifest; a missing or empty file produces an empty map.
    pub fn parse_rendition(&mut self, opf_path: &str) -> Result<EpubBook, EpubError> {
        let mut book = self.parse_opf(opf_path)?;
        book.encryptions = self.parse_encryption().unwrap_or_default();
        Ok(book)
    }

    // ── Resource access ───────────────────────────────────────────────────────

    /// Get a readable stream for a resource given its manifest href.
    pub fn read_resource_by_href<'a>(
        &'a mut self,
        book: &EpubBook,
        href: &str,
    ) -> Result<Box<dyn Read + 'a>, EpubError> {
        let zip_path = if book.opf_dir.is_empty() {
            Self::normalize_path("", href)
        } else {
            Self::normalize_path(&book.opf_dir, href)
        };

        let file = self.provider.read_file(&zip_path)?;

        // Wrap with deobfuscating reader if this file is encrypted
        if let Some(enc) = book.encryptions.get(&zip_path) {
            let identifier = book.metadata.identifier.as_deref().unwrap_or("");
            let deobfuscated =
                crate::crypto::DeobfuscatingReader::new(file, identifier, enc.algorithm);
            Ok(Box::new(deobfuscated))
        } else {
            Ok(file)
        }
    }

    /// Get a readable stream for a resource given its manifest ID.
    pub fn read_resource_by_id<'a>(
        &'a mut self,
        book: &EpubBook,
        id: &str,
    ) -> Result<Box<dyn Read + 'a>, EpubError> {
        let href = book
            .manifest
            .get(id)
            .ok_or_else(|| EpubError::InvalidFormat(format!("ID {} not found in manifest", id)))?
            .href
            .clone();
        self.read_resource_by_href(book, &href)
    }

    /// Read the raw bytes of a resource from the archive given its manifest href.
    pub fn get_resource_by_href(
        &mut self,
        book: &EpubBook,
        href: &str,
    ) -> Result<Vec<u8>, EpubError> {
        let mut file = self.read_resource_by_href(book, href)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// Helper to get a resource by its manifest ID.
    pub fn get_resource_by_id(&mut self, book: &EpubBook, id: &str) -> Result<Vec<u8>, EpubError> {
        let mut file = self.read_resource_by_id(book, id)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// Smart API to find and extract the cover image of the EPUB.
    /// Returns the bytes of the image and its media_type (e.g., `"image/jpeg"`).
    pub fn get_cover_image(&mut self, book: &EpubBook) -> Result<(Vec<u8>, String), EpubError> {
        let mut best_match = None;

        // 1. O(1) Fast path: EPUB 2 meta name="cover"
        if let Some(cover_id) = &book.metadata.cover_id
            && let Some(item) = book.manifest.get(cover_id)
        {
            best_match = Some((&item.id, &item.media_type));
        }

        // 2. O(N) Single-pass scan for EPUB 3 cover or heuristics
        if best_match.is_none() {
            let mut fallback_name = None;
            let mut fallback_any = None;

            for item in book.manifest.values() {
                // Priority A: Exact EPUB 3 property
                if item.properties.iter().any(|p| p == "cover-image") {
                    best_match = Some((&item.id, &item.media_type));
                    break; // Absolute certainty, we can stop scanning
                }

                if item.media_type.starts_with("image/") {
                    // Priority B: Filename heuristic
                    if fallback_name.is_none() {
                        let id_lower = item.id.to_lowercase();
                        let href_lower = item.href.to_lowercase();
                        if id_lower.contains("cover") || href_lower.contains("cover") {
                            fallback_name = Some((&item.id, &item.media_type));
                        }
                    }
                    // Priority C: Any image fallback
                    if fallback_any.is_none() {
                        fallback_any = Some((&item.id, &item.media_type));
                    }
                }
            }

            if best_match.is_none() {
                best_match = fallback_name.or(fallback_any);
            }
        }

        // Clone the tiny IDs to immediately drop the borrow on `book.manifest`
        if let Some((id, media_type)) = best_match {
            let id_clone = id.clone();
            let media_type_clone = media_type.clone();
            let bytes = self.get_resource_by_id(book, &id_clone)?;
            Ok((bytes, media_type_clone))
        } else {
            Err(EpubError::InvalidFormat(
                "No cover image found in EPUB".to_string(),
            ))
        }
    }

    // ── High-level reader features ────────────────────────────────────────────

    /// Reads a chapter's HTML and automatically injects `data-cfi` attributes into all DOM nodes.
    /// This is a high-level method designed for building Web Readers.
    ///
    /// It automatically calculates the `base_cfi` (OPF context) for the given spine item.
    pub fn get_chapter_with_cfi(&mut self, book: &EpubBook, id: &str) -> Result<String, EpubError> {
        let spine_index = book
            .spine
            .iter()
            .position(|s| s.idref == id)
            .ok_or_else(|| EpubError::InvalidFormat(format!("ID {} not found in spine", id)))?;

        let base_cfi = crate::cfi::EpubCfi::generate_spine_base_cfi(spine_index, id);
        let raw_html = self.get_resource_by_id(book, id)?;
        let html_str = String::from_utf8_lossy(&raw_html);

        crate::processor::inject_cfi_dom(&html_str, &base_cfi)
    }

    /// Searches the given chapter's HTML for a regular expression and returns exact CFI ranges.
    pub fn search_chapter(
        &mut self,
        book: &EpubBook,
        id: &str,
        pattern: &regex::Regex,
    ) -> Result<Vec<crate::processor::SearchResult>, EpubError> {
        let spine_index = book
            .spine
            .iter()
            .position(|s| s.idref == id)
            .ok_or_else(|| EpubError::InvalidFormat(format!("ID {} not found in spine", id)))?;

        let base_cfi = crate::cfi::EpubCfi::generate_spine_base_cfi(spine_index, id);
        let raw_html = self.get_resource_by_id(book, id)?;
        let html_str = String::from_utf8_lossy(&raw_html);

        crate::processor::search_chapter(&html_str, &base_cfi, pattern)
    }

    /// Extracts semantic content blocks (paragraphs, headings) from a specific chapter.
    /// Returns blocks with their text, tags, languages, and CFI paths.
    /// This is highly useful for Text-to-Speech (TTS) integrations.
    pub fn get_semantic_content(
        &mut self,
        book: &EpubBook,
        id: &str,
    ) -> Result<Vec<crate::model::ContentElement>, EpubError> {
        let spine_index = book
            .spine
            .iter()
            .position(|s| s.idref == id)
            .ok_or_else(|| EpubError::InvalidFormat(format!("ID {} not found in spine", id)))?;

        let base_cfi = crate::cfi::EpubCfi::generate_spine_base_cfi(spine_index, id);
        let raw_html = self.get_resource_by_id(book, id)?;
        let html_str = String::from_utf8_lossy(&raw_html);

        Ok(crate::processor::extract_semantic_content(
            &html_str, &base_cfi,
        ))
    }

    // ── Media Overlays API ──────────────────────────────────────────

    /// Returns `true` if any content document in this EPUB has a SMIL Media Overlay.
    ///
    /// This is an O(n) scan over manifest items. It is useful as a fast check to
    /// determine whether an EPUB is an audiobook with synchronized text–audio playback
    /// before calling [`Self::get_media_overlay`] for individual chapters.
    pub fn has_media_overlays(&self, book: &EpubBook) -> bool {
        book.manifest
            .values()
            .any(|item| item.media_overlay.is_some())
    }

    /// Parses and returns the SMIL Media Overlay for a spine content document.
    ///
    /// `content_href` is the manifest `href` of the XHTML file (as stored in
    /// `book.manifest`, EPUB-root-relative).
    ///
    /// # Returns
    /// - `Ok(None)` — no overlay is associated with this document (normal for
    ///   non-audiobook EPUBs or spine items without a `media-overlay` attribute).
    /// - `Ok(Some(doc))` — the parsed overlay with ordered sync points and
    ///   optional prev/next chapter links for sequential playback.
    /// - `Err(_)` — I/O failure or malformed SMIL XML.
    ///
    /// # Note
    /// This method performs I/O on each call.  The caller is responsible for
    /// caching the result if repeated access is expected.
    pub fn get_media_overlay(
        &mut self,
        book: &EpubBook,
        content_href: &str,
    ) -> Result<Option<crate::model::SmilDocument>, EpubError> {
        // Find the manifest item for the requested content document
        let overlay_id = book
            .manifest
            .values()
            .find(|item| item.href == content_href)
            .and_then(|item| item.media_overlay.clone());

        let overlay_id = match overlay_id {
            Some(id) => id,
            None => return Ok(None),
        };

        // Resolve the SMIL file path
        let smil_item = book.manifest.get(&overlay_id).ok_or_else(|| {
            EpubError::InvalidFormat(format!(
                "media-overlay ID '{}' not found in manifest",
                overlay_id
            ))
        })?;

        let smil_href = smil_item.href.clone();
        let smil_path = if book.opf_dir.is_empty() {
            smil_href.clone()
        } else {
            format!("{}/{}", book.opf_dir, smil_href)
        };

        // smil_dir = directory portion of the SMIL file's EPUB-root-relative path
        let smil_dir = smil_path
            .rfind('/')
            .map(|i| smil_path[..i].to_string())
            .unwrap_or_default();

        // Read and parse the SMIL file
        let mut file = self.provider.read_file(&smil_path)?;
        let mut xml_buf = String::new();
        file.read_to_string(&mut xml_buf)?;

        let objects = smil::parse_smil(&xml_buf, &smil_dir)?;

        // Build prev/next SMIL links by walking the spine in reading order
        // (only spine items that have an overlay are considered)
        let overlay_hrefs: Vec<String> = book
            .spine
            .iter()
            .filter(|s| s.linear)
            .filter_map(|s| book.manifest.get(&s.idref))
            .filter(|item| item.media_overlay.is_some())
            .filter_map(|item| {
                let ov_id = item.media_overlay.as_ref()?;
                book.manifest.get(ov_id).map(|smil| {
                    if book.opf_dir.is_empty() {
                        smil.href.clone()
                    } else {
                        format!("{}/{}", book.opf_dir, smil.href)
                    }
                })
            })
            .collect();

        let cur_pos = overlay_hrefs.iter().position(|h| *h == smil_path);
        let prev_smil_href = cur_pos
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| overlay_hrefs.get(i))
            .cloned();
        let next_smil_href = cur_pos.and_then(|i| overlay_hrefs.get(i + 1)).cloned();

        Ok(Some(crate::model::SmilDocument {
            objects,
            prev_smil_href,
            next_smil_href,
        }))
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Normalizes an EPUB path by resolving `.` and `..` relative segments.
    pub(crate) fn normalize_path(base: &str, href: &str) -> String {
        let mut parts = Vec::new();

        for comp in base.split('/').chain(href.split('/')) {
            if comp.is_empty() || comp == "." {
                continue;
            }
            if comp == ".." {
                parts.pop(); // Go up one directory
            } else {
                parts.push(comp);
            }
        }

        parts.join("/")
    }
}
