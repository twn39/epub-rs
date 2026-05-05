//! EPUB parser module.
//!
//! Split into focused submodules:
//! - [`opf`]        — `META-INF/container.xml`, `encryption.xml`, and OPF package parsing
//! - [`positions`]  — Position computation strategy and reading-order locations
//! - [`navigation`] — TOC, page-list, and landmarks from `nav.xhtml` or `.ncx`

mod navigation;
mod opf;
pub mod positions;

use crate::error::EpubError;
use crate::model::EpubBook;
#[cfg(not(target_arch = "wasm32"))]
use crate::provider::DirProvider;
use crate::provider::{EpubProvider, ZipProvider};
use std::io::{Read, Seek};

// Re-export public types that were previously in parser.rs top-level
pub use positions::{
    ArchiveEntryLength, BYTES_PER_POSITION, ReflowableStrategy, recommended_reflowable_strategy,
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
    /// Get all available renditions (rootfiles) in the EPUB container.
    pub fn get_renditions(&mut self) -> Result<Vec<String>, EpubError> {
        self.parse_container()
    }

    /// Parse the EPUB archive and extract metadata, manifest, and spine.
    pub fn parse(&mut self) -> Result<EpubBook, EpubError> {
        let rootfiles = self.parse_container()?;
        self.parse_rendition(&rootfiles[0])
    }

    /// Parse a specific rendition by its OPF path.
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
        if let Some(&algo) = book.encryptions.get(&zip_path) {
            let identifier = book.metadata.identifier.as_deref().unwrap_or("");
            let deobfuscated = crate::crypto::DeobfuscatingReader::new(file, identifier, algo);
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
