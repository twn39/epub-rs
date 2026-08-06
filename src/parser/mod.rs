//! EPUB parser module.
//!
//! Split into focused submodules:
//! - [`opf`]         — `META-INF/container.xml` and OPF package parsing (incl. EPUB 2 guide)
//! - [`encryption`]  — `META-INF/encryption.xml`
//! - [`positions`]   — Package-level progression (byte-length strategies); not DOM CFI walk
//! - [`navigation`]  — TOC, page-list, and landmarks from `nav.xhtml` or `.ncx`
//! - [`reading`]     — prepare chapter, book search, preferred reading-start

// Adapter-only OPF cache (WASM / C FFI). Compiled in tests for unit coverage.
mod encryption;
#[cfg(any(target_arch = "wasm32", feature = "ffi", test))]
mod lazy_book;
mod navigation;
mod opf;
pub mod positions;
mod reading;
pub mod resolve;
mod smil;

#[cfg(any(target_arch = "wasm32", feature = "ffi"))]
pub(crate) use lazy_book::LazyBook;

use crate::error::EpubError;
use crate::model::EpubBook;
#[cfg(not(target_arch = "wasm32"))]
use crate::provider::DirProvider;
use crate::provider::{EpubProvider, ZipProvider};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Seek};

// Re-export public types that were previously in parser.rs top-level
pub use positions::{
    ArchiveEntryLength, BYTES_PER_POSITION, OriginalLength, PositionIndex, ReflowableStrategy,
    recommended_reflowable_strategy,
};
pub use reading::{BookSearchHit, ReadingStartInfo, SearchBookOptions};

// ── Core struct ───────────────────────────────────────────────────────────────

/// Optional AES/LCP content decryptor.
///
/// Arguments: `(zip_path, ciphertext, encryption_info) → plaintext`.
/// Return `None` to leave the resource as ciphertext (or skip decryption).
pub type ContentDecryptFn =
    Box<dyn FnMut(&str, &[u8], &crate::crypto::EncryptionInfo) -> Option<Vec<u8>> + Send>;

/// A struct that handles unpacking and parsing EPUB files.
pub struct EpubArchive<P: EpubProvider> {
    pub provider: P,
    /// Decrypted/decompressed resource cache.
    cache: HashMap<String, Vec<u8>>,
    /// Tracks insertion order/access order for eviction (LRU).
    cache_order: VecDeque<String>,
    /// Current total size of elements in the cache in bytes.
    current_cache_size: usize,
    /// Maximum size of cache in bytes.
    max_cache_size_bytes: usize,
    /// Optional hook for full-content AES/LCP decryption (not used for font obfuscation).
    content_decryptor: Option<ContentDecryptFn>,
    /// Parsed navigation document for the current rendition.
    ///
    /// Invalidated by [`EpubArchive::parse_rendition`]; avoids re-parsing
    /// `nav.xhtml` / NCX on every TOC / landmarks / title lookup.
    nav_cache: Option<crate::model::NavigationDocument>,
}

impl<R: Read + Seek> EpubArchive<ZipProvider<R>> {
    /// Create a new `EpubArchive` from a generic reader containing a ZIP file.
    pub fn new(reader: R) -> Result<Self, EpubError> {
        let provider = ZipProvider::new(reader)?;
        Ok(Self {
            provider,
            cache: HashMap::new(),
            cache_order: VecDeque::new(),
            current_cache_size: 0,
            max_cache_size_bytes: 16 * 1024 * 1024, // 16MB default
            content_decryptor: None,
            nav_cache: None,
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl EpubArchive<DirProvider> {
    /// Create a new `EpubArchive` from an unzipped local directory.
    pub fn from_dir<P: AsRef<std::path::Path>>(path: P) -> Self {
        let provider = DirProvider::new(path);
        Self {
            provider,
            cache: HashMap::new(),
            cache_order: VecDeque::new(),
            current_cache_size: 0,
            max_cache_size_bytes: 16 * 1024 * 1024, // 16MB default
            content_decryptor: None,
            nav_cache: None,
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

impl<P: EpubProvider> EpubArchive<P> {
    /// Create a new `EpubArchive` from a custom provider (mainly for testing).
    pub fn new_with_provider(provider: P) -> Self {
        Self {
            provider,
            cache: HashMap::new(),
            cache_order: VecDeque::new(),
            current_cache_size: 0,
            max_cache_size_bytes: 16 * 1024 * 1024, // 16MB default
            content_decryptor: None,
            nav_cache: None,
        }
    }

    /// Install a content decryptor for AES/LCP (and other non-font) encrypted resources.
    ///
    /// Font obfuscation (IDPF/Adobe) is always handled built-in and does not use this hook.
    /// When the hook returns `None`, ciphertext bytes are returned unchanged.
    ///
    /// Clears the resource cache so subsequent reads re-apply decryption.
    pub fn set_content_decryptor<F>(&mut self, f: F)
    where
        F: FnMut(&str, &[u8], &crate::crypto::EncryptionInfo) -> Option<Vec<u8>> + Send + 'static,
    {
        self.content_decryptor = Some(Box::new(f));
        self.clear_resource_cache();
    }

    /// Remove any content decryptor and clear the resource cache.
    pub fn clear_content_decryptor(&mut self) {
        self.content_decryptor = None;
        self.clear_resource_cache();
    }

    fn clear_resource_cache(&mut self) {
        self.cache.clear();
        self.cache_order.clear();
        self.current_cache_size = 0;
    }

    /// Apply font deobfuscation and optional content decryptor to raw archive bytes.
    fn process_encrypted_bytes(
        content_decryptor: &mut Option<ContentDecryptFn>,
        book: &EpubBook,
        zip_path: &str,
        bytes: Vec<u8>,
    ) -> Vec<u8> {
        let Some(enc) = book.encryptions.get(zip_path) else {
            return bytes;
        };
        if let Some(font_algo) = enc.font_obfuscation() {
            let identifier = book.metadata.identifier.as_deref().unwrap_or("");
            let mut deobfuscated = crate::crypto::DeobfuscatingReader::new(
                Box::new(std::io::Cursor::new(bytes)),
                identifier,
                font_algo,
            );
            let mut dec_bytes = Vec::new();
            let _ = deobfuscated.read_to_end(&mut dec_bytes);
            return dec_bytes;
        }
        // Content encryption (AES/LCP, etc.)
        if let Some(decryptor) = content_decryptor
            && let Some(plain) = decryptor(zip_path, &bytes, enc)
        {
            return plain;
        }
        bytes
    }
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
        // A new rendition means different nav / manifest hrefs.
        self.nav_cache = None;
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
        self.read_zip_entry(book, &zip_path)
    }

    /// Get a readable stream for a package-root-relative ZIP path (no OPF join).
    ///
    /// Use this when the caller already holds a canonical package path (e.g.
    /// from [`EpubArchive::resolve_resource_path`]); manifest hrefs should go
    /// through [`Self::read_resource_by_href`] instead.
    pub fn read_zip_entry<'a>(
        &'a mut self,
        book: &EpubBook,
        zip_path: &str,
    ) -> Result<Box<dyn Read + 'a>, EpubError> {
        // 1. Check if the resource is in the cache (Cache Hit)
        if self.cache.contains_key(zip_path) {
            // Update LRU access order by moving this key to the back of the queue
            if let Some(pos) = self.cache_order.iter().position(|k| k == zip_path) {
                self.cache_order.remove(pos);
            }
            self.cache_order.push_back(zip_path.to_string());

            let cached = self.cache.get(zip_path).unwrap();
            return Ok(Box::new(std::io::Cursor::new(cached.as_slice())));
        }

        // 2. Cache Miss: Query the length and check if we should cache it
        let length = self.provider.entry_length(zip_path).unwrap_or(0) as usize;
        let max_cache_size = 2 * 1024 * 1024; // 2MB file size limit for caching

        if length <= max_cache_size {
            // Read, decompress, and decrypt/deobfuscate the resource
            let file = self.provider.read_file(zip_path)?;
            let mut bytes = Vec::new();
            let mut buf_reader = file;
            buf_reader.read_to_end(&mut bytes)?;

            let decrypted =
                Self::process_encrypted_bytes(&mut self.content_decryptor, book, zip_path, bytes);

            let decrypted_len = decrypted.len();

            // Perform LRU eviction if caching this item would exceed the total cache limit
            while self.current_cache_size + decrypted_len > self.max_cache_size_bytes
                && !self.cache_order.is_empty()
            {
                let oldest = self.cache_order.pop_front().unwrap();
                if let Some(old_bytes) = self.cache.remove(&oldest) {
                    self.current_cache_size -= old_bytes.len();
                }
            }

            // Insert into the cache and update the LRU order
            self.current_cache_size += decrypted_len;
            self.cache.insert(zip_path.to_string(), decrypted);
            self.cache_order.push_back(zip_path.to_string());

            let cached = self.cache.get(zip_path).unwrap();
            Ok(Box::new(std::io::Cursor::new(cached.as_slice())))
        } else {
            // 3. Bypass cache for files exceeding the size limit
            // Large files: stream when unencrypted; materialise when encryption map hits.
            if book.encryptions.contains_key(zip_path) {
                let mut raw = Vec::new();
                {
                    let mut r = self.provider.read_file(zip_path)?;
                    r.read_to_end(&mut raw)?;
                }
                let decrypted =
                    Self::process_encrypted_bytes(&mut self.content_decryptor, book, zip_path, raw);
                Ok(Box::new(std::io::Cursor::new(decrypted)))
            } else {
                Ok(self.provider.read_file(zip_path)?)
            }
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

    /// Read the raw bytes of a resource by its package-root-relative ZIP path.
    ///
    /// Goes through the same LRU cache and decryption path as the href-based
    /// readers; use it when the path was canonicalized up front (e.g. via
    /// [`EpubArchive::resolve_resource_path`]).
    pub fn get_resource_by_zip_path(
        &mut self,
        book: &EpubBook,
        zip_path: &str,
    ) -> Result<Vec<u8>, EpubError> {
        let mut file = self.read_zip_entry(book, zip_path)?;
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

    // Reading-system APIs (prepare / search / reading-start) live in `reading.rs`.

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

    /// Normalizes an EPUB path by resolving `.` and `..` relative segments.
    ///
    /// Delegates to [`crate::path::join_epub_path`] so OPF/manifest resolution
    /// matches navigation and SMIL path joining.
    pub(crate) fn normalize_path(base: &str, href: &str) -> String {
        crate::path::join_epub_path(base, href)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EpubBook;
    use crate::provider::EpubProvider;
    use std::io::Read;

    struct MockProvider {
        files: HashMap<String, Vec<u8>>,
    }

    impl EpubProvider for MockProvider {
        fn read_file<'a>(&'a mut self, path: &str) -> Result<Box<dyn Read + 'a>, EpubError> {
            match self.files.get(path) {
                Some(bytes) => Ok(Box::new(std::io::Cursor::new(bytes.clone()))),
                None => Err(EpubError::InvalidFormat(format!(
                    "File not found: {}",
                    path
                ))),
            }
        }

        fn entry_length(&mut self, path: &str) -> Result<u64, EpubError> {
            match self.files.get(path) {
                Some(bytes) => Ok(bytes.len() as u64),
                None => Ok(0),
            }
        }
    }

    fn make_test_book() -> EpubBook {
        EpubBook {
            metadata: crate::model::Metadata::default(),
            manifest: HashMap::new(),
            spine: Vec::new(),
            opf_dir: String::new(),
            toc_id: None,
            guide: Vec::new(),
            encryptions: HashMap::new(),
        }
    }

    #[test]
    fn test_cache_hit_and_lru_order() {
        let mut files = HashMap::new();
        files.insert("a.xhtml".to_string(), vec![1; 10]);
        files.insert("b.xhtml".to_string(), vec![2; 20]);

        let provider = MockProvider { files };
        let mut archive = EpubArchive::new_with_provider(provider);
        archive.max_cache_size_bytes = 100;

        let book = make_test_book();

        // 1. Initial read of A (cache miss)
        {
            let mut r1 = archive.read_resource_by_href(&book, "a.xhtml").unwrap();
            let mut content1 = Vec::new();
            r1.read_to_end(&mut content1).unwrap();
            assert_eq!(content1, vec![1; 10]);
        }
        assert_eq!(archive.current_cache_size, 10);
        assert_eq!(archive.cache_order, vec!["a.xhtml".to_string()]);
        assert!(archive.cache.contains_key("a.xhtml"));

        // 2. Initial read of B (cache miss)
        {
            let mut r2 = archive.read_resource_by_href(&book, "b.xhtml").unwrap();
            let mut content2 = Vec::new();
            r2.read_to_end(&mut content2).unwrap();
            assert_eq!(content2, vec![2; 20]);
        }
        assert_eq!(archive.current_cache_size, 30);
        assert_eq!(
            archive.cache_order,
            vec!["a.xhtml".to_string(), "b.xhtml".to_string()]
        );

        // 3. Read A again (cache hit - should update LRU order to move A to the back)
        {
            let mut r3 = archive.read_resource_by_href(&book, "a.xhtml").unwrap();
            let mut content3 = Vec::new();
            r3.read_to_end(&mut content3).unwrap();
            assert_eq!(content3, vec![1; 10]);
        }
        assert_eq!(archive.current_cache_size, 30);
        assert_eq!(
            archive.cache_order,
            vec!["b.xhtml".to_string(), "a.xhtml".to_string()]
        );
    }

    #[test]
    fn test_cache_lru_eviction() {
        let mut files = HashMap::new();
        files.insert("a.xhtml".to_string(), vec![1; 10]);
        files.insert("b.xhtml".to_string(), vec![2; 20]);
        files.insert("c.xhtml".to_string(), vec![3; 30]);

        let provider = MockProvider { files };
        let mut archive = EpubArchive::new_with_provider(provider);
        // Set maximum cache size limit such that we can hold A and B (30 bytes),
        // but adding C (30 bytes) will exceed it (total 60 bytes > 45 bytes).
        archive.max_cache_size_bytes = 45;

        let book = make_test_book();

        // Cache A
        {
            let _ = archive.read_resource_by_href(&book, "a.xhtml").unwrap();
        }
        // Cache B
        {
            let _ = archive.read_resource_by_href(&book, "b.xhtml").unwrap();
        }

        assert_eq!(archive.current_cache_size, 30);
        assert_eq!(
            archive.cache_order,
            vec!["a.xhtml".to_string(), "b.xhtml".to_string()]
        );

        // Read C: C has size 30.
        // current_cache_size (30) + 30 = 60 > 45.
        // Eviction happens:
        // 1. Evicts oldest ("a.xhtml" - 10 bytes). Remaining size = 20.
        // 2. 20 + 30 = 50 > 45. Evicts next oldest ("b.xhtml" - 20 bytes). Remaining size = 0.
        // 3. 0 + 30 = 30 <= 45. Cache C.
        {
            let mut r = archive.read_resource_by_href(&book, "c.xhtml").unwrap();
            let mut content = Vec::new();
            r.read_to_end(&mut content).unwrap();
            assert_eq!(content, vec![3; 30]);
        }

        assert_eq!(archive.current_cache_size, 30);
        assert_eq!(archive.cache_order, vec!["c.xhtml".to_string()]);
        assert!(!archive.cache.contains_key("a.xhtml"));
        assert!(!archive.cache.contains_key("b.xhtml"));
        assert!(archive.cache.contains_key("c.xhtml"));
    }

    #[test]
    fn test_cache_bypass_large_file() {
        let mut files = HashMap::new();
        // 2.1 MB file (> 2MB limit)
        let large_size = 2 * 1024 * 1024 + 100 * 1024;
        files.insert("large.xhtml".to_string(), vec![9; large_size]);
        files.insert("small.xhtml".to_string(), vec![1; 10]);

        let provider = MockProvider { files };
        let mut archive = EpubArchive::new_with_provider(provider);
        archive.max_cache_size_bytes = 5 * 1024 * 1024; // 5MB limit

        let book = make_test_book();

        // 1. Read large file -> should bypass cache
        {
            let mut r1 = archive.read_resource_by_href(&book, "large.xhtml").unwrap();
            let mut content1 = Vec::new();
            r1.read_to_end(&mut content1).unwrap();
            assert_eq!(content1.len(), large_size);
        }
        assert_eq!(archive.current_cache_size, 0);
        assert!(archive.cache.is_empty());
        assert!(archive.cache_order.is_empty());

        // 2. Read small file -> should be cached
        {
            let mut r2 = archive.read_resource_by_href(&book, "small.xhtml").unwrap();
            let mut content2 = Vec::new();
            r2.read_to_end(&mut content2).unwrap();
            assert_eq!(content2, vec![1; 10]);
        }
        assert_eq!(archive.current_cache_size, 10);
        assert_eq!(archive.cache_order, vec!["small.xhtml".to_string()]);
        assert!(archive.cache.contains_key("small.xhtml"));
    }

    #[test]
    fn test_parse_by_index_out_of_bounds() {
        let mut files = HashMap::new();
        files.insert(
            "META-INF/container.xml".to_string(),
            br#"<?xml version="1.0"?>
            <container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
                <rootfiles>
                    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
                </rootfiles>
            </container>"#.to_vec(),
        );

        let provider = MockProvider { files };
        let mut archive = EpubArchive::new_with_provider(provider);
        let err = archive.parse_by_index(5).unwrap_err();
        match err {
            EpubError::InvalidFormat(msg) => assert!(msg.contains("out of range")),
            other => panic!("Expected InvalidFormat, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_best_for_scoring() {
        use crate::model::RenditionInfo;

        let renditions = [
            RenditionInfo {
                opf_path: "default.opf".to_string(),
                layout: Some("pre-paginated".to_string()),
                language: Some("fr".to_string()),
                label: None,
                media: None,
                access_mode: None,
            },
            RenditionInfo {
                opf_path: "text.opf".to_string(),
                layout: Some("reflowable".to_string()),
                language: Some("en".to_string()),
                label: None,
                media: None,
                access_mode: None,
            },
            RenditionInfo {
                opf_path: "comic.opf".to_string(),
                layout: Some("pre-paginated".to_string()),
                language: Some("en".to_string()),
                label: None,
                media: None,
                access_mode: None,
            },
        ];

        // 1. Layout match (reflowable) should pick text.opf (weight 2 for layout match + 0 for language fr != en = score 2)
        let layout_pref = "reflowable";
        let lang_pref = "fr";
        let best1 = renditions
            .iter()
            .max_by_key(|r| {
                let mut score = 0;
                if r.layout.as_deref() == Some(layout_pref) {
                    score += 2;
                }
                if r.language.as_deref() == Some(lang_pref) {
                    score += 1;
                }
                score
            })
            .unwrap();
        assert_eq!(best1.opf_path, "text.opf");

        // 2. Exact match (pre-paginated + en) should pick comic.opf (score 3)
        let layout_pref2 = "pre-paginated";
        let lang_pref2 = "en";
        let best2 = renditions
            .iter()
            .max_by_key(|r| {
                let mut score = 0;
                if r.layout.as_deref() == Some(layout_pref2) {
                    score += 2;
                }
                if r.language.as_deref() == Some(lang_pref2) {
                    score += 1;
                }
                score
            })
            .unwrap();
        assert_eq!(best2.opf_path, "comic.opf");
    }

    #[test]
    fn test_get_resource_by_id_missing_item() {
        let provider = MockProvider {
            files: HashMap::new(),
        };
        let mut archive = EpubArchive::new_with_provider(provider);
        let book = make_test_book();
        let err = archive
            .get_resource_by_id(&book, "nonexistent_id")
            .unwrap_err();
        match err {
            EpubError::InvalidFormat(msg) => assert!(msg.contains("nonexistent_id")),
            other => panic!("Expected InvalidFormat, got {other:?}"),
        }
    }
}
