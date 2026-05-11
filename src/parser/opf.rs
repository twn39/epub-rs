//! OPF package document, container, and encryption parsing.
//!
//! Handles:
//! - `META-INF/container.xml` → rootfile paths
//! - `META-INF/encryption.xml` → obfuscated resource map
//! - `*.opf` → full `EpubBook` (metadata, manifest, spine)

use crate::error::EpubError;
use crate::model::{EpubBook, ManifestItem};
use crate::provider::EpubProvider;
use quick_xml::Reader;
use quick_xml::events::Event;
use std::io::Read;

use super::EpubArchive;

// ── OPF state machine ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(super) enum OpfState {
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
    MetaRefines {
        ref_id: String,
        property: String,
    },
    /// Global `<meta property="...">` — carries the element's own `id` for refinement lookup
    MetaGlobal {
        property: String,
        id: Option<String>,
    },
}

// ── RawTitle (parse-phase intermediate) ──────────────────────────────────────

/// Collected during streaming parse; refined into [`TitleEntry`] in post-processing.
struct RawTitle {
    /// Value of the element's `id` attribute (for refinement lookup).
    id: Option<String>,
    /// BCP-47 language tag from `xml:lang`.
    lang: Option<String>,
    /// Text content.
    text: String,
    /// `true` when the EPUB 2 `opf:title-type="subtitle"` inline attribute
    /// was detected (before EPUB 3 refinements are applied).
    epub2_subtitle: bool,
}

// ── EpubArchive impl ────────────────────────────────────────────────────

impl<P: EpubProvider> EpubArchive<P> {
    /// Reads `META-INF/container.xml` to find the paths of the OPF files.
    pub(super) fn parse_container(&mut self) -> Result<Vec<String>, EpubError> {
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

    /// Reads `META-INF/encryption.xml` to find obfuscated/encrypted resources.
    ///
    /// Returns a map from ZIP-relative path to [`crate::crypto::EncryptionInfo`],
    /// which carries both the obfuscation algorithm and the optional original
    /// plaintext length from `<Compression OriginalLength="N">`.
    pub(super) fn parse_encryption(
        &mut self,
    ) -> Result<std::collections::HashMap<String, crate::crypto::EncryptionInfo>, EpubError>
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
        // Original plaintext length from <Compression OriginalLength="N">.
        // Only present for LCP/AES full-content encryption; absent for IDPF/Adobe font obfuscation.
        let mut current_original_length: Option<u64> = None;
        let mut current_uri: Option<String> = None;
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
                                    current_algo =
                                        Some(crate::crypto::ObfuscationAlgorithm::Idpf);
                                } else if val == "http://ns.adobe.com/pdf/enc#RC" {
                                    current_algo =
                                        Some(crate::crypto::ObfuscationAlgorithm::Adobe);
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
                                current_uri = Some(decoded_uri);
                            }
                        }
                    } else if name.ends_with("Compression") {
                        // <comp:Compression Method="8" OriginalLength="13291">
                        // Method 8 = deflate was applied before encryption.
                        // Method 4 = stored (no compression) before encryption.
                        // OriginalLength = plaintext size; absent for font obfuscation.
                        for attr in e.attributes() {
                            if let Ok(attr) = attr
                                && attr.key.as_ref() == b"OriginalLength"
                            {
                                let val = String::from_utf8_lossy(&attr.value);
                                current_original_length = val.parse::<u64>().ok();
                            }
                        }
                    }
                }
                Ok(Event::End(ref e)) => {
                    let name = String::from_utf8_lossy(e.name().into_inner()).into_owned();
                    if name.ends_with("EncryptedData") {
                        // Commit the entry when we have both an algorithm and a URI.
                        if let (Some(algo), Some(uri)) = (current_algo, current_uri.take()) {
                            encryptions.insert(
                                uri,
                                crate::crypto::EncryptionInfo {
                                    algorithm: algo,
                                    original_length: current_original_length,
                                },
                            );
                        }
                        // Reset per-entry state.
                        current_algo = None;
                        current_original_length = None;
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

    /// Parses the OPF file (usually `.opf`) to build the full [`EpubBook`] domain model.
    pub(super) fn parse_opf(&mut self, opf_path: &str) -> Result<EpubBook, EpubError> {
        let mut opf_file = self.provider.read_file(opf_path)?;
        let mut buf = String::new();
        opf_file.read_to_string(&mut buf)?;

        let mut reader = Reader::from_str(&buf);
        reader.config_mut().trim_text(true);

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
        // Collects raw dc:title data (streamed one at a time, refined in post-processing)
        let mut raw_titles: Vec<RawTitle> = Vec::new();
        // Tracks the id / lang of the dc:title element currently being parsed
        let mut current_title_id: Option<String> = None;
        let mut current_title_lang: Option<String> = None;
        // Collects (meta_id, collection_name) for belongs-to-collection post-processing
        let mut pending_collections: Vec<(String, String)> = Vec::new();
        // Accumulates media:* metadata; None until the first media: property is seen
        let mut mo_meta: Option<crate::model::MediaOverlayMetadata> = None;

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
                                "page-progression-direction" if attr.value.as_ref() == b"rtl" => {
                                    book.metadata.reading_progression =
                                        crate::model::ReadingProgression::Rtl;
                                }
                                _ => {}
                            }
                        }
                    } else if name_str.ends_with("title") {
                        // Capture the element's id and xml:lang for post-processing
                        current_title_id = e
                            .attributes()
                            .flatten()
                            .find(|a| a.key.as_ref() == b"id")
                            .map(|a| String::from_utf8_lossy(&a.value).into_owned());
                        current_title_lang = e
                            .attributes()
                            .flatten()
                            .find(|a| {
                                let k = String::from_utf8_lossy(a.key.into_inner());
                                k == "xml:lang" || k == "lang"
                            })
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
                        let mut media_overlay: Option<String> = None;

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
                                "media-overlay" => media_overlay = Some(value),
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
                                    media_overlay,
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
                            // Collect into raw_titles; post-processing picks the main title.
                            raw_titles.push(RawTitle {
                                id: current_title_id.take(),
                                lang: current_title_lang.take(),
                                text: text.clone(),
                                epub2_subtitle: false,
                            });
                            // Streaming fallback so title is never empty even if
                            // post-processing is somehow skipped.
                            if book.metadata.title.is_none() {
                                book.metadata.title = Some(text);
                            }
                        }
                        OpfState::Subtitle => {
                            // EPUB 2 inline subtitle — collect alongside regular titles.
                            raw_titles.push(RawTitle {
                                id: current_title_id.take(),
                                lang: current_title_lang.take(),
                                text: text.clone(),
                                epub2_subtitle: true,
                            });
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
                        OpfState::MetaGlobal { property, id } => match property.as_str() {
                            "rendition:layout" => {
                                if text == "pre-paginated" {
                                    book.metadata.layout = crate::model::LayoutType::PrePaginated;
                                } else {
                                    book.metadata.layout = crate::model::LayoutType::Reflowable;
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
                            // ── Media Overlays OPF metadata ──────────────────────────────────
                            // Spec: EPUB 3.3 §9.3.5.2 / Appendix D.8
                            "media:duration" => {
                                if let Some(secs) =
                                    super::smil::parse_clock_value(&text)
                                {
                                    // `refines` is already stripped in the MetaGlobal branch above;
                                    // for media:duration with refines we land in MetaRefines, not here.
                                    // This branch captures the GLOBAL duration (no refines).
                                    let mo = mo_meta.get_or_insert_with(Default::default);
                                    mo.total_duration = Some(secs);
                                }
                            }
                            "media:narrator" => {
                                let mo = mo_meta.get_or_insert_with(Default::default);
                                mo.narrators.push(text);
                            }
                            "media:active-class" => {
                                let mo = mo_meta.get_or_insert_with(Default::default);
                                mo.active_class = Some(text);
                            }
                            "media:playback-active-class" => {
                                let mo = mo_meta.get_or_insert_with(Default::default);
                                mo.playback_active_class = Some(text);
                            }
                            _ => {}
                        },
                        OpfState::None => {}
                    }
                    // Reset state and title trackers after consuming text
                    current_title_id = None;
                    current_title_lang = None;
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

        // 2. Build Metadata.titles from raw_titles + refinements.
        //    Then re-derive title / subtitle / sort_as with correct semantics:
        //      - main title: prefer title-type="main", fall back to first non-subtitle
        //      - subtitle: first entry with title-type="subtitle" (lowest display-seq)
        //      - sort_as: file-as of the main title entry
        {
            let mut entries: Vec<crate::model::TitleEntry> = Vec::with_capacity(raw_titles.len());
            for raw in &raw_titles {
                let props = raw.id.as_ref().and_then(|id| refinements.get(id));
                // title-type: EPUB 3 refinement wins; EPUB 2 inline attr is the fallback
                let title_type = if raw.epub2_subtitle {
                    Some("subtitle".to_string())
                } else {
                    props.and_then(|p| p.get("title-type")).cloned()
                };
                let sort_as = props.and_then(|p| p.get("file-as")).cloned();
                let display_seq = props
                    .and_then(|p| p.get("display-seq"))
                    .and_then(|s| s.parse::<u32>().ok());
                entries.push(crate::model::TitleEntry {
                    value: raw.text.clone(),
                    lang: raw.lang.clone(),
                    title_type,
                    sort_as,
                    display_seq,
                });
            }
            book.metadata.titles = entries;
        }

        // Re-derive the simple scalar fields from the now-complete titles list.
        let main_entry = book
            .metadata
            .titles
            .iter()
            .find(|t| t.title_type.as_deref() == Some("main"))
            .or_else(|| {
                book.metadata
                    .titles
                    .iter()
                    .find(|t| t.title_type.as_deref() != Some("subtitle"))
            });
        if let Some(m) = main_entry {
            book.metadata.title = Some(m.value.clone());
            if book.metadata.sort_as.is_none() {
                book.metadata.sort_as = m.sort_as.clone();
            }
        }

        // Subtitle: lowest display-seq among subtitle entries.
        let first_subtitle = {
            let mut subs: Vec<&crate::model::TitleEntry> = book
                .metadata
                .titles
                .iter()
                .filter(|t| t.title_type.as_deref() == Some("subtitle"))
                .collect();
            subs.sort_by_key(|t| t.display_seq.unwrap_or(u32::MAX));
            subs.into_iter().next()
        };
        if let Some(sub) = first_subtitle {
            book.metadata.subtitle = Some(sub.value.clone());
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

        // 4. Finalize Media Overlays metadata
        //    Per-SMIL durations are stored as `<meta property="media:duration" refines="#smil-id">`.
        //    Those land in `refinements[smil-id]["media:duration"]`; extract them now.
        if let Some(mut mo) = mo_meta {
            for (item_id, props) in &refinements {
                if let Some(dur_str) = props.get("media:duration") {
                    if let Some(secs) = super::smil::parse_clock_value(dur_str) {
                        mo.durations.insert(item_id.clone(), secs);
                    }
                }
            }
            book.metadata.media_overlays = Some(mo);
        }

        Ok(book)
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::EpubArchive;
    use std::io::{Cursor, Write};

    fn epub_with_metadata(metadata_xml: &str) -> Vec<u8> {
        let opf = format!(
            "<?xml version=\"1.0\"?>\n\
             <package version=\"3.0\" xmlns=\"http://www.idpf.org/2007/opf\"\
             \n         unique-identifier=\"uid\">\n\
               <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
                         xmlns:opf=\"http://www.idpf.org/2007/opf\">\n\
                 {}\n\
               </metadata>\n\
               <manifest/>\n\
               <spine/>\n\
             </package>",
            metadata_xml
        );
        let container = b"<?xml version=\"1.0\"?>\n\
            <container version=\"1.0\"\
            \n  xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n\
              <rootfiles>\n\
                <rootfile full-path=\"content.opf\"\
            \n              media-type=\"application/oebps-package+xml\"/>\n\
              </rootfiles>\n\
            </container>";

        let mut buf = Vec::new();
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("META-INF/container.xml", stored).unwrap();
        zip.write_all(container).unwrap();
        zip.start_file("content.opf", stored).unwrap();
        zip.write_all(opf.as_bytes()).unwrap();
        zip.finish().unwrap();
        buf
    }

    // ── xml:lang is captured ──────────────────────────────────────────────────

    #[test]
    fn test_title_xml_lang_captured() {
        let bytes = epub_with_metadata(
            "<dc:title xml:lang=\"zh-CN\">软件设计的哲学</dc:title>\
             <dc:title xml:lang=\"en\">A Philosophy of Software Design</dc:title>",
        );
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();

        assert_eq!(book.metadata.titles.len(), 2);
        assert_eq!(book.metadata.titles[0].lang.as_deref(), Some("zh-CN"));
        assert_eq!(book.metadata.titles[1].lang.as_deref(), Some("en"));
    }

    // ── title-type=main beats document order ──────────────────────────────────

    #[test]
    fn test_title_type_main_wins_over_first() {
        let bytes = epub_with_metadata(
            "<dc:title id=\"t-sub\">A Subtitle</dc:title>\
             <dc:title id=\"t-main\">The Real Main Title</dc:title>\
             <meta refines=\"#t-sub\"  property=\"title-type\">subtitle</meta>\
             <meta refines=\"#t-main\" property=\"title-type\">main</meta>",
        );
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("The Real Main Title"));
        assert_eq!(book.metadata.subtitle.as_deref(), Some("A Subtitle"));
    }

    // ── sort_as comes from the main title's file-as ───────────────────────────

    #[test]
    fn test_sort_as_from_main_title_file_as() {
        let bytes = epub_with_metadata(
            "<dc:title id=\"t1\">The Hobbit</dc:title>\
             <meta refines=\"#t1\" property=\"title-type\">main</meta>\
             <meta refines=\"#t1\" property=\"file-as\">Hobbit, The</meta>",
        );
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("The Hobbit"));
        assert_eq!(book.metadata.sort_as.as_deref(), Some("Hobbit, The"));
        assert_eq!(
            book.metadata.titles[0].sort_as.as_deref(),
            Some("Hobbit, The")
        );
    }

    // ── display-seq picks the subtitle ────────────────────────────────────────

    #[test]
    fn test_subtitle_picked_by_display_seq() {
        let bytes = epub_with_metadata(
            "<dc:title id=\"t-main\">Main</dc:title>\
             <dc:title id=\"t-s2\">Second Subtitle</dc:title>\
             <dc:title id=\"t-s1\">First Subtitle</dc:title>\
             <meta refines=\"#t-main\" property=\"title-type\">main</meta>\
             <meta refines=\"#t-s2\"   property=\"title-type\">subtitle</meta>\
             <meta refines=\"#t-s2\"   property=\"display-seq\">2</meta>\
             <meta refines=\"#t-s1\"   property=\"title-type\">subtitle</meta>\
             <meta refines=\"#t-s1\"   property=\"display-seq\">1</meta>",
        );
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();

        assert_eq!(book.metadata.subtitle.as_deref(), Some("First Subtitle"));
    }

    // ── EPUB 2 inline opf:title-type="subtitle" ───────────────────────────────

    #[test]
    fn test_epub2_inline_subtitle_attribute() {
        let bytes = epub_with_metadata(
            "<dc:title>Main Title</dc:title>\
             <dc:title opf:title-type=\"subtitle\">Inline Subtitle</dc:title>",
        );
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("Main Title"));
        assert_eq!(book.metadata.subtitle.as_deref(), Some("Inline Subtitle"));
        let sub = book
            .metadata
            .titles
            .iter()
            .find(|t| t.title_type.as_deref() == Some("subtitle"));
        assert!(sub.is_some(), "subtitle TitleEntry must exist");
    }

    // ── backward compat: single title, no lang, no refinements ───────────────

    #[test]
    fn test_single_title_no_lang_backward_compat() {
        let bytes = epub_with_metadata("<dc:title>Simple Book</dc:title>");
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("Simple Book"));
        assert_eq!(book.metadata.titles.len(), 1);
        assert_eq!(book.metadata.titles[0].value, "Simple Book");
        assert!(book.metadata.titles[0].lang.is_none());
        assert!(book.metadata.titles[0].title_type.is_none());
    }
}
