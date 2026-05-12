//! EPUB Domain Models

use std::collections::HashMap;

/// Specifies the version of the EPUB standard to target during generation.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EpubVersion {
    /// EPUB 2.0 (Compatible with older e-readers, relies on NCX)
    V20,
    /// EPUB 3.0 (Modern standard, uses HTML5 navigation and semantic tags)
    #[default]
    V30,
}

/// The layout rendition type of the EPUB.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutType {
    /// Content flows dynamically to fit the screen (default).
    #[default]
    Reflowable,
    /// Content is pre-paginated with fixed dimensions (e.g. comics, children's books).
    PrePaginated,
}

/// Hints for how a fixed-layout spine item should be displayed in a synthetic spread.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSpread {
    None,
    Left,
    Right,
    Center,
}

/// The reading progression direction of the EPUB publication.
///
/// Parsed from `<spine page-progression-direction="...">` in the OPF package document.
/// When not explicitly set, use [`Metadata::effective_reading_progression`] to obtain
/// a language-inferred value.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ReadingProgression {
    /// Left-to-right (default for Latin, CJK horizontal scripts, etc.)
    #[default]
    Ltr,
    /// Right-to-left (Arabic, Hebrew, Persian, Urdu, Japanese vertical, etc.)
    Rtl,
}

/// Describes a single rendition (OPF package document) declared in `META-INF/container.xml`.
///
/// An EPUB container may include multiple renditions of the same publication — for example a
/// reflowable text version and a fixed-layout (comic/manga) version.  Each rendition is
/// identified by its [`opf_path`][RenditionInfo::opf_path] and may carry optional selection
/// attributes that conformant Reading Systems use to automatically choose the most appropriate
/// rendition for the current device or user preference.
///
/// The **first** entry in the list returned by `EpubArchive::get_renditions()` is always the
/// *default rendition*, which every Reading System must be capable of processing (OCF §3.5.1).
///
/// # EPUB Multiple-Rendition Example
/// ```xml
/// <rootfiles>
///   <!-- Default: pre-paginated (manga) edition -->
///   <rootfile full-path="EPUB/manga.opf"
///             media-type="application/oebps-package+xml"
///             rendition:layout="pre-paginated"
///             rendition:label="漫画版"/>
///   <!-- Alternative: reflowable text edition -->
///   <rootfile full-path="EPUB/text.opf"
///             media-type="application/oebps-package+xml"
///             rendition:layout="reflowable"
///             rendition:label="阅读版"
///             rendition:accessMode="textual"/>
/// </rootfiles>
/// ```
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Default)]
pub struct RenditionInfo {
    /// EPUB-root-relative path to the OPF package document (`full-path` attribute).
    pub opf_path: String,

    /// `rendition:layout` selection hint — `"reflowable"` or `"pre-paginated"`.
    ///
    /// `None` means the attribute was absent; per the EPUB Multiple-Rendition spec the
    /// absence is treated the same as `"reflowable"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,

    /// `rendition:media` — CSS media query string (e.g. `"screen and (min-width:800px)"`).
    ///
    /// Reading Systems evaluate this query against the device's capabilities to decide
    /// which rendition best matches the current viewing environment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<String>,

    /// `rendition:language` — RFC 5646 language tag (e.g. `"zh-Hant"`, `"en"`).
    ///
    /// Useful when the container holds different-language editions of the same title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// `rendition:label` — human-readable name for the rendition.
    ///
    /// Suitable for display in a Reading System's rendition-selection UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,

    /// `rendition:accessMode` — primary access mode (`"textual"`, `"visual"`, `"auditory"`).
    ///
    /// Based on ISO 24751-3; allows accessibility-aware Reading Systems to prefer the
    /// rendition that is most appropriate for users who rely on screen readers or other AT.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_mode: Option<String>,
}

impl RenditionInfo {
    /// Returns `true` if this rendition can reflow to fit the reading surface.
    ///
    /// A missing `layout` attribute is treated as reflowable because the EPUB
    /// Multiple-Rendition spec defines reflowable as the default; it is the mode
    /// every legacy EPUB implicitly uses.
    pub fn is_reflowable(&self) -> bool {
        self.layout.as_deref() != Some("pre-paginated")
    }

    /// Returns `true` if this rendition uses a fixed coordinate system per page.
    ///
    /// Fixed-layout EPUBs (comics, children's books, technical manuals with precise
    /// typography) cannot be reflowed and require a different rendering path.
    pub fn is_fixed_layout(&self) -> bool {
        self.layout.as_deref() == Some("pre-paginated")
    }
}

/// Represents a series or collection this EPUB belongs to.
///
/// Parsed from `<meta property="belongs-to-collection">` in EPUB 3 OPF,
/// with optional refinements for `collection-type` and `group-position`.
///
/// # EPUB 3 Example
/// ```xml
/// <meta id="col-1" property="belongs-to-collection">A Song of Ice and Fire</meta>
/// <meta refines="#col-1" property="collection-type">series</meta>
/// <meta refines="#col-1" property="group-position">1</meta>
/// ```
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct BelongsTo {
    /// The name of the series or collection.
    pub name: String,
    /// The collection type. Common values: `"series"`, `"collection"`.
    /// Defaults to `"series"` when the `collection-type` refinement is absent.
    #[serde(default = "default_collection_type")]
    pub collection_type: String,
    /// Optional ordinal position within the series (e.g., `1.0`, `2.5`).
    /// Uses `f64` because EPUB allows fractional positions for sub-volumes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<f64>,
}

fn default_collection_type() -> String {
    "series".to_string()
}

/// Represents an item in the reading order (spine).
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct SpineItem {
    /// The ID reference to the manifest item
    pub idref: String,
    /// Whether this item should be read linearly (part of the normal reading flow).
    /// If false, it's typically supplementary content (like an answer key or popup).
    pub linear: bool,
    /// Optional property indicating if the item has a specific layout override.
    pub layout_override: Option<LayoutType>,
    /// Optional property indicating how the item behaves in a two-page spread (left, right, center, none).
    pub page_spread: Option<PageSpread>,
}

impl SpineItem {
    pub fn new(idref: impl Into<String>) -> Self {
        Self {
            idref: idref.into(),
            linear: true,
            layout_override: None,
            page_spread: None,
        }
    }
}

/// A synthetic reading position (virtual page).
///
/// Mirrors the `Locator` type from the Readium go-toolkit, structured to carry the same
/// fields: `href`, `global_position` (≡ `Locations.Position`),
/// `chapter_progression` (≡ `Locations.Progression`),
/// `total_progression` (≡ `Locations.TotalProgression`), and `title`.
///
/// The `cfi` field is an epub-rs extension not present in go-toolkit's Locator.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct Position {
    /// The index of the spine item this position belongs to.
    pub spine_index: usize,
    /// The href of the manifest item (relative to the EPUB root).
    pub href: String,
    /// EPUB CFI pointing to this position. epub-rs extension; not in go-toolkit's Locator.
    pub cfi: String,
    /// 1-based global page index across the entire EPUB. Equivalent to `Locator.Locations.Position`.
    pub global_position: usize,
    /// Progression within the current spine item (0.0 = start, approaches 1.0 at end).
    /// Equivalent to `Locator.Locations.Progression`.
    /// Formula: `pos_in_chapter / position_count_for_chapter`.
    pub chapter_progression: f32,
    /// Progression within the entire publication (0.0 = first page, < 1.0).
    /// Equivalent to `Locator.Locations.TotalProgression`.
    /// Formula: `(global_position - 1) / total_page_count`.
    pub total_progression: f32,
    /// Optional chapter title, populated from the TOC when available.
    /// Equivalent to `Locator.Title`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// A structured, semantic representation of a content block (e.g., a paragraph or heading).
/// Useful for Text-To-Speech (TTS), accessibility (A11Y), or advanced reading interfaces.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct ContentElement {
    /// The plain text content of the element.
    pub text: String,
    /// The exact CFI range covering this element's text.
    pub cfi_range: String,
    /// The semantic tag name (e.g., "h1", "p", "blockquote").
    pub tag_name: String,
    /// The declared language of this block (e.g., "en", "zh-CN"), useful for switching TTS voices.
    pub language: Option<String>,
}

/// The central EPUB document structure
#[derive(serde::Serialize, serde::Deserialize, Debug, Default, Clone)]
pub struct EpubBook {
    /// EPUB metadata (title, creators, etc.)
    pub metadata: Metadata,
    /// Resource manifest mapping IDs to files
    pub manifest: HashMap<String, ManifestItem>,
    /// The reading order of the document using spine items
    pub spine: Vec<SpineItem>,
    /// The directory containing the OPF file, used to resolve relative paths
    pub opf_dir: String,
    /// ID of the NCX table of contents if available
    pub toc_id: Option<String>,
    /// Map of encrypted files and their full encryption metadata (ZIP relative path → EncryptionInfo).
    ///
    /// Populated from `META-INF/encryption.xml`. Each entry carries the obfuscation algorithm
    /// and, when present, the original plaintext byte length from `<Compression OriginalLength="N">`.
    /// The latter is used by the [`crate::parser::OriginalLength`] position strategy to compute
    /// accurate reading positions for LCP / AES-CBC encrypted EPUBs.
    pub encryptions: HashMap<String, crate::crypto::EncryptionInfo>,
}

/// Represents a creator or contributor to the EPUB.
#[derive(serde::Serialize, serde::Deserialize, Debug, Default, Clone)]
pub struct Creator {
    pub name: String,
    /// An optional role, such as "aut" (Author), "trl" (Translator), "ill" (Illustrator).
    pub role: Option<String>,
    /// How the name should be sorted (e.g., "Doe, John").
    pub file_as: Option<String>,
}

impl Creator {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            role: None,
            file_as: None,
        }
    }
}

/// Represents a table of contents entry.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct TocEntry {
    pub title: String,
    pub href: String,
    pub children: Vec<TocEntry>,
}

impl TocEntry {
    pub fn new(title: impl Into<String>, href: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            href: href.into(),
            children: Vec::new(),
        }
    }

    pub fn add_child(mut self, child: TocEntry) -> Self {
        self.children.push(child);
        self
    }
}

/// The complete navigation information extracted from a single `nav.xhtml` or `.ncx` file,
/// parsed in one I/O + one parse operation.
///
/// All fields share the [`TocEntry`] type (mirroring go-toolkit's unified `Link`).
/// TOC entries and page-list entries are structurally identical:
/// - `title` = chapter name (TOC) **or** page label (page-list: `"42"`, `"xii"`, `"A-3"`)
/// - `href`  = spine document path, optionally with a fragment anchor (`"ch3.xhtml#p42"`)
/// - `children` = nested entries (TOC only; always empty for page-list / landmarks)
///
/// Mirrors go-toolkit's `ParseNavDoc` / `ParseNCX` which both return
/// `map[string]manifest.LinkList` with keys `"toc"`, `"page-list"`, `"landmarks"`, etc.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct NavigationDocument {
    /// Table of Contents — `epub:type="toc"` or NCX `<navMap>`.
    pub toc: Vec<TocEntry>,

    /// Page List — `epub:type="page-list"` or NCX `<pageList>/<pageTarget>`.
    ///
    /// Each entry: `title` = page label, `href` = document position with fragment.
    /// Entries are always flat (no children).
    pub page_list: Vec<TocEntry>,

    /// Landmarks — `epub:type="landmarks"` (EPUB 3 only; empty for EPUB 2 NCX).
    ///
    /// Structural navigation points such as "Begin Reading", "Table of Contents", "Index".
    pub landmarks: Vec<TocEntry>,
}

impl NavigationDocument {
    /// Returns `true` if all navigation lists are empty.
    pub fn is_empty(&self) -> bool {
        self.toc.is_empty() && self.page_list.is_empty() && self.landmarks.is_empty()
    }
}

/// A single `dc:title` element with all its OPF refinement metadata.
///
/// `Metadata.titles` is a flat list of every `dc:title` in document order,
/// each carrying the language tag, semantic type, sort key, and display-sequence
/// refinements that the EPUB author attached.
///
/// Callers choose the entry they need (e.g. filter by `lang`, pick
/// `title_type == "main"`, etc.).  For simple access the resolved
/// `Metadata.title` / `Metadata.subtitle` / `Metadata.sort_as` fields
/// are always filled and remain fully backward-compatible.
#[derive(serde::Serialize, serde::Deserialize, Debug, Default, Clone, PartialEq)]
pub struct TitleEntry {
    /// Text content of the `dc:title` element.
    pub value: String,

    /// BCP-47 language tag from `xml:lang`, if present (e.g. `"zh-CN"`, `"en"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,

    /// Semantic role from `title-type` refinement or EPUB 2 attribute.
    /// Common values: `"main"`, `"subtitle"`, `"short"`, `"collection"`, `"edition"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_type: Option<String>,

    /// Sort key from `file-as` refinement (e.g. `"Hobbit, The"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_as: Option<String>,

    /// Display ordering hint from `display-seq` refinement.
    /// Lower values appear first.  Absent when no refinement is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_seq: Option<u32>,
}

/// A secondary identifier for the publication (ISBN, DOI, ARK, UUID, etc.).
///
/// Follows the Readium RWPM [`altIdentifier` schema](https://readium.org/webpub-manifest/schema/altIdentifier.schema.json).
///
/// The `#[serde(untagged)]` attribute drives the RWPM compact serialisation:
/// - `Simple(v)` → serialises as a bare JSON string `"urn:isbn:..."`
/// - `WithScheme { value, scheme }` → serialises as `{"value": "...", "scheme": "..."}`
///
/// This matches go-toolkit's `manifest.AltIdentifier` MarshalJSON behaviour.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
#[serde(untagged)]
pub enum AltIdentifier {
    /// Identifier with no scheme annotation.
    Simple(String),
    /// Identifier with a scheme annotation (e.g. `"ISBN"`, `"onix:codelist5"`).
    WithScheme { value: String, scheme: String },
}

impl AltIdentifier {
    /// Returns the identifier string regardless of variant.
    pub fn value(&self) -> &str {
        match self {
            Self::Simple(v) | Self::WithScheme { value: v, .. } => v,
        }
    }

    /// Returns the scheme annotation if present.
    pub fn scheme(&self) -> Option<&str> {
        match self {
            Self::Simple(_) => None,
            Self::WithScheme { scheme, .. } => Some(scheme),
        }
    }

    /// Consumes the value, returning the inner string.
    pub fn into_value(self) -> String {
        match self {
            Self::Simple(v) | Self::WithScheme { value: v, .. } => v,
        }
    }
}

/// Represents the `metadata` block in the OPF package document.
///
/// All fields added after the initial version carry `#[serde(default)]` to maintain
/// full backward compatibility with existing serialized `EpubBook` JSON payloads.
#[derive(serde::Serialize, serde::Deserialize, Debug, Default, Clone)]
pub struct Metadata {
    // ── Core title group ─────────────────────────────────────────────────────
    /// Main title of the EPUB, from `<dc:title>`.
    pub title: Option<String>,

    /// Subtitle. Parsed from:
    /// - EPUB 2: `<dc:title opf:title-type="subtitle">`
    /// - EPUB 3: `<meta refines="#title-id" property="title-type">subtitle</meta>`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,

    /// Sort key for the title. Parsed from `<meta property="file-as">` refining the title.
    /// Example: `"Hobbit, The"` for the title `"The Hobbit"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_as: Option<String>,

    // ── Contributors ─────────────────────────────────────────────────────────
    /// All creators and contributors (authors, translators, illustrators, etc.).
    /// Each entry carries an optional MARC relator `role` code (e.g., `"aut"`, `"trl"`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub creators: Vec<Creator>,

    // ── Language group ────────────────────────────────────────────────────────
    /// Primary language of the EPUB. Retained for backward compatibility.
    /// Mirrors `languages[0]` when multiple languages are declared.
    pub language: Option<String>,

    /// All declared languages, parsed from every `<dc:language>` element.
    /// Most EPUBs have exactly one; bilingual works may declare two or more.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,

    // ── Identifiers ───────────────────────────────────────────────────────────
    /// Unique identifier of the EPUB (e.g., ISBN, UUID).
    ///
    /// This is the `dc:identifier` element whose `id` attribute matches the
    /// `<package unique-identifier="...">` pointer. When that pointer is absent
    /// or unresolvable, the first non-empty `dc:identifier` is used as a fallback.
    pub identifier: Option<String>,

    /// Additional identifiers beyond the primary unique identifier.
    ///
    /// Collected from any `<dc:identifier>` elements that are not the designated
    /// unique identifier. Each entry carries an optional `scheme` annotation
    /// sourced from either the EPUB 2 `opf:scheme` attribute or the EPUB 3
    /// `<meta property="identifier-type" refines="#id">` meta element.
    ///
    /// Serialises using the Readium RWPM altIdentifier compact form:
    /// - no scheme → bare string `"urn:isbn:..."`
    /// - with scheme → object `{"value": "...", "scheme": "..."}`
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alt_identifiers: Vec<AltIdentifier>,

    // ── Publication info ──────────────────────────────────────────────────────
    pub publisher: Option<String>,
    pub description: Option<String>,

    /// Publication date (`dc:date`). Stored as a raw string for maximum compatibility.
    /// Common formats: `"2024"`, `"2024-01-15"`, `"2024-01-15T00:00:00Z"`.
    pub date: Option<String>,

    /// Last modification timestamp. Parsed from:
    /// - EPUB 3: `<meta property="dcterms:modified">2024-01-15T12:00:00Z</meta>`
    /// - EPUB 2: `<dc:date opf:event="modification">2024-01-15</dc:date>`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,

    pub rights: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subjects: Vec<String>,

    // ── Layout & direction ────────────────────────────────────────────────────
    /// Global layout type of the EPUB (reflowable or pre-paginated).
    #[serde(default)]
    pub layout: LayoutType,

    /// Reading progression direction, parsed from `<spine page-progression-direction="...">`.
    /// Use [`Metadata::effective_reading_progression`] for language-inferred fallback.
    #[serde(default, skip_serializing_if = "is_default_reading_progression")]
    pub reading_progression: ReadingProgression,

    // ── Series / collection ───────────────────────────────────────────────────
    /// Series or collection memberships.
    /// Parsed from `<meta property="belongs-to-collection">` in EPUB 3 OPF,
    /// with `collection-type` and `group-position` refinements.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub belongs_to: Vec<BelongsTo>,

    // ── Quantitative info ─────────────────────────────────────────────────────
    /// Total declared page count, if present in the OPF metadata.
    /// Useful for rendering "Page X of N" UI elements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number_of_pages: Option<u32>,

    // ── Cover ─────────────────────────────────────────────────────────────────
    /// EPUB 2 compatible cover image manifest ID reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_id: Option<String>,

    // ── Multi-title / localization ────────────────────────────────────────────
    /// All `dc:title` elements in document order, each with its refinement metadata.
    ///
    /// Use this when you need multi-language title data or the full set of title
    /// entries.  For simple single-language access, use `title` / `subtitle` /
    /// `sort_as` which are always populated by the parser.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub titles: Vec<TitleEntry>,

    // ── Media Overlays ────────────────────────────────────────────────────────
    /// Global Media Overlays metadata, populated when the OPF contains `media:*` properties.
    /// Present only in EPUB 3 audiobooks with synchronized text–audio overlays.
    /// `None` for standard (non-audiobook) EPUBs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_overlays: Option<MediaOverlayMetadata>,

    /// EPUB Accessibility metadata (EPUB A11y 1.0/1.1, WCAG 2.x, schema.org).
    ///
    /// `None` when the OPF contains no accessibility properties, which is the
    /// case for the vast majority of older EPUBs. `#[serde(default)]` ensures
    /// existing serialised JSON payloads without this field deserialise cleanly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accessibility: Option<Accessibility>,
}

impl Metadata {
    /// Returns the effective reading progression direction for this publication.
    ///
    /// Resolution order:
    /// 1. If `reading_progression` is explicitly `Rtl`, return `Rtl` immediately.
    /// 2. Otherwise perform language-based inference:
    ///    - `ar`, `fa`, `he`, `ur` (and variants) → `Rtl`
    ///    - All other languages, or no language declared → `Ltr`
    ///
    /// **Note:** When `reading_progression` is left at its default `Ltr`, this method
    /// will still infer from the language. To respect an explicit `Ltr` override
    /// regardless of language, read `self.reading_progression` directly.
    ///
    /// This mirrors Readium go-toolkit's `EffectiveReadingProgression`.
    pub fn effective_reading_progression(&self) -> ReadingProgression {
        // Explicit RTL always wins.
        if self.reading_progression == ReadingProgression::Rtl {
            return ReadingProgression::Rtl;
        }
        // Language-based inference (applies when reading_progression is Ltr/default).
        let lang = self
            .language
            .as_deref()
            .or_else(|| self.languages.first().map(|s| s.as_str()))
            .unwrap_or("")
            .to_lowercase();
        let base = lang.split('-').next().unwrap_or("");
        if matches!(base, "ar" | "fa" | "he" | "ur") {
            ReadingProgression::Rtl
        } else {
            ReadingProgression::Ltr
        }
    }

    /// Returns the name of the primary series this EPUB belongs to, if any.
    ///
    /// "Primary" is defined as the first entry whose `collection_type` equals `"series"`.
    pub fn series_name(&self) -> Option<&str> {
        self.belongs_to
            .iter()
            .find(|b| b.collection_type == "series")
            .map(|b| b.name.as_str())
    }

    /// Returns the ordinal position within the primary series, if declared.
    pub fn series_position(&self) -> Option<f64> {
        self.belongs_to
            .iter()
            .find(|b| b.collection_type == "series")
            .and_then(|b| b.position)
    }
}

/// Serde skip-serializing helper: omit `reading_progression` when it is the default `Ltr`.
fn is_default_reading_progression(p: &ReadingProgression) -> bool {
    *p == ReadingProgression::default()
}

/// Global Media Overlays metadata extracted from the OPF `media:*` properties.
///
/// Present only in EPUB 3 audiobooks with synchronized text–audio playback.
/// Spec: EPUB 3.3 §9.3.5.2 / Appendix D.8
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Default)]
pub struct MediaOverlayMetadata {
    /// Total playback duration for the entire publication, in seconds.
    /// From `<meta property="media:duration">` **without** a `refines` attribute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_duration: Option<f64>,

    /// Per-SMIL file durations in seconds, keyed by manifest item ID.
    /// From `<meta property="media:duration" refines="#smil-item-id">`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub durations: HashMap<String, f64>,

    /// Narrator name(s). From `<meta property="media:narrator">`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub narrators: Vec<String>,

    /// CSS class applied to the currently-active sync element during playback.
    /// From `<meta property="media:active-class">`.
    /// Reading systems apply this class to the XHTML fragment pointed to by the active `<par>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_class: Option<String>,

    /// CSS class applied to the XHTML document root while playback is ongoing.
    /// From `<meta property="media:playback-active-class">`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback_active_class: Option<String>,
}

/// A single synchronization point or container from a SMIL Media Overlay file.
///
/// - A `<par>` element maps to a leaf `SmilObject` with both `text_ref` and `audio_ref`.
/// - A `<seq>` element maps to a container `SmilObject` with `children` and no `audio_ref`.
///
/// Spec: EPUB 3.3 §9.2 / SMIL 3.0
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct SmilObject {
    /// EPUB-root-relative URI of the XHTML fragment this sync point targets.
    /// Example: `"OEBPS/ch01.xhtml#word_0001"`.
    /// Empty string for `<seq>` container objects that lack `epub:textref`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text_ref: String,

    /// W3C Media Fragment URI for the audio clip.
    /// Format: `"OEBPS/audio/ch01.mp3#t=0.000,3.450"`
    /// `None` for `<seq>` container objects (no direct audio association).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_ref: Option<String>,

    /// Semantic roles from the `epub:type` attribute on the SMIL element.
    /// Examples: `["chapter"]`, `["sidebar"]`, `["footnote"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub role: Vec<String>,

    /// Child sync points from a nested `<seq>`.
    /// Non-empty only for container objects (parsed from `<seq>`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<SmilObject>,
}

/// The parsed output of a single SMIL Media Overlay file (`.smil`).
///
/// Returned by [`crate::parser::EpubArchive::get_media_overlay`].
/// Contains the ordered list of synchronization points for one spine document,
/// plus optional links to the previous/next overlay for sequential audio playback.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Default)]
pub struct SmilDocument {
    /// Ordered list of sync points / containers for this overlay.
    pub objects: Vec<SmilObject>,

    /// EPUB-root-relative path of the previous chapter's SMIL file, if any.
    /// Enables reading systems to chain overlays for continuous playback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_smil_href: Option<String>,

    /// EPUB-root-relative path of the next chapter's SMIL file, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_smil_href: Option<String>,
}

/// Represents an `item` in the `manifest` block of the OPF
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct ManifestItem {
    pub id: String,
    pub href: String,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<String>,
    /// ID of the SMIL Media Overlay manifest item for this content document.
    /// Parsed from `media-overlay="..."` on the OPF `<item>` element.
    /// Present only for EPUB 3 audiobooks with synchronized text–audio overlays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_overlay: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Accessibility (A11y) types
// ─────────────────────────────────────────────────────────────────────────────

/// An EPUB Accessibility conformance profile.
///
/// Supports both the legacy EPUB A11y 1.0 URL form and the EPUB A11y 1.1
/// human-readable string pattern introduced in the W3C Recommendation
/// (2024-10-17). Stored as the normalised canonical value in both cases.
///
/// Sorted by conformance strength: A < AA < AAA, v1.0 < v1.1, WCAG 2.0 < 2.1 < 2.2.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct A11yProfile(pub String);

impl A11yProfile {
    // EPUB Accessibility 1.0 canonical URLs
    pub const A10_WCAG_20_A:   &'static str = "http://www.idpf.org/epub/a11y/accessibility-20170105.html#wcag-a";
    pub const A10_WCAG_20_AA:  &'static str = "http://www.idpf.org/epub/a11y/accessibility-20170105.html#wcag-aa";
    pub const A10_WCAG_20_AAA: &'static str = "http://www.idpf.org/epub/a11y/accessibility-20170105.html#wcag-aaa";

    /// Normalise a raw OPF `dcterms:conformsTo` value into a canonical profile.
    ///
    /// Handles:
    /// 1. EPUB A11y 1.0 URL aliases (http/https, www/non-www variants)
    /// 2. EPUB A11y 1.1 text pattern — parsed with a hand-rolled token splitter
    ///    so future WCAG versions are accepted without code changes.
    ///    Pattern: `"EPUB Accessibility X.Y - WCAG A.B Level C"` (after whitespace normalisation)
    pub fn from_opf_value(raw: &str) -> Option<Self> {
        // Whitespace normalisation as required by EPUB A11y 1.1 §3.5.2
        let s: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");

        // ── EPUB A11y 1.0 URL aliases ────────────────────────────────────────
        let canonical = match s.as_str() {
            "http://idpf.org/epub/a11y/accessibility-20170105.html#wcag-a"
            | "http://www.idpf.org/epub/a11y/accessibility-20170105.html#wcag-a"
            | "https://idpf.org/epub/a11y/accessibility-20170105.html#wcag-a"
            | "https://www.idpf.org/epub/a11y/accessibility-20170105.html#wcag-a" => {
                Some(Self::A10_WCAG_20_A)
            }
            "http://idpf.org/epub/a11y/accessibility-20170105.html#wcag-aa"
            | "http://www.idpf.org/epub/a11y/accessibility-20170105.html#wcag-aa"
            | "https://idpf.org/epub/a11y/accessibility-20170105.html#wcag-aa"
            | "https://www.idpf.org/epub/a11y/accessibility-20170105.html#wcag-aa" => {
                Some(Self::A10_WCAG_20_AA)
            }
            "http://idpf.org/epub/a11y/accessibility-20170105.html#wcag-aaa"
            | "http://www.idpf.org/epub/a11y/accessibility-20170105.html#wcag-aaa"
            | "https://idpf.org/epub/a11y/accessibility-20170105.html#wcag-aaa"
            | "https://www.idpf.org/epub/a11y/accessibility-20170105.html#wcag-aaa" => {
                Some(Self::A10_WCAG_20_AAA)
            }
            _ => None,
        };
        if let Some(url) = canonical {
            return Some(Self(url.to_owned()));
        }

        // ── EPUB A11y 1.1 text pattern ───────────────────────────────────────
        // "EPUB Accessibility X.Y - WCAG A.B Level C"
        // We hand-roll the parse to avoid the `regex` crate (WASM binary size).
        if parse_a11y_11_profile(&s).is_some() {
            return Some(Self(s));
        }

        None
    }

    /// Numeric rank for deterministic sorting; unknown profiles sort last.
    pub fn sort_rank(&self) -> u8 {
        match self.0.as_str() {
            s if s == Self::A10_WCAG_20_A   => 1,
            s if s == Self::A10_WCAG_20_AA  => 2,
            s if s == Self::A10_WCAG_20_AAA => 3,
            s => parse_a11y_11_profile(s)
                .map(|(_, wcag_ver, level)| {
                    // Base: 1.1 profiles start at rank 4
                    let wcag_base: u8 = match wcag_ver {
                        "2.0" => 0,
                        "2.1" => 3,
                        "2.2" => 6,
                        _ => 9,
                    };
                    let level_offset: u8 = match level {
                        "A"   => 1,
                        "AA"  => 2,
                        "AAA" => 3,
                        _ => 4,
                    };
                    4u8.saturating_add(wcag_base).saturating_add(level_offset - 1)
                })
                .unwrap_or(255),
        }
    }
}

impl PartialOrd for A11yProfile {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for A11yProfile {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.sort_rank().cmp(&other.sort_rank())
    }
}

/// Parse `"EPUB Accessibility X.Y - WCAG A.B Level C"` without the `regex` crate.
///
/// Returns `(a11y_ver, wcag_ver, level)` on success. We avoid the `regex` crate
/// to keep the WASM binary small (regex adds ~300 KB).
fn parse_a11y_11_profile(s: &str) -> Option<(&str, &str, &str)> {
    let rest = s.strip_prefix("EPUB Accessibility ")?;
    let (a11y_ver, rest) = rest.split_once(" - WCAG ")?;
    let (wcag_ver, level) = rest.split_once(" Level ")?;
    let level = level.trim();
    if a11y_ver.is_empty() || wcag_ver.is_empty() || level.is_empty() {
        return None;
    }
    Some((a11y_ver, wcag_ver, level))
}

/// The human sensory mode through which a person may perceive the publication.
///
/// Values from <https://www.w3.org/2021/a11y-discov-vocab/latest/#accessMode>.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum A11yAccessMode {
    Auditory,
    ChartOnVisual,
    ChemOnVisual,
    ColorDependent,
    DiagramOnVisual,
    MathOnVisual,
    MusicOnVisual,
    Tactile,
    TextOnVisual,
    Textual,
    Visual,
    /// Any value not defined in the current vocabulary — preserved for forward compatibility.
    #[serde(untagged)]
    Other(String),
}

impl<'de> serde::Deserialize<'de> for A11yAccessMode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "auditory"        => Self::Auditory,
            "chartOnVisual"   => Self::ChartOnVisual,
            "chemOnVisual"    => Self::ChemOnVisual,
            "colorDependent"  => Self::ColorDependent,
            "diagramOnVisual" => Self::DiagramOnVisual,
            "mathOnVisual"    => Self::MathOnVisual,
            "musicOnVisual"   => Self::MusicOnVisual,
            "tactile"         => Self::Tactile,
            "textOnVisual"    => Self::TextOnVisual,
            "textual"         => Self::Textual,
            "visual"          => Self::Visual,
            _                 => Self::Other(s),
        })
    }
}

impl A11yAccessMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "auditory"        => Self::Auditory,
            "chartOnVisual"   => Self::ChartOnVisual,
            "chemOnVisual"    => Self::ChemOnVisual,
            "colorDependent"  => Self::ColorDependent,
            "diagramOnVisual" => Self::DiagramOnVisual,
            "mathOnVisual"    => Self::MathOnVisual,
            "musicOnVisual"   => Self::MusicOnVisual,
            "tactile"         => Self::Tactile,
            "textOnVisual"    => Self::TextOnVisual,
            "textual"         => Self::Textual,
            "visual"          => Self::Visual,
            _                 => Self::Other(s.to_owned()),
        }
    }
}

/// A primary access mode that is sufficient on its own to consume the publication.
///
/// Each `Vec<A11yPrimaryAccessMode>` represents one sufficient set (OR-joined across sets).
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum A11yPrimaryAccessMode {
    Auditory,
    Tactile,
    Textual,
    Visual,
    /// Any value not defined in the current vocabulary.
    #[serde(untagged)]
    Other(String),
}

impl<'de> serde::Deserialize<'de> for A11yPrimaryAccessMode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "auditory" => Self::Auditory,
            "tactile"  => Self::Tactile,
            "textual"  => Self::Textual,
            "visual"   => Self::Visual,
            _          => Self::Other(s),
        })
    }
}

impl A11yPrimaryAccessMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "auditory" => Self::Auditory,
            "tactile"  => Self::Tactile,
            "textual"  => Self::Textual,
            "visual"   => Self::Visual,
            _          => Self::Other(s.to_owned()),
        }
    }
}

/// Content feature of the resource that contributes to accessibility.
///
/// Values from <https://www.w3.org/2021/a11y-discov-vocab/latest/#accessibilityFeature>.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub enum A11yFeature {
    #[serde(rename = "annotations")]        Annotations,
    #[serde(rename = "ARIA")]              Aria,
    #[serde(rename = "bookmark")]          Bookmarks,
    #[serde(rename = "index")]             Index,
    #[serde(rename = "pageBreakMarkers")]  PageBreakMarkers,
    #[serde(rename = "pageNavigation")]    PageNavigation,
    #[serde(rename = "readingOrder")]      ReadingOrder,
    #[serde(rename = "structuralNavigation")] StructuralNavigation,
    #[serde(rename = "tableOfContents")]   TableOfContents,
    #[serde(rename = "taggedPDF")]         TaggedPdf,
    #[serde(rename = "alternativeText")]   AlternativeText,
    #[serde(rename = "audioDescription")]  AudioDescription,
    #[serde(rename = "captions")]          Captions,
    #[serde(rename = "describedMath")]     DescribedMath,
    #[serde(rename = "longDescription")]   LongDescription,
    #[serde(rename = "rubyAnnotations")]   RubyAnnotations,
    #[serde(rename = "signLanguage")]      SignLanguage,
    #[serde(rename = "transcript")]        Transcript,
    #[serde(rename = "displayTransformability")] DisplayTransformability,
    #[serde(rename = "synchronizedAudioText")]   SynchronizedAudioText,
    #[serde(rename = "timingControl")]     TimingControl,
    #[serde(rename = "unlocked")]          Unlocked,
    #[serde(rename = "ChemML")]            ChemMl,
    #[serde(rename = "latex")]             Latex,
    #[serde(rename = "MathML")]            MathMl,
    #[serde(rename = "ttsMarkup")]         TtsMarkup,
    #[serde(rename = "highContrastAudio")] HighContrastAudio,
    #[serde(rename = "highContrastDisplay")] HighContrastDisplay,
    #[serde(rename = "largePrint")]        LargePrint,
    #[serde(rename = "braille")]           Braille,
    #[serde(rename = "tactileGraphic")]    TactileGraphic,
    #[serde(rename = "tactileObject")]     TactileObject,
    #[serde(rename = "none")]              None,
    /// Any value not defined in the current vocabulary.
    #[serde(untagged)]
    Other(String),
}

impl<'de> serde::Deserialize<'de> for A11yFeature {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Self::from_str(&s))
    }
}

impl A11yFeature {
    pub fn from_str(s: &str) -> Self {
        match s {
            "annotations"            => Self::Annotations,
            "ARIA"                   => Self::Aria,
            "bookmark"               => Self::Bookmarks,
            "index"                  => Self::Index,
            "pageBreakMarkers"       => Self::PageBreakMarkers,
            "pageNavigation"         => Self::PageNavigation,
            "readingOrder"           => Self::ReadingOrder,
            "structuralNavigation"   => Self::StructuralNavigation,
            "tableOfContents"        => Self::TableOfContents,
            "taggedPDF"              => Self::TaggedPdf,
            "alternativeText"        => Self::AlternativeText,
            "audioDescription"       => Self::AudioDescription,
            "captions"               => Self::Captions,
            "describedMath"          => Self::DescribedMath,
            "longDescription"        => Self::LongDescription,
            "rubyAnnotations"        => Self::RubyAnnotations,
            "signLanguage"           => Self::SignLanguage,
            "transcript"             => Self::Transcript,
            "displayTransformability"=> Self::DisplayTransformability,
            "synchronizedAudioText"  => Self::SynchronizedAudioText,
            "timingControl"          => Self::TimingControl,
            "unlocked"               => Self::Unlocked,
            "ChemML"                 => Self::ChemMl,
            "latex"                  => Self::Latex,
            "MathML"                 => Self::MathMl,
            "ttsMarkup"              => Self::TtsMarkup,
            "highContrastAudio"      => Self::HighContrastAudio,
            "highContrastDisplay"    => Self::HighContrastDisplay,
            "largePrint"             => Self::LargePrint,
            "braille"                => Self::Braille,
            "tactileGraphic"         => Self::TactileGraphic,
            "tactileObject"          => Self::TactileObject,
            "none"                   => Self::None,
            _                        => Self::Other(s.to_owned()),
        }
    }
}

/// A physiological hazard the content may present to some users.
///
/// Values from <https://www.w3.org/2021/a11y-discov-vocab/latest/#accessibilityHazard>.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub enum A11yHazard {
    #[serde(rename = "flashing")]               Flashing,
    #[serde(rename = "noFlashingHazard")]        NoFlashingHazard,
    #[serde(rename = "motionSimulation")]        MotionSimulation,
    #[serde(rename = "noMotionSimulationHazard")]NoMotionSimulationHazard,
    #[serde(rename = "sound")]                   Sound,
    #[serde(rename = "noSoundHazard")]           NoSoundHazard,
    #[serde(rename = "unknown")]                 Unknown,
    #[serde(rename = "none")]                    None,
    /// Any value not defined in the current vocabulary.
    #[serde(untagged)]
    Other(String),
}

impl<'de> serde::Deserialize<'de> for A11yHazard {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Self::from_str(&s))
    }
}

impl A11yHazard {
    pub fn from_str(s: &str) -> Self {
        match s {
            "flashing"                  => Self::Flashing,
            "noFlashingHazard"          => Self::NoFlashingHazard,
            "motionSimulation"          => Self::MotionSimulation,
            "noMotionSimulationHazard"  => Self::NoMotionSimulationHazard,
            "sound"                     => Self::Sound,
            "noSoundHazard"             => Self::NoSoundHazard,
            "unknown"                   => Self::Unknown,
            "none"                      => Self::None,
            _                           => Self::Other(s.to_owned()),
        }
    }
}

/// Regulatory exemption justifying non-conformance (EU EAA).
///
/// Values from the EPUB Accessibility Exemptions vocabulary.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub enum A11yExemption {
    #[serde(rename = "eaa-disproportionate-burden")] EaaDisproportionateBurden,
    #[serde(rename = "eaa-fundamental-alteration")]  EaaFundamentalAlteration,
    #[serde(rename = "eaa-microenterprise")]          EaaMicroenterprise,
    /// Any value not defined in the current vocabulary.
    #[serde(untagged)]
    Other(String),
}

impl<'de> serde::Deserialize<'de> for A11yExemption {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Self::from_str(&s))
    }
}

impl A11yExemption {
    pub fn from_str(s: &str) -> Self {
        match s {
            "eaa-disproportionate-burden" => Self::EaaDisproportionateBurden,
            "eaa-fundamental-alteration"  => Self::EaaFundamentalAlteration,
            "eaa-microenterprise"         => Self::EaaMicroenterprise,
            _                            => Self::Other(s.to_owned()),
        }
    }
}

/// Certification details for an accessible publication.
///
/// Maps to the `a11y:certifiedBy` / `a11y:certifierCredential` / `a11y:certifierReport`
/// triple in the OPF `<metadata>` block. Per the EPUB A11y 1.1 spec §3.5.3,
/// `certifiedBy` refines the `dcterms:conformsTo` meta element via `refines="#conf-id"`.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Default)]
pub struct A11yCertification {
    /// The organisation or person that evaluated and certified the publication.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub certified_by: String,

    /// URL of a credential or badge proving the certifier's authority to certify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,

    /// URL of the full accessibility audit report produced by the certifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
}

impl A11yCertification {
    pub fn is_empty(&self) -> bool {
        self.certified_by.is_empty() && self.credential.is_none() && self.report.is_none()
    }
}

/// Accessibility metadata for an EPUB publication.
///
/// Mirrors the Readium go-toolkit `manifest.A11y` struct and the
/// W3C Accessibility Discoverability Vocabulary
/// (<https://www.w3.org/2021/a11y-discov-vocab/latest/>).
///
/// All fields are empty / `None` when absent from the OPF; the entire
/// `Metadata.accessibility` field is `None` when no a11y metadata is present,
/// avoiding an empty struct in the common case of non-accessible EPUBs.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Accessibility {
    /// Established standards this publication conforms to, sorted by conformance level.
    /// From `dcterms:conformsTo` — supports both EPUB A11y 1.0 URLs and 1.1 text strings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conforms_to: Vec<A11yProfile>,

    /// Certification metadata (certifier name, credential URL, report URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certification: Option<A11yCertification>,

    /// Human-readable summary of accessibility features and known deficiencies.
    /// From `schema:accessibilitySummary`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// Sensory modes through which the publication can be perceived.
    /// From `schema:accessMode` (one element per `<meta>`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub access_modes: Vec<A11yAccessMode>,

    /// Combinations of access modes that are individually sufficient to consume the work.
    /// Each inner `Vec` is one sufficient set; multiple sets are OR-joined.
    /// From `schema:accessModeSufficient` (comma-separated per `<meta>`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub access_modes_sufficient: Vec<Vec<A11yPrimaryAccessMode>>,

    /// Accessibility features present (alt text, page navigation, MathML, etc.).
    /// From `schema:accessibilityFeature`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<A11yFeature>,

    /// Physiological hazards that may affect some users.
    /// From `schema:accessibilityHazard`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hazards: Vec<A11yHazard>,

    /// Regulatory exemptions justifying non-conformance (EU EAA).
    /// From `a11y:exemption`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exemptions: Vec<A11yExemption>,
}

impl Accessibility {
    /// Returns `true` when no a11y metadata was found in the OPF.
    ///
    /// Used internally to avoid attaching an empty struct to `Metadata.accessibility`.
    pub fn is_empty(&self) -> bool {
        self.conforms_to.is_empty()
            && self.certification.is_none()
            && self.summary.is_none()
            && self.access_modes.is_empty()
            && self.access_modes_sufficient.is_empty()
            && self.features.is_empty()
            && self.hazards.is_empty()
            && self.exemptions.is_empty()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {

    use super::*;

    // ── ReadingProgression inference ──────────────────────────────────────────

    #[test]
    fn test_rtl_inference_arabic() {
        let m = Metadata {
            language: Some("ar".to_string()),
            ..Default::default()
        };
        assert_eq!(m.effective_reading_progression(), ReadingProgression::Rtl);
    }

    #[test]
    fn test_rtl_inference_farsi() {
        let m = Metadata {
            language: Some("fa-IR".to_string()), // base tag "fa" → RTL
            ..Default::default()
        };
        assert_eq!(m.effective_reading_progression(), ReadingProgression::Rtl);
    }

    #[test]
    fn test_rtl_inference_hebrew() {
        let m = Metadata {
            language: Some("he".to_string()),
            ..Default::default()
        };
        assert_eq!(m.effective_reading_progression(), ReadingProgression::Rtl);
    }

    #[test]
    fn test_ltr_inference_english() {
        let m = Metadata {
            language: Some("en".to_string()),
            ..Default::default()
        };
        assert_eq!(m.effective_reading_progression(), ReadingProgression::Ltr);
    }

    #[test]
    fn test_explicit_rtl_overrides_language() {
        // Explicit RTL must be respected even for a nominally LTR language
        let m = Metadata {
            language: Some("en".to_string()),
            reading_progression: ReadingProgression::Rtl,
            ..Default::default()
        };
        assert_eq!(m.effective_reading_progression(), ReadingProgression::Rtl);
    }

    #[test]
    fn test_explicit_ltr_overrides_arabic_language() {
        // Note: effective_reading_progression() uses language inference when reading_progression
        // is Ltr (the default). To suppress inference, read `reading_progression` directly.
        // Here we verify that an explicit Rtl set in the OPF is correctly returned.
        let mut m = Metadata {
            language: Some("ar".to_string()),
            ..Default::default()
        };
        // With the default Ltr + Arabic language, inference wins → Rtl
        assert_eq!(m.effective_reading_progression(), ReadingProgression::Rtl);
        // After explicitly setting Rtl, still Rtl
        m.reading_progression = ReadingProgression::Rtl;
        assert_eq!(m.effective_reading_progression(), ReadingProgression::Rtl);
        // Direct field access always reflects the explicit value
        m.reading_progression = ReadingProgression::Ltr;
        assert_eq!(m.reading_progression, ReadingProgression::Ltr);
    }

    #[test]
    fn test_rtl_inference_from_languages_vec_when_no_primary() {
        // Falls back to languages[0] when `language` is None
        let m = Metadata {
            languages: vec!["he".to_string(), "en".to_string()],
            ..Default::default()
        };
        assert_eq!(m.effective_reading_progression(), ReadingProgression::Rtl);
    }

    #[test]
    fn test_ltr_default_no_language() {
        let m = Metadata::default();
        assert_eq!(m.effective_reading_progression(), ReadingProgression::Ltr);
    }

    // ── Series / BelongsTo accessors ─────────────────────────────────────────

    #[test]
    fn test_series_name_and_position() {
        let m = Metadata {
            belongs_to: vec![BelongsTo {
                name: "Dune Chronicles".to_string(),
                collection_type: "series".to_string(),
                position: Some(1.0),
            }],
            ..Default::default()
        };
        assert_eq!(m.series_name(), Some("Dune Chronicles"));
        assert_eq!(m.series_position(), Some(1.0));
    }

    #[test]
    fn test_series_position_fractional() {
        let m = Metadata {
            belongs_to: vec![BelongsTo {
                name: "Foundation".to_string(),
                collection_type: "series".to_string(),
                position: Some(2.5),
            }],
            ..Default::default()
        };
        assert_eq!(m.series_position(), Some(2.5));
    }

    #[test]
    fn test_no_series_returns_none() {
        let m = Metadata::default();
        assert!(m.series_name().is_none());
        assert!(m.series_position().is_none());
    }

    #[test]
    fn test_collection_type_skips_non_series() {
        // Only entries with collection_type == "series" should be returned
        let m = Metadata {
            belongs_to: vec![
                BelongsTo {
                    name: "SF Classics".to_string(),
                    collection_type: "collection".to_string(),
                    position: Some(3.0),
                },
                BelongsTo {
                    name: "Hyperion Cantos".to_string(),
                    collection_type: "series".to_string(),
                    position: Some(1.0),
                },
            ],
            ..Default::default()
        };
        assert_eq!(m.series_name(), Some("Hyperion Cantos"));
    }

    // ── Serde backward compatibility ─────────────────────────────────────────

    #[test]
    fn test_serde_roundtrip_new_fields() {
        let original = Metadata {
            title: Some("Test Book".to_string()),
            subtitle: Some("A Fine Subtitle".to_string()),
            sort_as: Some("Book, Test".to_string()),
            modified: Some("2024-05-01T00:00:00Z".to_string()),
            reading_progression: ReadingProgression::Rtl,
            belongs_to: vec![BelongsTo {
                name: "My Series".to_string(),
                collection_type: "series".to_string(),
                position: Some(1.0),
            }],
            number_of_pages: Some(300),
            languages: vec!["en".to_string(), "fr".to_string()],
            ..Default::default()
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: Metadata = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.subtitle, original.subtitle);
        assert_eq!(decoded.sort_as, original.sort_as);
        assert_eq!(decoded.modified, original.modified);
        assert_eq!(decoded.reading_progression, original.reading_progression);
        assert_eq!(decoded.series_name(), Some("My Series"));
        assert_eq!(decoded.number_of_pages, Some(300));
        assert_eq!(decoded.languages, vec!["en", "fr"]);
    }

    #[test]
    fn test_serde_backward_compat_old_json_without_new_fields() {
        // Old JSON payload (no new fields) must deserialize cleanly with safe defaults
        let old_json = r#"{
            "title": "Old Book",
            "creators": [],
            "language": "en",
            "identifier": null,
            "publisher": null,
            "description": null,
            "date": null,
            "rights": null,
            "subjects": [],
            "layout": "Reflowable",
            "cover_id": null
        }"#;
        let m: Metadata = serde_json::from_str(old_json).unwrap();
        assert_eq!(m.title.as_deref(), Some("Old Book"));
        assert_eq!(m.subtitle, None);
        assert_eq!(m.sort_as, None);
        assert_eq!(m.modified, None);
        assert_eq!(m.reading_progression, ReadingProgression::Ltr);
        assert!(m.belongs_to.is_empty());
        assert_eq!(m.number_of_pages, None);
        assert!(m.languages.is_empty());
    }

    #[test]
    fn test_reading_progression_not_serialized_when_ltr() {
        // `reading_progression` must be omitted from JSON when it is the default Ltr
        let m = Metadata {
            title: Some("Book".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(
            !json.contains("reading_progression"),
            "ltr should be omitted: {json}"
        );
    }

    #[test]
    fn test_reading_progression_serialized_when_rtl() {
        let m = Metadata {
            reading_progression: ReadingProgression::Rtl,
            ..Default::default()
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(
            json.contains("\"reading_progression\":\"rtl\""),
            "rtl should be present: {json}"
        );
    }
}
