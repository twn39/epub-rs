//! EPUB parser module.

use crate::error::EpubError;
use crate::model::{EpubBook, ManifestItem, NavigationDocument, Position, TocEntry};
#[cfg(not(target_arch = "wasm32"))]
use crate::provider::DirProvider;
use crate::provider::{EpubProvider, ZipProvider};
use kuchikiki::traits::*;

#[derive(Debug, Clone)]
enum OpfState {
    None,
    Title,
    /// EPUB 2 `opf:title-type="subtitle"` or EPUB 3 refinement `title-type=subtitle`
    Subtitle,
    Creator(Option<String>),
    Contributor(Option<String>),
    Language,
    Identifier,
    Publisher,
    Description,
    Date,
    /// EPUB 3 `dcterms:modified` or EPUB 2 `dc:date opf:event="modification"`
    Modified,
    Rights,
    Subject,
    MetaRefines { ref_id: String, property: String },
    /// Global `<meta property="...">` — carries the element's own `id` for refinement lookup
    MetaGlobal { property: String, id: Option<String> },
}

use quick_xml::Reader;
use quick_xml::events::Event;
use std::io::{Read, Seek};

// ─────────────────────────────────────────────────────────────────────────────
// Positions strategy
// ─────────────────────────────────────────────────────────────────────────────

/// Default bytes per reading position, matching the Adobe RMSDK and Readium standard.
///
/// See: <https://github.com/readium/architecture/issues/123>
pub const BYTES_PER_POSITION: usize = 1024;

/// Strategy for computing the number of positions in a reflowable spine item.
///
/// Mirrors go-toolkit's `ReflowableStrategy` interface (`positions_service.go`).
/// A fixed-layout spine item always produces exactly 1 position, regardless of strategy.
pub trait ReflowableStrategy: Send + Sync {
    /// Returns the number of positions (≥ 1) for a spine item with the given byte length.
    fn position_count(&self, entry_length: u64) -> usize;
}

/// Strategy that uses the uncompressed ZIP entry length divided by `page_length`.
///
/// This is the **recommended** strategy, matching Adobe RMSDK and Readium defaults.
///
/// Equivalent to go-toolkit's `ArchiveEntryLength` (not `OriginalLength`, which has
/// a bug where it uses `math.Min` instead of `math.Max`, capping results at 1).
pub struct ArchiveEntryLength {
    /// Number of bytes per reading position. Typically 1024.
    pub page_length: usize,
}

impl ReflowableStrategy for ArchiveEntryLength {
    fn position_count(&self, entry_length: u64) -> usize {
        // max(ceil(entry_length / page_length), 1)
        // Ensures at least 1 position even for empty or very small files.
        let page_len = (self.page_length.max(1)) as u64;
        let count = entry_length.div_ceil(page_len) as usize;
        count.max(1)
    }
}

/// Returns the recommended reflowable strategy: `ArchiveEntryLength { page_length: 1024 }`.
pub fn recommended_reflowable_strategy() -> ArchiveEntryLength {
    ArchiveEntryLength {
        page_length: BYTES_PER_POSITION,
    }
}


/// A struct that handles unpacking and parsing EPUB files.
pub struct EpubArchive<P: EpubProvider> {
    pub provider: P,
}

impl<R: Read + Seek> EpubArchive<ZipProvider<R>> {
    /// Create a new `EpubArchive` from a generic reader containing a ZIP file
    pub fn new(reader: R) -> Result<Self, EpubError> {
        let provider = ZipProvider::new(reader)?;
        Ok(Self { provider })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl EpubArchive<DirProvider> {
    /// Create a new `EpubArchive` from an unzipped local directory
    pub fn from_dir<P: AsRef<std::path::Path>>(path: P) -> Self {
        let provider = DirProvider::new(path);
        Self { provider }
    }
}

impl<P: EpubProvider> EpubArchive<P> {
    /// Get all available renditions (rootfiles) in the EPUB container.
    pub fn get_renditions(&mut self) -> Result<Vec<String>, EpubError> {
        self.parse_container()
    }

    /// Parse the EPUB archive and extract metadata, manifest, and spine
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

    /// Generate virtual pages (locations) for the entire EPUB based on a character limit.
    /// Returns positions grouped by spine item (reading order).
    ///
    /// The outer `Vec` index corresponds to the **reading order index** (i.e. only linear
    /// spine items are included; non-linear items such as pop-up footnotes are skipped).
    ///
    /// Mirrors go-toolkit's `PositionsByReadingOrder()` / `computePositions()`.
    ///
    /// The `strategy` parameter controls how position counts are computed for reflowable
    /// resources. Use [`recommended_reflowable_strategy()`] for the Adobe/Readium standard.
    pub fn positions_by_reading_order(
        &mut self,
        book: &EpubBook,
        strategy: &dyn ReflowableStrategy,
    ) -> Result<Vec<Vec<crate::model::Position>>, EpubError> {
        // Build href -> title map from the TOC for position title enrichment
        let toc = self.get_toc(book).unwrap_or_default();
        let title_map = build_title_map(&toc);

        // Collect only linear spine items — non-linear items don't count toward reading progress
        // (mirrors go-toolkit which skips `linear=false` items in reading-order traversal)
        let linear_items: Vec<(usize, &crate::model::SpineItem)> = book
            .spine
            .iter()
            .enumerate()
            .filter(|(_, item)| item.linear)
            .collect();

        let mut result: Vec<Vec<crate::model::Position>> =
            Vec::with_capacity(linear_items.len());

        // `last_position` carries the last global_position from the previous chapter,
        // exactly like go-toolkit's `lastPositionOfPreviousResource`.
        let mut last_position: usize = 0;

        // ── Pass 1: compute per-chapter positions with local progressions ─────
        for (spine_index, item) in &linear_items {
            let manifest_item = book.manifest.get(&item.idref).ok_or_else(|| {
                EpubError::InvalidFormat(format!("Missing manifest item: {}", item.idref))
            })?;

            // Determine the effective layout for this spine item:
            // the item-level override takes precedence over the publication-level default.
            let is_fixed = matches!(
                item.layout_override.unwrap_or(book.metadata.layout),
                crate::model::LayoutType::PrePaginated
            );

            // Fixed layout: always 1 position.
            // Reflowable: delegate to the strategy (ArchiveEntryLength by default).
            let position_count = if is_fixed {
                1usize
            } else {
                let byte_len = self
                    .provider
                    .entry_length(&manifest_item.href)
                    .unwrap_or(0); // graceful degradation for missing/unreadable files
                strategy.position_count(byte_len)
            };

            let base_cfi =
                crate::cfi::EpubCfi::generate_spine_base_cfi(*spine_index, &item.idref);
            let spine_path = base_cfi.trim_end_matches('!');

            // Look up the chapter title from the TOC (strip fragment from href for matching)
            let href_key = manifest_item
                .href
                .split('#')
                .next()
                .unwrap_or(&manifest_item.href);
            let title = title_map.get(href_key).cloned();

            let chapter_positions: Vec<crate::model::Position> = (0..position_count)
                .map(|p| {
                    // global_position is 1-based and continues monotonically across chapters.
                    // Formula: startPosition + p + 1  (identical to go-toolkit's createReflowable)
                    let global_position = last_position + p + 1;

                    // chapter_progression = p / position_count
                    // (0.0 for the first position in the chapter, approaching 1.0)
                    // Formula mirrors go-toolkit: `float64(p) / float64(positionCount)`
                    let chapter_progression = if position_count <= 1 {
                        0.0f32
                    } else {
                        p as f32 / position_count as f32
                    };

                    // CFI generation (without DOM parsing):
                    //   position 0 or fixed-layout → document root element (/4)
                    //   position N > 0             → /4/N*2 (even step = element, per CFI spec)
                    let cfi = if p == 0 || is_fixed {
                        format!("epubcfi({}!/4)", spine_path)
                    } else {
                        format!("epubcfi({}!/4/{})", spine_path, p * 2)
                    };

                    crate::model::Position {
                        spine_index: *spine_index,
                        href: manifest_item.href.clone(),
                        cfi,
                        global_position,
                        chapter_progression,
                        total_progression: 0.0, // filled in Pass 2
                        title: title.clone(),
                    }
                })
                .collect();

            last_position += position_count;
            result.push(chapter_positions);
        }

        // ── Pass 2: compute totalProgression ──────────────────────────────────
        // total_page_count = last global_position reached across all chapters.
        // Formula: (position - 1) / total_page_count  (identical to go-toolkit's computePositions)
        let total_page_count = last_position;
        if total_page_count > 0 {
            for chapter in &mut result {
                for loc in chapter.iter_mut() {
                    loc.total_progression =
                        (loc.global_position - 1) as f32 / total_page_count as f32;
                }
            }
        }

        Ok(result)
    }

    /// Returns a flat list of all reading positions across the entire EPUB.
    ///
    /// This is a convenience wrapper around [`positions_by_reading_order`] that flattens the
    /// per-chapter grouping. Mirrors go-toolkit's `Positions()`.
    ///
    /// `bytes_per_position` sets the granularity of reflowable positions.
    /// Pass `0` (or [`BYTES_PER_POSITION`]) to use the Readium/Adobe default of 1024 bytes.
    pub fn generate_locations(
        &mut self,
        book: &EpubBook,
        bytes_per_position: usize,
    ) -> Result<Vec<crate::model::Position>, EpubError> {
        let strategy = ArchiveEntryLength {
            page_length: if bytes_per_position == 0 {
                BYTES_PER_POSITION
            } else {
                bytes_per_position
            },
        };
        let by_chapter = self.positions_by_reading_order(book, &strategy)?;
        Ok(by_chapter.into_iter().flatten().collect())
    }


    /// Reads `META-INF/encryption.xml` to find obfuscated resources
    fn parse_encryption(
        &mut self,
    ) -> Result<std::collections::HashMap<String, crate::crypto::ObfuscationAlgorithm>, EpubError>
    {
        let mut encryptions = std::collections::HashMap::new();

        let mut enc_file = match self.provider.read_file("META-INF/encryption.xml") {
            Ok(f) => f,
            Err(_) => return Ok(encryptions), // Doesn't exist, which is fine
        };

        let mut buf = String::new();
        if enc_file.read_to_string(&mut buf).is_err() {
            return Ok(encryptions);
        }

        let mut reader = Reader::from_str(&buf);
        reader.config_mut().trim_text(true);

        let mut current_algo = None;
        let mut event_buf = Vec::new();

        loop {
            match reader.read_event_into(&mut event_buf) {
                Ok(Event::Empty(ref e)) | Ok(Event::Start(ref e)) => {
                    let name = String::from_utf8_lossy(e.name().into_inner()).into_owned();
                    if name.ends_with("EncryptionMethod") {
                        for attr in e.attributes() {
                            if let Ok(attr) = attr
                                && attr.key.as_ref() == b"Algorithm"
                            {
                                let val = String::from_utf8_lossy(&attr.value);
                                if val == "http://www.idpf.org/2008/embedding" {
                                    current_algo = Some(crate::crypto::ObfuscationAlgorithm::Idpf);
                                } else if val == "http://ns.adobe.com/pdf/enc#RC" {
                                    current_algo = Some(crate::crypto::ObfuscationAlgorithm::Adobe);
                                }
                            }
                        }
                    } else if name.ends_with("CipherReference") {
                        for attr in e.attributes() {
                            if let Ok(attr) = attr
                                && attr.key.as_ref() == b"URI"
                            {
                                let uri = String::from_utf8_lossy(&attr.value).into_owned();
                                // URL Decode URI (encryption.xml URIs are standard percent-encoded)
                                let decoded_uri = percent_encoding::percent_decode_str(&uri)
                                    .decode_utf8_lossy()
                                    .into_owned();
                                if let Some(algo) = current_algo {
                                    encryptions.insert(decoded_uri, algo);
                                }
                            }
                        }
                    }
                }
                Ok(Event::End(ref e)) => {
                    let name = String::from_utf8_lossy(e.name().into_inner()).into_owned();
                    if name.ends_with("EncryptedData") {
                        current_algo = None;
                    }
                }
                Ok(Event::Eof) => break,
                Err(_) => break, // Gracefully ignore encryption XML parsing errors
                _ => {}
            }
            event_buf.clear();
        }

        Ok(encryptions)
    }

    /// Reads `META-INF/container.xml` to find the paths of the OPF files
    fn parse_container(&mut self) -> Result<Vec<String>, EpubError> {
        let mut container_file = self
            .provider
            .read_file("META-INF/container.xml")
            .map_err(|_| EpubError::MissingContainer)?;

        let mut buf = String::new();
        container_file.read_to_string(&mut buf)?;

        let mut reader = Reader::from_str(&buf);
        reader.config_mut().trim_text(true);

        let mut rootfiles = Vec::new();
        let mut event_buf = Vec::new();

        loop {
            match reader.read_event_into(&mut event_buf)? {
                Event::Empty(ref e) | Event::Start(ref e) if e.name().as_ref() == b"rootfile" => {
                    for attr in e.attributes() {
                        let attr = attr?;
                        if attr.key.as_ref() == b"full-path" {
                            rootfiles.push(String::from_utf8_lossy(&attr.value).into_owned());
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            event_buf.clear();
        }

        if rootfiles.is_empty() {
            Err(EpubError::InvalidFormat(
                "No rootfile full-path found in container.xml".to_string(),
            ))
        } else {
            Ok(rootfiles)
        }
    }

    /// Parses the OPF file (usually .opf) to build the domain models
    fn parse_opf(&mut self, opf_path: &str) -> Result<EpubBook, EpubError> {
        let mut opf_file = self.provider.read_file(opf_path)?;
        let mut buf = String::new();
        opf_file.read_to_string(&mut buf)?;

        let mut reader = Reader::from_str(&buf);
        reader.config_mut().trim_text(true);

        // ... (rest of parse_opf will remain unchanged below)

        let mut book = EpubBook::default();
        if let Some(pos) = opf_path.rfind('/') {
            book.opf_dir = opf_path[..pos].to_string();
        } else {
            book.opf_dir = String::new();
        }
        let mut event_buf = Vec::new();

        // State tracking
        let mut in_metadata = false;
        let mut state = OpfState::None;

        // A temporary map to store metadata refinements (refines -> property -> value)
        let mut refinements: std::collections::HashMap<
            String,
            std::collections::HashMap<String, String>,
        > = std::collections::HashMap::new();
        // A temporary map connecting an ID to the index in the creators vector
        let mut creator_id_to_idx: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        // Maps dc:title element id -> its text, for EPUB 3 subtitle refinement lookup
        let mut title_id_to_text: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        // Tracks the id attribute of the current dc:title being parsed
        let mut current_title_id: Option<String> = None;
        // Collects (meta_id, collection_name) for belongs-to-collection post-processing
        let mut pending_collections: Vec<(String, String)> = Vec::new();

        loop {
            match reader.read_event_into(&mut event_buf)? {
                Event::Start(ref e) => {
                    let name = e.name();
                    let name_str = String::from_utf8_lossy(name.into_inner()).into_owned();

                    if name_str.ends_with("metadata") {
                        in_metadata = true;
                    } else if name_str.ends_with("spine") {
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.into_inner());
                            match key.as_ref() {
                                "toc" => {
                                    book.toc_id =
                                        Some(String::from_utf8_lossy(&attr.value).into_owned());
                                }
                                "page-progression-direction" => {
                                    if attr.value.as_ref() == b"rtl" {
                                        book.metadata.reading_progression =
                                            crate::model::ReadingProgression::Rtl;
                                    }
                                }
                                _ => {}
                            }
                        }
                    } else if name_str.ends_with("title") {
                        // Capture the element's id for EPUB 3 subtitle refinement
                        current_title_id = e
                            .attributes()
                            .flatten()
                            .find(|a| a.key.as_ref() == b"id")
                            .map(|a| String::from_utf8_lossy(&a.value).into_owned());
                        // Detect EPUB 2 subtitle via opf:title-type attribute
                        let title_type = e
                            .attributes()
                            .flatten()
                            .find(|a| {
                                let k = String::from_utf8_lossy(a.key.into_inner());
                                k.ends_with("title-type")
                            })
                            .map(|a| String::from_utf8_lossy(&a.value).into_owned());
                        match title_type.as_deref() {
                            Some("subtitle") => state = OpfState::Subtitle,
                            _ => state = OpfState::Title,
                        }
                    } else if name_str.ends_with("creator") || name_str.ends_with("contributor") {
                        let id = e
                            .attributes()
                            .flatten()
                            .find(|a| a.key.as_ref() == b"id")
                            .map(|a| String::from_utf8_lossy(&a.value).into_owned());
                        if name_str.ends_with("creator") {
                            state = OpfState::Creator(id);
                        } else {
                            state = OpfState::Contributor(id);
                        }
                    } else if name_str.ends_with("language") {
                        state = OpfState::Language;
                    } else if name_str.ends_with("identifier") {
                        state = OpfState::Identifier;
                    } else if name_str.ends_with("publisher") {
                        state = OpfState::Publisher;
                    } else if name_str.ends_with("description") {
                        state = OpfState::Description;
                    } else if name_str.ends_with("date") {
                        // EPUB 2: <dc:date opf:event="modification"> → map to `modified`
                        let event_attr = e
                            .attributes()
                            .flatten()
                            .find(|a| {
                                let k = String::from_utf8_lossy(a.key.into_inner());
                                k.ends_with("event")
                            })
                            .map(|a| String::from_utf8_lossy(&a.value).into_owned());
                        if event_attr.as_deref() == Some("modification") {
                            state = OpfState::Modified;
                        } else {
                            state = OpfState::Date;
                        }

                    } else if name_str.ends_with("rights") {
                        state = OpfState::Rights;
                    } else if name_str.ends_with("subject") {
                        state = OpfState::Subject;
                    } else if name_str.ends_with("meta") {
                        let mut refines = None;
                        let mut property = None;
                        let mut meta_id = None;
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.into_inner());
                            match key.as_ref() {
                                "refines" => {
                                    let val = String::from_utf8_lossy(&attr.value).into_owned();
                                    if let Some(stripped) = val.strip_prefix('#') {
                                        refines = Some(stripped.to_string());
                                    }
                                }
                                "property" => {
                                    property =
                                        Some(String::from_utf8_lossy(&attr.value).into_owned());
                                }
                                "id" => {
                                    meta_id =
                                        Some(String::from_utf8_lossy(&attr.value).into_owned());
                                }
                                _ => {}
                            }
                        }
                        if let (Some(r), Some(p)) = (refines, property.clone()) {
                            state = OpfState::MetaRefines {
                                ref_id: r,
                                property: p,
                            };
                        } else if let Some(p) = property {
                            state = OpfState::MetaGlobal {
                                property: p,
                                id: meta_id,
                            };
                        } else {
                            // Try EPUB 2 cover for `<meta name="cover" content="id"/>`
                            // though it's technically more correct to handle this in Event::Empty since meta is often self-closing
                            let mut is_cover = false;
                            let mut content = None;
                            for attr in e.attributes().flatten() {
                                let key = String::from_utf8_lossy(attr.key.into_inner());
                                let val = String::from_utf8_lossy(&attr.value).into_owned();
                                if key == "name" && val == "cover" {
                                    is_cover = true;
                                } else if key == "content" {
                                    content = Some(val);
                                }
                            }
                            if is_cover && content.is_some() {
                                book.metadata.cover_id = content;
                            }
                        }
                    }
                }
                Event::Empty(ref e) => {
                    let name = e.name();
                    let name_str = String::from_utf8_lossy(name.into_inner()).into_owned();

                    if name_str.ends_with("meta") && in_metadata {
                        let mut is_cover = false;
                        let mut content = None;
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.into_inner());
                            let val = String::from_utf8_lossy(&attr.value).into_owned();
                            if key == "name" && val == "cover" {
                                is_cover = true;
                            } else if key == "content" {
                                content = Some(val);
                            }
                        }
                        if is_cover && content.is_some() {
                            book.metadata.cover_id = content;
                        }
                    } else if name_str.ends_with("item") {
                        // Extract manifest item
                        let mut id = String::new();
                        let mut href = String::new();
                        let mut media_type = String::new();
                        let mut properties = Vec::new();

                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = String::from_utf8_lossy(attr.key.into_inner());
                            let value = String::from_utf8_lossy(&attr.value).into_owned();

                            match key.as_ref() {
                                "id" => id = value,
                                "href" => {
                                    // URL Decode the href as EPUB paths are URI encoded (e.g. "chapter%201.xhtml")
                                    let decoded = percent_encoding::percent_decode_str(&value)
                                        .decode_utf8_lossy()
                                        .into_owned();
                                    href = decoded;
                                }
                                "media-type" => media_type = value,
                                "properties" => {
                                    properties =
                                        value.split_whitespace().map(|s| s.to_string()).collect()
                                }
                                _ => {}
                            }
                        }

                        if !id.is_empty() && !href.is_empty() {
                            book.manifest.insert(
                                id.clone(),
                                ManifestItem {
                                    id,
                                    href,
                                    media_type,
                                    properties,
                                },
                            );
                        }
                    } else if name_str.ends_with("itemref") {
                        // Extract spine reading order
                        let mut idref = String::new();
                        let mut linear = true;
                        let mut layout_override = None;
                        let mut page_spread = None;

                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = String::from_utf8_lossy(attr.key.into_inner());
                            let val = String::from_utf8_lossy(&attr.value).into_owned();
                            if key == "idref" {
                                idref = val;
                            } else if key == "linear" {
                                linear = val != "no";
                            } else if key == "properties" {
                                if val.contains("rendition:layout-reflowable") {
                                    layout_override = Some(crate::model::LayoutType::Reflowable);
                                } else if val.contains("rendition:layout-pre-paginated") {
                                    layout_override = Some(crate::model::LayoutType::PrePaginated);
                                }

                                if val.contains("page-spread-left")
                                    || val.contains("rendition:page-spread-left")
                                {
                                    page_spread = Some(crate::model::PageSpread::Left);
                                } else if val.contains("page-spread-right")
                                    || val.contains("rendition:page-spread-right")
                                {
                                    page_spread = Some(crate::model::PageSpread::Right);
                                } else if val.contains("rendition:page-spread-center") {
                                    page_spread = Some(crate::model::PageSpread::Center);
                                }
                            }
                        }

                        if !idref.is_empty() {
                            book.spine.push(crate::model::SpineItem {
                                idref,
                                linear,
                                layout_override,
                                page_spread,
                            });
                        }
                    }
                }
                Event::Text(e) if in_metadata => {
                    let text = String::from_utf8_lossy(&e).into_owned();
                    if text.trim().is_empty() {
                        continue;
                    }

                    match &state {
                        OpfState::Title => {
                            // Record id -> text mapping for EPUB 3 subtitle refinement
                            if let Some(tid) = current_title_id.take() {
                                title_id_to_text.insert(tid, text.clone());
                            }
                            // Only set the main title if not yet populated
                            if book.metadata.title.is_none() {
                                book.metadata.title = Some(text);
                            }
                        }
                        OpfState::Subtitle => {
                            book.metadata.subtitle = Some(text);
                        }
                        OpfState::Creator(id) => {
                            let creator = crate::model::Creator::new(&text);
                            if let Some(id_str) = id {
                                creator_id_to_idx
                                    .insert(id_str.clone(), book.metadata.creators.len());
                            }
                            book.metadata.creators.push(creator);
                        }
                        OpfState::Contributor(id) => {
                            let creator = crate::model::Creator::new(&text);
                            if let Some(id_str) = id {
                                creator_id_to_idx
                                    .insert(id_str.clone(), book.metadata.creators.len());
                            }
                            book.metadata.creators.push(creator);
                        }
                        OpfState::Language => {
                            // Append to languages Vec; also keep `language` for backward compat
                            book.metadata.languages.push(text.clone());
                            if book.metadata.language.is_none() {
                                book.metadata.language = Some(text);
                            }
                        }
                        OpfState::Identifier => book.metadata.identifier = Some(text),
                        OpfState::Publisher => book.metadata.publisher = Some(text),
                        OpfState::Description => book.metadata.description = Some(text),
                        OpfState::Date => book.metadata.date = Some(text),
                        OpfState::Modified => book.metadata.modified = Some(text),
                        OpfState::Rights => book.metadata.rights = Some(text),
                        OpfState::Subject => book.metadata.subjects.push(text),
                        OpfState::MetaRefines { ref_id, property } => {
                            let entry = refinements.entry(ref_id.clone()).or_default();
                            entry.insert(property.clone(), text);
                        }
                        OpfState::MetaGlobal { property, id } => {
                            match property.as_str() {
                                "rendition:layout" => {
                                    if text == "pre-paginated" {
                                        book.metadata.layout =
                                            crate::model::LayoutType::PrePaginated;
                                    } else {
                                        book.metadata.layout =
                                            crate::model::LayoutType::Reflowable;
                                    }
                                }
                                "dcterms:modified" => {
                                    book.metadata.modified = Some(text);
                                }
                                "belongs-to-collection" => {
                                    if let Some(mid) = id {
                                        pending_collections.push((mid.clone(), text));
                                    }
                                }
                                _ => {}
                            }
                        }
                        OpfState::None => {}
                    }
                    // Reset state and title-id tracker after consuming text
                    current_title_id = None;
                    state = OpfState::None;
                }
                Event::End(ref e) => {
                    let name = e.name();
                    let name_str = String::from_utf8_lossy(name.into_inner());
                    if name_str.ends_with("metadata") {
                        in_metadata = false;
                    }
                    state = OpfState::None;
                }
                Event::Eof => break,
                _ => {}
            }
            event_buf.clear();
        }

        // ── Post-process refinements ──────────────────────────────────────────────

        // 1. Apply creator role / file-as from refinements
        for (id, props) in &refinements {
            if let Some(&idx) = creator_id_to_idx.get(id)
                && let Some(creator) = book.metadata.creators.get_mut(idx)
            {
                if let Some(role) = props.get("role") {
                    creator.role = Some(role.clone());
                }
                if let Some(file_as) = props.get("file-as") {
                    creator.file_as = Some(file_as.clone());
                }
            }
        }

        // 2. Resolve EPUB 3 subtitle and sort-as from title refinements
        for (id, props) in &refinements {
            if title_id_to_text.contains_key(id.as_str()) {
                // Subtitle
                if let Some(title_type) = props.get("title-type") {
                    if title_type == "subtitle" {
                        if let Some(txt) = title_id_to_text.get(id) {
                            book.metadata.subtitle = Some(txt.clone());
                            // If the main title was set to this same text (only one dc:title),
                            // clear it so the caller knows there is no separate main title.
                            // In practice, EPUBs with a subtitle always declare a separate main
                            // dc:title, so this branch is a safety guard only.
                            if book.metadata.title.as_deref() == Some(txt.as_str()) {
                                book.metadata.title = None;
                            }
                        }
                    }
                }
                // Sort-as (file-as refining a title element)
                if book.metadata.sort_as.is_none() {
                    if let Some(sort) = props.get("file-as") {
                        book.metadata.sort_as = Some(sort.clone());
                    }
                }
            }
        }

        // 3. Resolve belongs-to-collection with its refinements
        for (meta_id, name) in pending_collections {
            let props = refinements.get(&meta_id);
            let collection_type = props
                .and_then(|p| p.get("collection-type"))
                .cloned()
                .unwrap_or_else(|| "series".to_string());
            let position = props
                .and_then(|p| p.get("group-position"))
                .and_then(|s| s.parse::<f64>().ok());
            book.metadata.belongs_to.push(crate::model::BelongsTo {
                name,
                collection_type,
                position,
            });
        }

        Ok(book)
    }

    /// Get a readable stream for a resource given its manifest href
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

    /// Normalizes an EPUB path by resolving `.` and `..` relative segments.
    fn normalize_path(base: &str, href: &str) -> String {
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

    /// Get a readable stream for a resource given its manifest ID
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

    /// Read the raw bytes of a resource from the archive given its manifest href
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

    /// Helper to get a resource by its manifest ID
    pub fn get_resource_by_id(&mut self, book: &EpubBook, id: &str) -> Result<Vec<u8>, EpubError> {
        let mut file = self.read_resource_by_id(book, id)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// Smart API to find and extract the cover image of the EPUB.
    /// Returns the bytes of the image and its media_type (e.g., "image/jpeg").
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

    /// Generates a list of synthetic reading positions (virtual pages) for the entire EPUB.
    /// This is crucial for computing reading progress and providing a unified pagination experience
    /// across different screen sizes.
    ///
    /// `chars_per_position` is the number of characters that constitute a single "position" (typically 1024).
    pub fn get_positions(
        &mut self,
        book: &EpubBook,
        chars_per_position: usize,
    ) -> Result<Vec<Position>, EpubError> {
        let mut all_positions = Vec::new();
        let mut global_pos = 0;
        let mut char_counter = 0;

        for (i, item) in book.spine.iter().enumerate() {
            if !item.linear {
                continue; // Skip supplementary chapters for global progression
            }

            let base_cfi = crate::cfi::EpubCfi::generate_spine_base_cfi(i, &item.idref);

            // Get href for the manifest item
            let href = book
                .manifest
                .get(&item.idref)
                .map(|m| m.href.clone())
                .unwrap_or_default();

            // Always add the chapter start position
            global_pos += 1;

            let mut stripped_base = base_cfi.clone();
            if stripped_base.ends_with('!') {
                stripped_base.pop();
            }

            let mut chapter_positions = vec![Position {
                spine_index: i,
                href: href.clone(),
                cfi: format!("epubcfi({}!/4)", stripped_base), // pointing roughly to body
                global_position: global_pos,
                chapter_progression: 0.0,
                total_progression: 0.0,
                title: None,
            }];

            // Read the chapter HTML
            if let Ok(raw_html) = self.get_resource_by_id(book, &item.idref) {
                let html_str = String::from_utf8_lossy(&raw_html);

                let ctx = crate::processor::PositionContext {
                    base_cfi: &base_cfi,
                    chars_per_position,
                    spine_index: i,
                    href: &href,
                };

                crate::processor::extract_positions(
                    &html_str,
                    &ctx,
                    &mut char_counter,
                    &mut chapter_positions,
                    &mut global_pos,
                );
            }

            // Update chapter progressions
            let total_in_chapter = chapter_positions.len();
            for (idx, pos) in chapter_positions.iter_mut().enumerate() {
                pos.chapter_progression = idx as f32 / total_in_chapter as f32;
            }

            all_positions.extend(chapter_positions);
        }

        // Update total progression
        let total_positions = all_positions.len();
        if total_positions > 0 {
            for pos in all_positions.iter_mut() {
                // Progression from 0.0 to 1.0 based on position index
                pos.total_progression =
                    (pos.global_position - 1) as f32 / (total_positions.max(1)) as f32;
            }
        }

        Ok(all_positions)
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

    // ── Navigation ────────────────────────────────────────────────────────────

    /// Parses **all** navigation data from the EPUB in a **single I/O + single parse** operation.
    ///
    /// Returns a [`NavigationDocument`] containing:
    /// - `toc`       — Table of Contents
    /// - `page_list` — Print page → document position mapping
    /// - `landmarks` — Structural navigation points (EPUB 3 only)
    ///
    /// Priority (identical to go-toolkit):
    /// 1. EPUB 3 `nav.xhtml` → scans **all** `<nav epub:type="…">` elements in one DOM pass
    /// 2. EPUB 2 `.ncx`      → parses `<navMap>` (toc) + `<pageList>` (page-list) in one pass
    ///
    /// Calling both `get_toc()` and `get_page_list()` separately would read and parse the file
    /// twice; use this method when you need more than one navigation list.
    pub fn get_navigation(&mut self, book: &EpubBook) -> Result<NavigationDocument, EpubError> {
        // 1. Prefer EPUB 3 nav.xhtml — one read, all types
        if let Some(nav_item) = book
            .manifest
            .values()
            .find(|i| i.properties.iter().any(|p| p == "nav"))
        {
            let bytes = self.get_resource_by_id(book, &nav_item.id)?;
            let html = String::from_utf8_lossy(&bytes).to_string();
            return Self::parse_nav_xhtml_all(&html);
        }

        // 2. Fallback to EPUB 2 NCX — one read, toc + page-list
        if let Some(toc_id) = &book.toc_id
            && let Some(ncx_item) = book.manifest.get(toc_id)
        {
            let bytes = self.get_resource_by_id(book, &ncx_item.id)?;
            let xml = String::from_utf8_lossy(&bytes).to_string();
            return Self::parse_ncx_all(&xml);
        }

        Ok(NavigationDocument::default())
    }

    /// Returns the Table of Contents.
    ///
    /// This is a convenience wrapper around [`get_navigation`] that reads the nav file
    /// once and returns only `navigation.toc`. Prefer [`get_navigation`] when you also
    /// need the page list or landmarks, to avoid reading the file twice.
    pub fn get_toc(&mut self, book: &EpubBook) -> Result<Vec<TocEntry>, EpubError> {
        Ok(self.get_navigation(book)?.toc)
    }

    /// Returns the Page List (`epub:type="page-list"` or NCX `<pageList>`).
    ///
    /// Each returned `TocEntry` has:
    /// - `title` = page label as printed (`"1"`, `"42"`, `"xii"`, `"A-3"`)
    /// - `href`  = document position, typically with a fragment (`"ch3.xhtml#p42"`)
    /// - `children` = always empty (page lists are flat)
    ///
    /// Returns `Ok(Vec::new())` if no page list is present (page lists are optional
    /// per the EPUB specification and not present in most EPUBs).
    ///
    /// Convenience wrapper around [`get_navigation`].
    pub fn get_page_list(&mut self, book: &EpubBook) -> Result<Vec<TocEntry>, EpubError> {
        Ok(self.get_navigation(book)?.page_list)
    }

    /// Parse a `nav.xhtml` document, extracting **all** `<nav epub:type="…">` elements
    /// in a single DOM traversal.
    ///
    /// Mirrors go-toolkit `ParseNavDoc`:
    /// ```go
    /// for _, nav := range body.SelectElements("//nav") {
    ///     types, links := parseNavElement(nav, ...)
    ///     ret[type] = links   // collects ALL types in one loop
    /// }
    /// ```
    ///
    /// The same `parse_ol_node` method is reused for every nav type (toc, page-list,
    /// landmarks) — no duplication.
    fn parse_nav_xhtml_all(html: &str) -> Result<NavigationDocument, EpubError> {
        let document = kuchikiki::parse_html().one(html);
        let mut nav_doc = NavigationDocument::default();

        // Iterate ALL <nav> elements (one DOM parse, multiple results)
        let nav_nodes = document.select("nav").unwrap_or_else(|_| {
            // select() only errors on invalid CSS; "nav" is always valid
            panic!("'nav' is a valid CSS selector")
        });

        for nav in nav_nodes {
            // Read epub:type attribute. kuchikiki stores the attribute name
            // verbatim; the colon is escaped in CSS but stored as-is in attrs.
            let attrs = nav.attributes.borrow();
            let epub_type = attrs
                .get("epub:type")
                .unwrap_or("")
                .to_string();
            drop(attrs);

            if epub_type.is_empty() {
                continue;
            }

            // Parse the <ol> — identical method for ALL nav types.
            // This is the key reuse: parse_ol_node is called once per nav element.
            let entries = match nav.as_node().select_first("ol") {
                Ok(ol) => Self::parse_ol_node(ol.as_node()),
                Err(_) => continue,
            };

            if entries.is_empty() {
                continue;
            }

            // epub:type may contain multiple space-separated tokens per EPUB spec.
            // We handle each token — e.g. `epub:type="toc landmarks"`.
            for token in epub_type.split_whitespace() {
                // Strip any "epub:" namespace prefix that may appear in the attribute value
                let key = token.trim_start_matches("epub:");
                match key {
                    "toc"       => nav_doc.toc       = entries.clone(),
                    "page-list" => nav_doc.page_list = entries.clone(),
                    "landmarks" => nav_doc.landmarks = entries.clone(),
                    _           => {} // ignore unknown types (forward-compatible)
                }
            }
        }

        // Fallback for malformed nav.xhtml that lacks epub:type:
        // if TOC is still empty, try <nav id="toc"> then first <nav>.
        if nav_doc.toc.is_empty() {
            let toc_node = document
                .select_first("nav#toc")
                .or_else(|_| document.select_first("nav"));
            if let Ok(nav) = toc_node {
                if let Ok(ol) = nav.as_node().select_first("ol") {
                    nav_doc.toc = Self::parse_ol_node(ol.as_node());
                }
            }
        }

        Ok(nav_doc)
    }

    /// Parse an `<ol>` element into a flat or nested list of [`TocEntry`] items.
    ///
    /// Used uniformly for every nav type: TOC (with nesting), page-list (flat),
    /// and landmarks (flat). This is the single shared implementation.
    fn parse_ol_node(ol: &kuchikiki::NodeRef) -> Vec<TocEntry> {
        let mut entries = Vec::new();
        // Since `select` might grab deep children, we manually iterate direct children.
        for li in ol.children().filter(|c| {
            c.as_element()
                .is_some_and(|e| e.name.local.as_ref() == "li")
        }) {
            if let Ok(a_node) = li.select_first("a") {
                let raw_href = a_node
                    .attributes
                    .borrow()
                    .get("href")
                    .unwrap_or("")
                    .to_string();
                let href = percent_encoding::percent_decode_str(&raw_href)
                    .decode_utf8_lossy()
                    .into_owned();
                let title = a_node.text_contents().trim().to_string();

                let mut entry = TocEntry::new(title, href);

                // Recursively parse nested <ol> — used by TOC; ignored for page-list/landmarks
                if let Ok(nested_ol) = li.select_first("ol") {
                    entry.children = Self::parse_ol_node(nested_ol.as_node());
                }
                entries.push(entry);
            }
        }
        entries
    }

    /// Parse an NCX document, extracting both `<navMap>` (TOC) and `<pageList>` (page-list)
    /// in a **single streaming pass** over the XML.
    ///
    /// Mirrors go-toolkit `ParseNCX`:
    /// ```go
    /// toc      := document.SelectElement("//navMap")
    /// pageList := document.SelectElement("//pageList")
    /// ret["toc"]       = parseNavMapElement(toc)
    /// ret["page-list"] = parsePageListElement(pageList)
    /// ```
    ///
    /// State machine regions:
    /// - Default scope → `navPoint` stack → TOC
    /// - `in_page_list` scope → `pageTarget` → page-list entries
    fn parse_ncx_all(xml: &str) -> Result<NavigationDocument, EpubError> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        /// Shared state for both navPoint and pageTarget accumulation.
        #[derive(Debug, Clone)]
        struct EntryState {
            title: String,
            href: String,
            children: Vec<TocEntry>,
        }

        let mut stack: Vec<EntryState> = Vec::new();
        let mut toc_entries: Vec<TocEntry> = Vec::new();
        let mut page_list: Vec<TocEntry> = Vec::new();

        // Region flags — mutually exclusive during parsing
        let mut in_page_list = false;
        let mut in_text = false;
        // pageTarget accumulator (only used when in_page_list)
        let mut page_target: Option<EntryState> = None;

        let mut event_buf = Vec::new();

        loop {
            match reader.read_event_into(&mut event_buf)? {
                Event::Start(ref e) => {
                    let name = String::from_utf8_lossy(e.name().into_inner()).into_owned();

                    if name.ends_with("pageList") {
                        // Enter page-list region
                        in_page_list = true;
                    } else if in_page_list && name.ends_with("pageTarget") {
                        // Start accumulating a page target entry
                        page_target = Some(EntryState {
                            title: String::new(),
                            href: String::new(),
                            children: Vec::new(), // always empty for page-list
                        });
                    } else if !in_page_list && name.ends_with("navPoint") {
                        // Push a new navPoint onto the TOC stack
                        stack.push(EntryState {
                            title: String::new(),
                            href: String::new(),
                            children: Vec::new(),
                        });
                    } else if name.ends_with("text") {
                        in_text = true;
                    }
                }

                Event::Empty(ref e) => {
                    let name = String::from_utf8_lossy(e.name().into_inner()).into_owned();
                    if name.ends_with("content") {
                        // <content src="…"/> — present in both navPoint and pageTarget
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"src" {
                                let raw = String::from_utf8_lossy(&attr.value);
                                let href = percent_encoding::percent_decode_str(&raw)
                                    .decode_utf8_lossy()
                                    .into_owned();

                                if in_page_list {
                                    if let Some(ref mut pt) = page_target {
                                        pt.href = href;
                                    }
                                } else if let Some(state) = stack.last_mut() {
                                    state.href = href;
                                }
                            }
                        }
                    }
                }

                Event::Text(e) => {
                    if in_text {
                        let text = String::from_utf8_lossy(&e).into_owned();
                        if in_page_list {
                            if let Some(ref mut pt) = page_target {
                                pt.title = text;
                            }
                        } else if let Some(state) = stack.last_mut() {
                            state.title = text;
                        }
                    }
                }

                Event::End(ref e) => {
                    let name = String::from_utf8_lossy(e.name().into_inner()).into_owned();

                    if name.ends_with("text") {
                        in_text = false;
                    } else if name.ends_with("pageList") {
                        // Exit page-list region
                        in_page_list = false;
                    } else if in_page_list && name.ends_with("pageTarget") {
                        // Commit a page-list entry — only if both title and href are present
                        if let Some(pt) = page_target.take() {
                            if !pt.title.is_empty() && !pt.href.is_empty() {
                                page_list.push(TocEntry {
                                    title: pt.title,
                                    href: pt.href,
                                    children: Vec::new(),
                                });
                            }
                        }
                    } else if !in_page_list
                        && name.ends_with("navPoint")
                        && let Some(state) = stack.pop()
                    {
                        // Commit a TOC entry (with any accumulated children)
                        let entry = TocEntry {
                            title: state.title,
                            href: state.href,
                            children: state.children,
                        };
                        if let Some(parent) = stack.last_mut() {
                            parent.children.push(entry);
                        } else {
                            toc_entries.push(entry);
                        }
                    }
                }

                Event::Eof => break,
                _ => {}
            }
            event_buf.clear();
        }

        Ok(NavigationDocument {
            toc: toc_entries,
            page_list,
            landmarks: Vec::new(), // NCX does not support landmarks
        })
    }
}

/// Builds a flat `href → title` lookup map from a TOC entry tree.
///
/// Used by [`EpubArchive::positions_by_reading_order`] to enrich each `Position`
/// with the chapter title from the table of contents.
///
/// The href key has any fragment suffix (`#anchor`) stripped so it matches the
/// bare file path stored in the manifest.
fn build_title_map(toc: &[TocEntry]) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    fn recurse(
        entries: &[TocEntry],
        map: &mut std::collections::HashMap<String, String>,
    ) {
        for entry in entries {
            // Strip fragment so `chapter.xhtml#section1` matches `chapter.xhtml`
            let key = entry
                .href
                .split('#')
                .next()
                .unwrap_or(&entry.href)
                .to_string();
            // Only set the first title seen for a given href (most specific wins for TOC order)
            map.entry(key).or_insert_with(|| entry.title.clone());
            recurse(&entry.children, map);
        }
    }
    recurse(toc, &mut map);
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestArchive = EpubArchive<crate::provider::ZipProvider<std::io::Cursor<Vec<u8>>>>;

    // ── NCX tests ─────────────────────────────────────────────────────────────

    #[test]
    fn test_parse_ncx_toc() {
        let xml = r#"
        <?xml version="1.0" encoding="UTF-8"?>
        <ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
          <navMap>
            <navPoint id="navPoint-1" playOrder="1">
              <navLabel><text>Chapter 1</text></navLabel>
              <content src="ch1.xhtml"/>
              <navPoint id="navPoint-2" playOrder="2">
                <navLabel><text>Chapter 1.1</text></navLabel>
                <content src="ch1_1.xhtml"/>
              </navPoint>
            </navPoint>
            <navPoint id="navPoint-3" playOrder="3">
              <navLabel><text>Chapter 2</text></navLabel>
              <content src="ch2.xhtml"/>
            </navPoint>
          </navMap>
        </ncx>
        "#;

        let nav = TestArchive::parse_ncx_all(xml).unwrap();

        assert_eq!(nav.toc.len(), 2); // Chapter 1, Chapter 2
        assert!(nav.page_list.is_empty()); // no page list in this NCX
        assert!(nav.landmarks.is_empty()); // NCX never has landmarks

        // Chapter 1
        assert_eq!(nav.toc[0].title, "Chapter 1");
        assert_eq!(nav.toc[0].href, "ch1.xhtml");
        assert_eq!(nav.toc[0].children.len(), 1); // Chapter 1.1 nested

        // Chapter 1.1
        assert_eq!(nav.toc[0].children[0].title, "Chapter 1.1");
        assert_eq!(nav.toc[0].children[0].href, "ch1_1.xhtml");

        // Chapter 2
        assert_eq!(nav.toc[1].title, "Chapter 2");
        assert_eq!(nav.toc[1].href, "ch2.xhtml");
    }

    #[test]
    fn test_parse_ncx_page_list() {
        let xml = r#"
        <?xml version="1.0" encoding="UTF-8"?>
        <ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
          <navMap>
            <navPoint id="np1" playOrder="1">
              <navLabel><text>Chapter 1</text></navLabel>
              <content src="ch1.xhtml"/>
            </navPoint>
          </navMap>
          <pageList>
            <pageTarget type="normal" playOrder="1">
              <navLabel><text>1</text></navLabel>
              <content src="ch1.xhtml#p1"/>
            </pageTarget>
            <pageTarget type="normal" playOrder="2">
              <navLabel><text>42</text></navLabel>
              <content src="ch3.xhtml#p42"/>
            </pageTarget>
            <pageTarget type="front" playOrder="3">
              <navLabel><text>xii</text></navLabel>
              <content src="front.xhtml#pxii"/>
            </pageTarget>
          </pageList>
        </ncx>
        "#;

        let nav = TestArchive::parse_ncx_all(xml).unwrap();

        // TOC unaffected
        assert_eq!(nav.toc.len(), 1);
        assert_eq!(nav.toc[0].title, "Chapter 1");

        // Page list
        assert_eq!(nav.page_list.len(), 3);
        assert_eq!(nav.page_list[0].title, "1");
        assert_eq!(nav.page_list[0].href, "ch1.xhtml#p1");
        assert!(nav.page_list[0].children.is_empty()); // always flat

        assert_eq!(nav.page_list[1].title, "42");
        assert_eq!(nav.page_list[1].href, "ch3.xhtml#p42");

        // Non-numeric page labels (roman numerals)
        assert_eq!(nav.page_list[2].title, "xii");
        assert_eq!(nav.page_list[2].href, "front.xhtml#pxii");
    }

    #[test]
    fn test_parse_ncx_page_list_requires_both_title_and_href() {
        // pageTarget without navLabel or content src should be skipped
        let xml = r#"
        <?xml version="1.0" encoding="UTF-8"?>
        <ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
          <navMap/>
          <pageList>
            <pageTarget type="normal">
              <navLabel><text>1</text></navLabel>
              <!-- no content src -->
            </pageTarget>
            <pageTarget type="normal">
              <!-- no navLabel -->
              <content src="ch1.xhtml#p2"/>
            </pageTarget>
            <pageTarget type="normal">
              <navLabel><text>3</text></navLabel>
              <content src="ch1.xhtml#p3"/>
            </pageTarget>
          </pageList>
        </ncx>
        "#;

        let nav = TestArchive::parse_ncx_all(xml).unwrap();
        // Only the third entry has both title and href
        assert_eq!(nav.page_list.len(), 1);
        assert_eq!(nav.page_list[0].title, "3");
    }

    // ── Nav XHTML tests ───────────────────────────────────────────────────────

    #[test]
    fn test_parse_nav_xhtml_toc_only() {
        let html = r#"<!DOCTYPE html>
        <html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
        <body>
          <nav epub:type="toc">
            <ol>
              <li><a href="ch1.xhtml">Chapter 1</a></li>
              <li><a href="ch2.xhtml">Chapter 2</a>
                <ol><li><a href="ch2s1.xhtml">Section 2.1</a></li></ol>
              </li>
            </ol>
          </nav>
        </body>
        </html>"#;

        let nav = TestArchive::parse_nav_xhtml_all(html).unwrap();

        assert_eq!(nav.toc.len(), 2);
        assert!(nav.page_list.is_empty());
        assert!(nav.landmarks.is_empty());

        assert_eq!(nav.toc[0].title, "Chapter 1");
        assert_eq!(nav.toc[0].href, "ch1.xhtml");
        assert_eq!(nav.toc[0].children.len(), 0);

        assert_eq!(nav.toc[1].title, "Chapter 2");
        assert_eq!(nav.toc[1].children.len(), 1);
        assert_eq!(nav.toc[1].children[0].title, "Section 2.1");
    }

    #[test]
    fn test_parse_nav_xhtml_toc_and_page_list() {
        // The most common real-world case: both toc and page-list in one nav.xhtml
        let html = r#"<!DOCTYPE html>
        <html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
        <body>
          <nav epub:type="toc">
            <ol>
              <li><a href="ch1.xhtml">Chapter 1</a></li>
            </ol>
          </nav>
          <nav epub:type="page-list">
            <ol>
              <li><a href="ch1.xhtml#p1">1</a></li>
              <li><a href="ch1.xhtml#p2">2</a></li>
              <li><a href="ch2.xhtml#p42">42</a></li>
            </ol>
          </nav>
        </body>
        </html>"#;

        let nav = TestArchive::parse_nav_xhtml_all(html).unwrap();

        // TOC
        assert_eq!(nav.toc.len(), 1);
        assert_eq!(nav.toc[0].title, "Chapter 1");

        // Page list — uses the SAME parse_ol_node method as TOC
        assert_eq!(nav.page_list.len(), 3);
        assert_eq!(nav.page_list[0].title, "1");
        assert_eq!(nav.page_list[0].href, "ch1.xhtml#p1");
        assert!(nav.page_list[0].children.is_empty()); // always flat

        assert_eq!(nav.page_list[2].title, "42");
        assert_eq!(nav.page_list[2].href, "ch2.xhtml#p42");
    }

    #[test]
    fn test_parse_nav_xhtml_landmarks() {
        let html = r#"<!DOCTYPE html>
        <html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
        <body>
          <nav epub:type="toc"><ol><li><a href="ch1.xhtml">Ch1</a></li></ol></nav>
          <nav epub:type="landmarks">
            <ol>
              <li><a href="cover.xhtml" epub:type="cover">Cover</a></li>
              <li><a href="toc.xhtml" epub:type="toc">Table of Contents</a></li>
              <li><a href="ch1.xhtml" epub:type="bodymatter">Begin Reading</a></li>
            </ol>
          </nav>
        </body>
        </html>"#;

        let nav = TestArchive::parse_nav_xhtml_all(html).unwrap();

        assert_eq!(nav.toc.len(), 1);
        assert!(nav.page_list.is_empty());
        assert_eq!(nav.landmarks.len(), 3);
        assert_eq!(nav.landmarks[0].title, "Cover");
        assert_eq!(nav.landmarks[0].href, "cover.xhtml");
        assert_eq!(nav.landmarks[2].title, "Begin Reading");
    }

    #[test]
    fn test_parse_nav_xhtml_all_three_types() {
        // Single nav.xhtml with TOC + page-list + landmarks — parsed in one pass
        let html = r#"<!DOCTYPE html>
        <html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
        <body>
          <nav epub:type="toc">
            <ol><li><a href="ch1.xhtml">Chapter 1</a></li></ol>
          </nav>
          <nav epub:type="page-list">
            <ol><li><a href="ch1.xhtml#p1">1</a></li></ol>
          </nav>
          <nav epub:type="landmarks">
            <ol><li><a href="ch1.xhtml">Begin Reading</a></li></ol>
          </nav>
        </body>
        </html>"#;

        let nav = TestArchive::parse_nav_xhtml_all(html).unwrap();

        assert_eq!(nav.toc.len(), 1);
        assert_eq!(nav.page_list.len(), 1);
        assert_eq!(nav.landmarks.len(), 1);
        assert!(!nav.is_empty());
    }

    #[test]
    fn test_parse_nav_xhtml_fallback_no_epub_type() {
        // Malformed nav.xhtml with no epub:type — falls back to first <nav>
        let html = r#"<!DOCTYPE html>
        <html>
        <body>
          <nav>
            <ol>
              <li><a href="ch1.xhtml">Chapter 1</a></li>
            </ol>
          </nav>
        </body>
        </html>"#;

        let nav = TestArchive::parse_nav_xhtml_all(html).unwrap();

        // TOC populated via fallback; page_list and landmarks remain empty
        assert_eq!(nav.toc.len(), 1);
        assert_eq!(nav.toc[0].title, "Chapter 1");
        assert!(nav.page_list.is_empty());
    }

    #[test]
    fn test_navigation_document_is_empty() {
        let empty = NavigationDocument::default();
        assert!(empty.is_empty());

        let mut nav = NavigationDocument::default();
        nav.toc.push(TocEntry::new("Ch1", "ch1.xhtml"));
        assert!(!nav.is_empty());
    }
}
