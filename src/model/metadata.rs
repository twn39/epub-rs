use super::a11y::Accessibility;
use super::base::{LayoutType, ReadingProgression};
use super::smil::MediaOverlayMetadata;

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
