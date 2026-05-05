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
    MetaRefines { ref_id: String, property: String },
    /// Global `<meta property="...">` — carries the element's own `id` for refinement lookup
    MetaGlobal { property: String, id: Option<String> },
}

// ── EpubArchive impl ──────────────────────────────────────────────────────────

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

    /// Reads `META-INF/encryption.xml` to find obfuscated resources.
    pub(super) fn parse_encryption(
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
                                    layout_override =
                                        Some(crate::model::LayoutType::PrePaginated);
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
}
