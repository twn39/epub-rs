//! OPF package document and container parsing.
//!
//! Handles:
//! - `META-INF/container.xml` → rootfile paths
//! - `*.opf` → full `EpubBook` (metadata, manifest, spine, guide)
//!
//! Encryption lives in [`super::encryption`].

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
    Identifier {
        id: Option<String>,
        scheme: Option<String>,
    },

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
        /// The element's own `id` attribute, if any.
        /// Needed so that a11y elements that both refine another element AND
        /// are themselves refined (e.g. `certifiedBy` refines `conformsTo` but
        /// is refined by `certifierCredential`) can be correctly linked.
        self_id: Option<String>,
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

// ── RawIdentifier (parse-phase intermediate) ──────────────────────────────────

/// A `<dc:identifier>` element captured during OPF streaming.
///
/// All identifiers are collected first so that EPUB 3 `identifier-type`
/// refinements (which may appear after the element in the XML) can be
/// back-filled in post-processing before the `AltIdentifier` list is built.
struct RawIdentifier {
    /// Value of the element's own `id` attribute.
    /// Matched against `package@unique-identifier` to identify the primary ID.
    id: Option<String>,
    /// Scheme annotation: EPUB 2 `opf:scheme` attribute or EPUB 3 `identifier-type`
    /// meta value (resolved in post-processing).
    scheme: Option<String>,
    /// Trimmed text content of the element.
    value: String,
}

// ── RawA11yMeta (parse-phase intermediate) ────────────────────────────────────

/// A single accessibility-related `<meta>` element captured during OPF streaming.
///
/// All a11y metas are collected first, then post-processed together so that
/// `refines` relationships can be resolved regardless of XML element order.
struct RawA11yMeta {
    /// Expanded property name, e.g. `"dcterms:conformsTo"`, `"schema:accessMode"`.
    property: String,
    /// Text content of the element.
    value: String,
    /// Value of the `id` attribute (without `#`), used so other elements can refine this one.
    id: Option<String>,
    /// Value of the `refines` attribute (without leading `#`), linking to another element's id.
    refines: Option<String>,
}

// ── EpubArchive impl ────────────────────────────────────────────────────

impl<P: EpubProvider> EpubArchive<P> {
    /// Reads `META-INF/container.xml` and returns every declared rendition with its
    /// selection attributes, preserving document order.
    ///
    /// The first entry is always the default rendition (OCF §3.5.1); callers must
    /// be prepared to handle containers with only one rootfile.
    pub(super) fn parse_container(
        &mut self,
    ) -> Result<Vec<crate::model::RenditionInfo>, EpubError> {
        let mut container_file = self
            .provider
            .read_file("META-INF/container.xml")
            .map_err(|_| EpubError::MissingContainer)?;

        let mut buf = String::new();
        container_file.read_to_string(&mut buf)?;

        let mut reader = Reader::from_str(&buf);
        reader.config_mut().trim_text(true);

        let mut renditions: Vec<crate::model::RenditionInfo> = Vec::new();
        let mut event_buf = Vec::new();

        loop {
            match reader.read_event_into(&mut event_buf)? {
                Event::Empty(ref e) | Event::Start(ref e) if e.name().as_ref() == b"rootfile" => {
                    let mut info = crate::model::RenditionInfo::default();

                    for attr in e.attributes().flatten() {
                        // Older EPUB authoring tools omit the namespace declaration but
                        // still prefix attributes as "rendition:layout", so we strip
                        // everything up to and including the last colon rather than
                        // relying on proper namespace expansion.
                        let key = String::from_utf8_lossy(attr.key.into_inner()).into_owned();
                        let val = String::from_utf8_lossy(&attr.value).into_owned();

                        let local = match key.rfind(':') {
                            Some(i) => &key[i + 1..],
                            None => key.as_str(),
                        };

                        match local {
                            "full-path" => info.opf_path = val,
                            "layout" => info.layout = Some(val),
                            "media" => info.media = Some(val),
                            "language" => info.language = Some(val),
                            "label" => info.label = Some(val),
                            "accessMode" => info.access_mode = Some(val),
                            _ => {} // media-type and others are intentionally ignored
                        }
                    }

                    if !info.opf_path.is_empty() {
                        renditions.push(info);
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            event_buf.clear();
        }

        if renditions.is_empty() {
            Err(EpubError::InvalidFormat(
                "No rootfile full-path found in container.xml".to_string(),
            ))
        } else {
            Ok(renditions)
        }
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
        // Collects all a11y-related <meta> elements; resolved in post-processing
        let mut raw_a11y: Vec<RawA11yMeta> = Vec::new();
        // Tracks `package@unique-identifier` — the id of the primary dc:identifier element
        let mut unique_id_ref: Option<String> = None;
        // Collects all dc:identifier elements; primary is singled out in post-processing
        let mut raw_identifiers: Vec<RawIdentifier> = Vec::new();

        loop {
            match reader.read_event_into(&mut event_buf)? {
                Event::Start(ref e) => {
                    let name = e.name();
                    let name_str = String::from_utf8_lossy(name.into_inner()).into_owned();

                    if name_str.ends_with("metadata") {
                        in_metadata = true;
                    } else if name_str == "package" || name_str.ends_with(":package") {
                        // Capture the unique-identifier pointer that designates the primary
                        // dc:identifier. Appears on the root element before <metadata>.
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.into_inner());
                            if key == "unique-identifier" {
                                unique_id_ref =
                                    Some(String::from_utf8_lossy(&attr.value).into_owned());
                            }
                        }
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
                        // Read the element's `id` attribute (matched against package@unique-identifier)
                        // and `opf:scheme` / `scheme` attribute (EPUB 2 scheme annotation).
                        let id = e
                            .attributes()
                            .flatten()
                            .find(|a| a.key.as_ref() == b"id")
                            .map(|a| String::from_utf8_lossy(&a.value).into_owned());
                        let scheme = e
                            .attributes()
                            .flatten()
                            .find(|a| {
                                String::from_utf8_lossy(a.key.into_inner()).ends_with("scheme")
                            })
                            .map(|a| String::from_utf8_lossy(&a.value).into_owned());
                        state = OpfState::Identifier { id, scheme };
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
                        let mut meta_scheme = None;
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
                                "scheme" => {
                                    meta_scheme =
                                        Some(String::from_utf8_lossy(&attr.value).into_owned());
                                }
                                _ => {}
                            }
                        }
                        // For identifier-type refinements, eagerly store the meta element's own
                        // `scheme` attribute (e.g. "onix:codelist5") under a synthetic key.
                        // go-toolkit uses this scheme URI as AltIdentifier.Scheme rather than
                        // the text value (the code within the codelist).
                        if let (Some(r), Some(p)) = (&refines, &property)
                            && p == "identifier-type"
                            && let Some(s) = &meta_scheme
                        {
                            refinements
                                .entry(r.clone())
                                .or_default()
                                .insert("identifier-type-scheme".to_string(), s.clone());
                        }
                        if let (Some(r), Some(p)) = (refines, property.clone()) {
                            state = OpfState::MetaRefines {
                                ref_id: r,
                                property: p,
                                self_id: meta_id,
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
                    } else if name_str.ends_with("reference") {
                        // EPUB 2 <guide><reference type="text" href="..." title="..."/></guide>
                        let mut ref_type = String::new();
                        let mut href = String::new();
                        let mut title = None;
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.into_inner());
                            let value = String::from_utf8_lossy(&attr.value).into_owned();
                            match key.as_ref() {
                                "type" => ref_type = value,
                                "href" => {
                                    let decoded = percent_encoding::percent_decode_str(&value)
                                        .decode_utf8_lossy()
                                        .into_owned();
                                    // Resolve against OPF directory → package-root-relative.
                                    href = crate::path::resolve_href(&book.opf_dir, &decoded);
                                }
                                "title" => title = Some(value),
                                _ => {}
                            }
                        }
                        if !ref_type.is_empty() && !href.is_empty() {
                            book.guide.push(crate::model::GuideReference {
                                ref_type,
                                href,
                                title,
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
                        OpfState::Identifier { id, scheme } => {
                            // Collect into raw_identifiers; post-processing resolves which is primary.
                            // Empty values are discarded here rather than later to avoid index confusion.
                            let trimmed = text.trim().to_owned();
                            if !trimmed.is_empty() {
                                raw_identifiers.push(RawIdentifier {
                                    id: id.clone(),
                                    scheme: scheme.clone(),
                                    value: trimmed,
                                });
                            }
                        }
                        OpfState::Publisher => book.metadata.publisher = Some(text),
                        OpfState::Description => book.metadata.description = Some(text),
                        OpfState::Date => book.metadata.date = Some(text),
                        OpfState::Modified => book.metadata.modified = Some(text),
                        OpfState::Rights => book.metadata.rights = Some(text),
                        OpfState::Subject => book.metadata.subjects.push(text),
                        OpfState::MetaRefines {
                            ref_id,
                            property,
                            self_id,
                        } => {
                            // Route a11y refinements to the dedicated collector
                            if is_a11y_property(property) {
                                raw_a11y.push(RawA11yMeta {
                                    property: property.clone(),
                                    value: text,
                                    id: self_id.clone(),
                                    refines: Some(ref_id.clone()),
                                });
                            } else {
                                let entry = refinements.entry(ref_id.clone()).or_default();
                                entry.insert(property.clone(), text);
                            }
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
                            // ── EPUB Accessibility metadata ───────────────────────────────────
                            // Suffix-matched to handle any schema: / a11y: / dcterms: prefix
                            // variant that authoring tools may emit.
                            p if is_a11y_property(p) => {
                                raw_a11y.push(RawA11yMeta {
                                    property: p.to_owned(),
                                    value: text,
                                    id: id.clone(),
                                    refines: None,
                                });
                                // Reset state so we don't fall into the default _ arm below
                                state = OpfState::None;
                                event_buf.clear();
                                continue;
                            }
                            // ── Media Overlays OPF metadata ──────────────────────────────────
                            // Spec: EPUB 3.3 §9.3.5.2 / Appendix D.8
                            "media:duration" => {
                                if let Some(secs) = super::smil::parse_clock_value(&text) {
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

        // 4. Finalise Media Overlays metadata
        //    Per-SMIL durations are stored as `<meta property="media:duration" refines="#smil-id">`.
        //    Those land in `refinements[smil-id]["media:duration"]`; extract them now.
        if let Some(mut mo) = mo_meta {
            for (item_id, props) in &refinements {
                if let Some(dur_str) = props.get("media:duration")
                    && let Some(secs) = super::smil::parse_clock_value(dur_str)
                {
                    mo.durations.insert(item_id.clone(), secs);
                }
            }
            book.metadata.media_overlays = Some(mo);
        }

        // 5. Build Accessibility metadata from collected raw metas
        book.metadata.accessibility = build_accessibility(&raw_a11y);

        // 6. Resolve dc:identifier elements into primary + alt identifiers.
        //
        //    EPUB 3.3 §5.5.3.1.1: the `<package unique-identifier="uid">` attribute
        //    names the `dc:identifier` element (by its `id` attribute) that is the
        //    canonical publication identifier. All other non-empty dc:identifier
        //    elements are alternate identifiers (e.g. ISBN-13 alongside a UUID).
        //
        //    EPUB 3 identifier-type refinements that arrived in `refinements` are
        //    back-filled here so that scheme information is available regardless of
        //    XML element order.
        if !raw_identifiers.is_empty() {
            // Back-fill EPUB 3 identifier-type: look up each raw identifier's own id
            // in the refinements map; if a "identifier-type" property exists there,
            // use it as the scheme annotation (EPUB 2 opf:scheme is already in raw.scheme).
            for raw in &mut raw_identifiers {
                if raw.scheme.is_none()
                    && let Some(id) = &raw.id
                    && let Some(props) = refinements.get(id)
                {
                    // Prefer the identifier-type meta's own `scheme` attribute
                    // (e.g. "onix:codelist5") over its text value (e.g. "15").
                    // This matches go-toolkit which stores the codelist URI as Scheme.
                    if let Some(s) = props.get("identifier-type-scheme") {
                        raw.scheme = Some(s.clone());
                    } else if let Some(itype) = props.get("identifier-type") {
                        raw.scheme = Some(itype.clone());
                    }
                }
            }

            // Find which entry is the primary unique identifier.
            // unique_id_ref == None means the OPF has no unique-identifier pointer
            // (technically invalid but common in malformed EPUB 2 files).
            let primary_idx = unique_id_ref.as_deref().and_then(|uid| {
                raw_identifiers
                    .iter()
                    .position(|r| r.id.as_deref() == Some(uid))
            });

            // If no pointer match, fall back to the first entry (go-toolkit lines 550-552).
            let primary_idx = primary_idx.unwrap_or(0);

            book.metadata.identifier = Some(raw_identifiers[primary_idx].value.clone());

            // Everything else becomes an AltIdentifier.
            for (i, raw) in raw_identifiers.into_iter().enumerate() {
                if i == primary_idx {
                    continue;
                }
                let alt = match raw.scheme {
                    Some(s) => crate::model::AltIdentifier::WithScheme {
                        value: raw.value,
                        scheme: s,
                    },
                    None => crate::model::AltIdentifier::Simple(raw.value),
                };
                book.metadata.alt_identifiers.push(alt);
            }
        }

        Ok(book)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// A11y helper functions
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `true` if the OPF property belongs to an accessibility vocabulary.
///
/// We use suffix matching so that any authoring-tool prefix variant
/// (e.g. `schema:accessMode`, `accessMode`, `http://schema.org/accessMode`)
/// is correctly routed to the a11y collector.
fn is_a11y_property(p: &str) -> bool {
    matches!(
        p,
        "dcterms:conformsTo"
            | "a11y:certifiedBy"
            | "a11y:certifierCredential"
            | "a11y:certifierReport"
            | "a11y:exemption"
            | "schema:accessMode"
            | "schema:accessModeSufficient"
            | "schema:accessibilityFeature"
            | "schema:accessibilityHazard"
            | "schema:accessibilitySummary"
    ) || p.ends_with(":conformsTo")
        || p.ends_with(":certifiedBy")
        || p.ends_with(":certifierCredential")
        || p.ends_with(":certifierReport")
        || p.ends_with(":exemption")
        || p.ends_with(":accessMode")
        || p.ends_with(":accessModeSufficient")
        || p.ends_with(":accessibilityFeature")
        || p.ends_with(":accessibilityHazard")
        || p.ends_with(":accessibilitySummary")
}

/// Build an [`crate::model::Accessibility`] from raw collected OPF meta elements.
///
/// Implements the two-pass strategy required because `refines` relationships
/// may appear in any XML order:
/// 1. All a11y `<meta>` elements are collected during SAX streaming.
/// 2. This function correlates `id` ↔ `refines` to assemble the typed model.
fn build_accessibility(metas: &[RawA11yMeta]) -> Option<crate::model::Accessibility> {
    use crate::model::{
        A11yAccessMode, A11yCertification, A11yExemption, A11yFeature, A11yHazard,
        A11yPrimaryAccessMode, A11yProfile, Accessibility,
    };

    let mut a11y = Accessibility::default();

    // ── 1. dcterms:conformsTo ─────────────────────────────────────────────────
    // Spec §3.5.2: multiple conformsTo elements are allowed.
    let conforms_to_metas: Vec<&RawA11yMeta> = metas
        .iter()
        .filter(|m| m.property.ends_with(":conformsTo") || m.property == "dcterms:conformsTo")
        .collect();

    for ct in &conforms_to_metas {
        if let Some(profile) = A11yProfile::from_opf_value(&ct.value)
            && !a11y.conforms_to.contains(&profile)
        {
            a11y.conforms_to.push(profile);
        }
    }
    a11y.conforms_to.sort();

    // ── 2. a11y:certifiedBy (refines a conformsTo meta, or stands alone) ──────
    // Spec §3.5.3.1 Example 2/3: certifiedBy refines="#conf" where "conf" is the
    // id of the dcterms:conformsTo meta.
    let conf_ids: std::collections::HashSet<&str> = conforms_to_metas
        .iter()
        .filter_map(|m| m.id.as_deref())
        .collect();

    let certified_by_meta = metas.iter().find(|m| {
        (m.property.ends_with(":certifiedBy") || m.property == "a11y:certifiedBy")
            && m.refines
                .as_deref()
                // Accept when it refines a known conformsTo id, or has no refines (standalone)
                .is_none_or(|r| conf_ids.contains(r))
    });

    if let Some(cb) = certified_by_meta {
        let certifier_id = cb.id.as_deref();

        let credential = certifier_id
            .and_then(|cid| {
                metas.iter().find(|m| {
                    (m.property.ends_with(":certifierCredential")
                        || m.property == "a11y:certifierCredential")
                        && m.refines.as_deref() == Some(cid)
                })
            })
            .map(|m| m.value.trim().to_owned());

        let report = certifier_id
            .and_then(|cid| {
                metas.iter().find(|m| {
                    (m.property.ends_with(":certifierReport")
                        || m.property == "a11y:certifierReport")
                        && m.refines.as_deref() == Some(cid)
                })
            })
            .map(|m| m.value.trim().to_owned());

        let cert = A11yCertification {
            certified_by: cb.value.trim().to_owned(),
            credential,
            report,
        };
        if !cert.is_empty() {
            a11y.certification = Some(cert);
        }
    }

    // ── 3. schema:accessMode ──────────────────────────────────────────────────
    a11y.access_modes = metas
        .iter()
        .filter(|m| {
            m.property.ends_with(":accessMode") && !m.property.ends_with(":accessModeSufficient")
        })
        .map(|m| A11yAccessMode::from_str(m.value.trim()))
        .collect();

    // ── 4. schema:accessModeSufficient ───────────────────────────────────────
    // Each <meta> value is a comma-separated list representing one sufficient set.
    for m in metas
        .iter()
        .filter(|m| m.property.ends_with(":accessModeSufficient"))
    {
        let set: Vec<A11yPrimaryAccessMode> = m
            .value
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(A11yPrimaryAccessMode::from_str)
            .collect();
        if !set.is_empty() {
            a11y.access_modes_sufficient.push(set);
        }
    }

    // ── 5. schema:accessibilityFeature ───────────────────────────────────────
    a11y.features = metas
        .iter()
        .filter(|m| m.property.ends_with(":accessibilityFeature"))
        .map(|m| A11yFeature::from_str(m.value.trim()))
        .collect();

    // ── 6. schema:accessibilityHazard ────────────────────────────────────────
    a11y.hazards = metas
        .iter()
        .filter(|m| m.property.ends_with(":accessibilityHazard"))
        .map(|m| A11yHazard::from_str(m.value.trim()))
        .collect();

    // ── 7. schema:accessibilitySummary ───────────────────────────────────────
    a11y.summary = metas
        .iter()
        .find(|m| m.property.ends_with(":accessibilitySummary"))
        .map(|m| m.value.trim().to_owned());

    // ── 8. a11y:exemption ────────────────────────────────────────────────────
    a11y.exemptions = metas
        .iter()
        .filter(|m| m.property.ends_with(":exemption") || m.property == "a11y:exemption")
        .map(|m| A11yExemption::from_str(m.value.trim()))
        .collect();

    if a11y.is_empty() { None } else { Some(a11y) }
}

#[cfg(test)]
mod tests {
    use crate::model::{A11yAccessMode, A11yFeature, A11yHazard, A11yProfile};
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

    // ── parse_container: RenditionInfo ────────────────────────────────────────

    fn epub_zip_with_container(container_xml: &str) -> Vec<u8> {
        let opf = b"<?xml version=\"1.0\"?>\
            <package version=\"3.0\" xmlns=\"http://www.idpf.org/2007/opf\" unique-identifier=\"uid\">\
              <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
                <dc:title>Test</dc:title>\
              </metadata>\
              <manifest/><spine/>\
            </package>";
        let mut buf = Vec::new();
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("META-INF/container.xml", stored).unwrap();
        zip.write_all(container_xml.as_bytes()).unwrap();
        zip.start_file("content.opf", stored).unwrap();
        zip.write_all(opf).unwrap();
        zip.finish().unwrap();
        buf
    }

    #[test]
    fn test_parse_container_single_rootfile() {
        let container = r#"<?xml version="1.0"?>
            <container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
              <rootfiles>
                <rootfile full-path="content.opf" media-type="application/oebps-package+xml"/>
              </rootfiles>
            </container>"#;
        let bytes = epub_zip_with_container(container);
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let renditions = archive.get_renditions().unwrap();

        assert_eq!(renditions.len(), 1);
        assert_eq!(renditions[0].opf_path, "content.opf");
        // No selection attributes present — all optional fields are None
        assert!(renditions[0].layout.is_none());
        assert!(renditions[0].label.is_none());
        assert!(
            renditions[0].is_reflowable(),
            "absent layout = reflowable per spec"
        );
    }

    #[test]
    fn test_parse_container_multiple_renditions_with_selection_attrs() {
        // Simulates a manga EPUB that ships both a fixed-layout and a reflowable edition.
        let container = r#"<?xml version="1.0"?>
            <container version="1.0"
                       xmlns="urn:oasis:names:tc:opendocument:xmlns:container"
                       xmlns:rendition="http://www.idpf.org/2013/rendition">
              <rootfiles>
                <rootfile full-path="content.opf"
                          media-type="application/oebps-package+xml"
                          rendition:layout="pre-paginated"
                          rendition:label="漫画版"
                          rendition:accessMode="visual"/>
                <rootfile full-path="text.opf"
                          media-type="application/oebps-package+xml"
                          rendition:layout="reflowable"
                          rendition:label="阅读版"
                          rendition:accessMode="textual"
                          rendition:language="zh-Hant"/>
              </rootfiles>
            </container>"#;
        let bytes = epub_zip_with_container(container);
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let renditions = archive.get_renditions().unwrap();

        assert_eq!(renditions.len(), 2);

        // First is default (fixed-layout manga)
        assert_eq!(renditions[0].opf_path, "content.opf");
        assert_eq!(renditions[0].layout.as_deref(), Some("pre-paginated"));
        assert_eq!(renditions[0].label.as_deref(), Some("漫画版"));
        assert_eq!(renditions[0].access_mode.as_deref(), Some("visual"));
        assert!(renditions[0].is_fixed_layout());
        assert!(!renditions[0].is_reflowable());

        // Second is the text edition
        assert_eq!(renditions[1].opf_path, "text.opf");
        assert_eq!(renditions[1].layout.as_deref(), Some("reflowable"));
        assert_eq!(renditions[1].label.as_deref(), Some("阅读版"));
        assert_eq!(renditions[1].access_mode.as_deref(), Some("textual"));
        assert_eq!(renditions[1].language.as_deref(), Some("zh-Hant"));
        assert!(renditions[1].is_reflowable());
    }

    // ── A11y: EPUB Accessibility 1.1 conformsTo text pattern ─────────────────

    #[test]
    fn test_a11y_conforms_to_epub_a11y_11() {
        let bytes = epub_with_metadata(
            r#"<meta property="dcterms:conformsTo">EPUB Accessibility 1.1 - WCAG 2.2 Level AA</meta>"#,
        );
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();
        let a11y = book.metadata.accessibility.expect("should have a11y");
        assert_eq!(a11y.conforms_to.len(), 1);
        assert_eq!(
            a11y.conforms_to[0].0,
            "EPUB Accessibility 1.1 - WCAG 2.2 Level AA"
        );
    }

    // ── A11y: EPUB Accessibility 1.0 URL alias normalization ─────────────────

    #[test]
    fn test_a11y_conforms_to_epub_a11y_10_url() {
        let bytes = epub_with_metadata(
            r#"<meta property="dcterms:conformsTo">https://www.idpf.org/epub/a11y/accessibility-20170105.html#wcag-aa</meta>"#,
        );
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();
        let a11y = book.metadata.accessibility.expect("should have a11y");
        assert_eq!(a11y.conforms_to[0].0, A11yProfile::A10_WCAG_20_AA);
    }

    // ── A11y: certifiedBy refines conformsTo ──────────────────────────────────

    #[test]
    fn test_a11y_certified_by_refines() {
        let bytes = epub_with_metadata(
            r##"<meta property="dcterms:conformsTo" id="conf">EPUB Accessibility 1.1 - WCAG 2.2 Level AA</meta>
               <meta property="a11y:certifiedBy" id="cert" refines="#conf">Acme Accessibility Lab</meta>
               <meta property="a11y:certifierCredential" refines="#cert">https://acme.example.com/badge</meta>"##,
        );
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();
        let a11y = book.metadata.accessibility.expect("should have a11y");
        let cert = a11y.certification.expect("should have certification");
        assert_eq!(cert.certified_by, "Acme Accessibility Lab");
        assert_eq!(
            cert.credential.as_deref(),
            Some("https://acme.example.com/badge")
        );
    }

    // ── A11y: access modes and features ──────────────────────────────────────

    #[test]
    fn test_a11y_access_modes_and_features() {
        let bytes = epub_with_metadata(
            r#"<meta property="schema:accessMode">textual</meta>
               <meta property="schema:accessMode">visual</meta>
               <meta property="schema:accessModeSufficient">textual</meta>
               <meta property="schema:accessModeSufficient">textual,visual</meta>
               <meta property="schema:accessibilityFeature">alternativeText</meta>
               <meta property="schema:accessibilityHazard">noFlashingHazard</meta>
               <meta property="schema:accessibilitySummary">All images have alt text.</meta>"#,
        );
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();
        let a11y = book.metadata.accessibility.expect("should have a11y");

        assert_eq!(a11y.access_modes.len(), 2);
        assert!(a11y.access_modes.contains(&A11yAccessMode::Textual));
        assert!(a11y.access_modes.contains(&A11yAccessMode::Visual));

        assert_eq!(a11y.access_modes_sufficient.len(), 2);
        assert_eq!(a11y.access_modes_sufficient[1].len(), 2); // "textual,visual"

        assert!(a11y.features.contains(&A11yFeature::AlternativeText));
        assert!(a11y.hazards.contains(&A11yHazard::NoFlashingHazard));
        assert_eq!(a11y.summary.as_deref(), Some("All images have alt text."));
    }

    // ── A11y: absent metadata → accessibility is None ────────────────────────

    #[test]
    fn test_a11y_absent_gives_none() {
        let bytes = epub_with_metadata(r#"<dc:title>Plain Book</dc:title>"#);
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();
        assert!(
            book.metadata.accessibility.is_none(),
            "no a11y meta → accessibility must be None"
        );
    }

    // ── A11y: A11yProfile ordering ───────────────────────────────────────────

    #[test]
    fn test_a11y_profile_ordering() {
        let aa = A11yProfile::from_opf_value("EPUB Accessibility 1.1 - WCAG 2.2 Level AA").unwrap();
        let a = A11yProfile::from_opf_value("EPUB Accessibility 1.1 - WCAG 2.0 Level A").unwrap();
        let v10_aa = A11yProfile::from_opf_value(A11yProfile::A10_WCAG_20_AA).unwrap();
        assert!(a < aa, "2.0 A should rank below 2.2 AA");
        assert!(v10_aa < a, "1.0 AA should rank below 1.1 A");
    }

    // ── Identifier helpers ────────────────────────────────────────────────────

    /// Build a minimal EPUB with a custom `unique-identifier` attribute on `<package>`.
    fn epub_with_opf(unique_id_attr: &str, metadata_xml: &str) -> Vec<u8> {
        let opf = format!(
            "<?xml version=\"1.0\"?>\n\
             <package version=\"3.0\" xmlns=\"http://www.idpf.org/2007/opf\" {unique_id_attr}>\n\
               <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
                         xmlns:opf=\"http://www.idpf.org/2007/opf\">\n\
                 {metadata_xml}\n\
               </metadata>\n\
               <manifest/>\n\
               <spine/>\n\
             </package>",
        );
        let container = b"<?xml version=\"1.0\"?>\n\
            <container version=\"1.0\"\n\
            \n  xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n\
              <rootfiles>\n\
                <rootfile full-path=\"content.opf\"\n\
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

    // ── Identifier: unique-identifier pointer selects correct primary ─────────

    #[test]
    fn test_identifier_unique_by_id() {
        // OPF with three dc:identifier elements; unique-identifier points to "pub-id"
        // which is the second one.  Matches the go-toolkit identifier-unique.opf fixture.
        let bytes = epub_with_opf(
            r#"unique-identifier="pub-id""#,
            r#"<dc:title>Test</dc:title>
               <dc:identifier>   </dc:identifier>
               <dc:identifier id="isbn">978-3-16-148410-0</dc:identifier>
               <dc:identifier id="pub-id">urn:uuid:2</dc:identifier>"#,
        );
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();

        // Primary identifier must be the element matched by unique-identifier
        assert_eq!(
            book.metadata.identifier.as_deref(),
            Some("urn:uuid:2"),
            "primary identifier must follow unique-identifier pointer"
        );
        // The ISBN becomes an AltIdentifier (empty dc:identifier is dropped)
        assert_eq!(book.metadata.alt_identifiers.len(), 1);
        assert_eq!(
            book.metadata.alt_identifiers[0].value(),
            "978-3-16-148410-0"
        );
        assert!(book.metadata.alt_identifiers[0].scheme().is_none());
    }

    // ── Identifier: fallback to first when no unique-identifier ──────────────

    #[test]
    fn test_identifier_fallback_to_first() {
        // No unique-identifier attribute; first dc:identifier should become primary.
        let bytes = epub_with_opf(
            "", // no unique-identifier
            r#"<dc:title>Test</dc:title>
               <dc:identifier>urn:uuid:first</dc:identifier>
               <dc:identifier>urn:uuid:second</dc:identifier>"#,
        );
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();

        assert_eq!(book.metadata.identifier.as_deref(), Some("urn:uuid:first"));
        assert_eq!(book.metadata.alt_identifiers.len(), 1);
        assert_eq!(book.metadata.alt_identifiers[0].value(), "urn:uuid:second");
    }

    // ── Identifier: EPUB 2 opf:scheme attribute preserved ────────────────────

    #[test]
    fn test_identifier_epub2_scheme() {
        // EPUB 2 uses opf:scheme attribute to annotate the identifier type.
        let bytes = epub_with_opf(
            r#"unique-identifier="book-id""#,
            r#"<dc:title>Test</dc:title>
               <dc:identifier id="book-id">urn:uuid:primary</dc:identifier>
               <dc:identifier opf:scheme="ISBN">978-0-306-40615-7</dc:identifier>"#,
        );
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();

        assert_eq!(
            book.metadata.identifier.as_deref(),
            Some("urn:uuid:primary")
        );
        assert_eq!(book.metadata.alt_identifiers.len(), 1);
        assert_eq!(
            book.metadata.alt_identifiers[0].value(),
            "978-0-306-40615-7"
        );
        assert_eq!(book.metadata.alt_identifiers[0].scheme(), Some("ISBN"));
    }

    // ── Identifier: EPUB 3 identifier-type refinement back-filled ────────────

    #[test]
    fn test_identifier_epub3_type_refines() {
        // EPUB 3 refines an alt identifier with its type via identifier-type property.
        let bytes = epub_with_opf(
            r#"unique-identifier="pub-id""#,
            "<dc:title>Test</dc:title>\n\
               <dc:identifier id=\"pub-id\">urn:uuid:main</dc:identifier>\n\
               <dc:identifier id=\"isbn-id\">978-3-16-148410-0</dc:identifier>\n\
               <meta refines=\"#isbn-id\" property=\"identifier-type\" scheme=\"onix:codelist5\">15</meta>",
        );
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();

        assert_eq!(book.metadata.identifier.as_deref(), Some("urn:uuid:main"));
        assert_eq!(book.metadata.alt_identifiers.len(), 1);
        // scheme should be back-filled from identifier-type refinement
        assert_eq!(
            book.metadata.alt_identifiers[0].scheme(),
            Some("onix:codelist5")
        );
    }

    // ── Identifier: empty/whitespace-only elements are filtered ──────────────

    #[test]
    fn test_identifier_empty_filtered() {
        // An empty dc:identifier element must be silently ignored.
        let bytes = epub_with_opf(
            r#"unique-identifier="pub-id""#,
            r#"<dc:title>Test</dc:title>
               <dc:identifier>   </dc:identifier>
               <dc:identifier id="pub-id">urn:uuid:real</dc:identifier>"#,
        );
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();

        assert_eq!(book.metadata.identifier.as_deref(), Some("urn:uuid:real"));
        assert!(
            book.metadata.alt_identifiers.is_empty(),
            "empty identifier must not appear in alt_identifiers"
        );
    }

    // ── AltIdentifier: serde bare-string form (no scheme) ────────────────────

    #[test]
    fn test_alt_identifier_serde_no_scheme() {
        let alt = crate::model::AltIdentifier::Simple("urn:isbn:9780306406157".to_string());
        let json = serde_json::to_string(&alt).unwrap();
        assert_eq!(json, r#""urn:isbn:9780306406157""#);

        let back: crate::model::AltIdentifier = serde_json::from_str(&json).unwrap();
        assert_eq!(back.value(), "urn:isbn:9780306406157");
        assert!(back.scheme().is_none());
    }

    // ── AltIdentifier: serde object form (with scheme) ────────────────────────

    #[test]
    fn test_alt_identifier_serde_with_scheme() {
        let alt = crate::model::AltIdentifier::WithScheme {
            value: "978-3-16-148410-0".to_string(),
            scheme: "ISBN".to_string(),
        };
        let json = serde_json::to_string(&alt).unwrap();
        assert_eq!(json, r#"{"value":"978-3-16-148410-0","scheme":"ISBN"}"#);

        let back: crate::model::AltIdentifier = serde_json::from_str(&json).unwrap();
        assert_eq!(back.value(), "978-3-16-148410-0");
        assert_eq!(back.scheme(), Some("ISBN"));
    }

    // ── encryption.xml: font + AES OriginalLength ─────────────────────────────

    fn epub_with_encryption(encryption_xml: &str) -> Vec<u8> {
        let opf = r#"<?xml version="1.0"?>
             <package version="3.0" xmlns="http://www.idpf.org/2007/opf"
                      unique-identifier="uid">
               <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
                 <dc:title>Enc</dc:title>
                 <dc:identifier id="uid">urn:uuid:enc</dc:identifier>
               </metadata>
               <manifest>
                 <item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
                 <item id="f1" href="fonts/a.otf" media-type="font/otf"/>
               </manifest>
               <spine>
                 <itemref idref="c1"/>
               </spine>
             </package>"#;
        let container = br#"<?xml version="1.0"?>
            <container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
              <rootfiles>
                <rootfile full-path="content.opf" media-type="application/oebps-package+xml"/>
              </rootfiles>
            </container>"#;
        let mut buf = Vec::new();
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("META-INF/container.xml", stored).unwrap();
        zip.write_all(container).unwrap();
        zip.start_file("META-INF/encryption.xml", stored).unwrap();
        zip.write_all(encryption_xml.as_bytes()).unwrap();
        zip.start_file("content.opf", stored).unwrap();
        zip.write_all(opf.as_bytes()).unwrap();
        zip.start_file("ch1.xhtml", stored).unwrap();
        zip.write_all(b"<html><body>Hi</body></html>").unwrap();
        zip.start_file("fonts/a.otf", stored).unwrap();
        zip.write_all(&[0u8; 100]).unwrap();
        zip.finish().unwrap();
        buf
    }

    #[test]
    fn test_encryption_font_and_aes_original_length() {
        let enc = r#"<?xml version="1.0"?>
<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:enc"
            xmlns:enc="http://www.w3.org/2001/04/xmlenc#"
            xmlns:comp="http://www.idpf.org/2016/encryption#compression">
  <enc:EncryptedData>
    <enc:EncryptionMethod Algorithm="http://www.idpf.org/2008/embedding"/>
    <enc:CipherData>
      <enc:CipherReference URI="fonts/a.otf"/>
    </enc:CipherData>
  </enc:EncryptedData>
  <enc:EncryptedData>
    <enc:EncryptionMethod Algorithm="http://www.w3.org/2001/04/xmlenc#aes256-cbc"/>
    <enc:CipherData>
      <enc:CipherReference URI="ch1.xhtml"/>
    </enc:CipherData>
    <enc:EncryptionProperties>
      <enc:EncryptionProperty>
        <comp:Compression Method="8" OriginalLength="42"/>
      </enc:EncryptionProperty>
    </enc:EncryptionProperties>
  </enc:EncryptedData>
</encryption>"#;
        let bytes = epub_with_encryption(enc);
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();

        let font = book.encryptions.get("fonts/a.otf").expect("font entry");
        assert_eq!(font.algorithm, crate::crypto::ObfuscationAlgorithm::Idpf);
        assert!(font.original_length.is_none());
        assert!(font.font_obfuscation().is_some());

        let chapter = book.encryptions.get("ch1.xhtml").expect("aes entry");
        assert_eq!(
            chapter.algorithm,
            crate::crypto::ObfuscationAlgorithm::AesCbc
        );
        assert_eq!(chapter.original_length, Some(42));
        assert!(chapter.font_obfuscation().is_none());
        assert!(
            chapter
                .algorithm_uri
                .as_deref()
                .unwrap_or("")
                .contains("aes256-cbc")
        );
    }

    // ── EPUB 2 guide ──────────────────────────────────────────────────────────

    #[test]
    fn test_parse_epub2_guide_text_reference() {
        let opf = r#"<?xml version="1.0"?>
             <package version="2.0" xmlns="http://www.idpf.org/2007/opf" unique-identifier="uid">
               <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
                 <dc:title>Guide Book</dc:title>
                 <dc:identifier id="uid">urn:uuid:guide</dc:identifier>
                 <dc:language>en</dc:language>
               </metadata>
               <manifest>
                 <item id="cover" href="cover.xhtml" media-type="application/xhtml+xml"/>
                 <item id="c1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/>
                 <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
               </manifest>
               <spine toc="ncx">
                 <itemref idref="cover"/>
                 <itemref idref="c1"/>
               </spine>
               <guide>
                 <reference type="cover" href="cover.xhtml" title="Cover"/>
                 <reference type="text" href="text/ch1.xhtml" title="Start"/>
               </guide>
             </package>"#;
        let container = br#"<?xml version="1.0"?>
            <container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
              <rootfiles>
                <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
              </rootfiles>
            </container>"#;
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let stored = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("META-INF/container.xml", stored).unwrap();
            zip.write_all(container).unwrap();
            zip.start_file("OEBPS/content.opf", stored).unwrap();
            zip.write_all(opf.as_bytes()).unwrap();
            zip.start_file("OEBPS/cover.xhtml", stored).unwrap();
            zip.write_all(b"<html><body>Cover</body></html>").unwrap();
            zip.start_file("OEBPS/text/ch1.xhtml", stored).unwrap();
            zip.write_all(b"<html><body>Chapter</body></html>").unwrap();
            zip.start_file("OEBPS/toc.ncx", stored).unwrap();
            zip.write_all(
                br#"<?xml version="1.0"?>
                <ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
                  <head><meta name="dtb:uid" content="urn:uuid:guide"/></head>
                  <docTitle><text>Guide Book</text></docTitle>
                  <navMap>
                    <navPoint id="np1"><navLabel><text>Ch1</text></navLabel>
                      <content src="text/ch1.xhtml"/></navPoint>
                  </navMap>
                </ncx>"#,
            )
            .unwrap();
            zip.finish().unwrap();
        }
        let mut archive = EpubArchive::new(Cursor::new(buf)).unwrap();
        let book = archive.parse().unwrap();
        assert_eq!(book.guide.len(), 2);
        assert_eq!(book.guide[0].ref_type, "cover");
        assert_eq!(book.guide[1].ref_type, "text");
        // href resolved against OEBPS/
        assert_eq!(book.guide[1].href, "OEBPS/text/ch1.xhtml");

        let start = archive.preferred_reading_start(&book);
        assert_eq!(start.source, "guide:text");
        assert_eq!(start.spine_index, 1);
    }

    #[test]
    fn test_content_decryptor_hook() {
        // AES entry without real crypto — decryptor rewrites ciphertext to plaintext.
        let enc = r#"<?xml version="1.0"?>
<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:enc"
            xmlns:enc="http://www.w3.org/2001/04/xmlenc#">
  <enc:EncryptedData>
    <enc:EncryptionMethod Algorithm="http://www.w3.org/2001/04/xmlenc#aes256-cbc"/>
    <enc:CipherData>
      <enc:CipherReference URI="ch1.xhtml"/>
    </enc:CipherData>
  </enc:EncryptedData>
</encryption>"#;
        let bytes = epub_with_encryption(enc);
        let mut archive = EpubArchive::new(Cursor::new(bytes)).unwrap();
        let book = archive.parse().unwrap();
        archive.set_content_decryptor(|path, cipher, info| {
            assert_eq!(path, "ch1.xhtml");
            assert_eq!(info.algorithm, crate::crypto::ObfuscationAlgorithm::AesCbc);
            assert!(!cipher.is_empty());
            Some(b"<html><body>DECRYPTED</body></html>".to_vec())
        });
        let out = archive.get_resource_by_href(&book, "ch1.xhtml").unwrap();
        assert_eq!(out, b"<html><body>DECRYPTED</body></html>");
    }
}
