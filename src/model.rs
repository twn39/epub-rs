//! EPUB Domain Models

use std::collections::HashMap;

/// Specifies the version of the EPUB standard to target during generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum EpubVersion {
    /// EPUB 2.0 (Compatible with older e-readers, relies on NCX)
    V20,
    /// EPUB 3.0 (Modern standard, uses HTML5 navigation and semantic tags)
    #[default]
    V30,
}


/// The layout rendition type of the EPUB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutType {
    /// Content flows dynamically to fit the screen (default).
    #[default]
    Reflowable,
    /// Content is pre-paginated with fixed dimensions (e.g. comics, children's books).
    PrePaginated,
}

/// Hints for how a fixed-layout spine item should be displayed in a synthetic spread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSpread {
    None,
    Left,
    Right,
    Center,
}

/// Represents an item in the reading order (spine).
#[derive(Debug, Clone)]
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

/// The central EPUB document structure
#[derive(Debug, Default, Clone)]
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
}

/// Represents a creator or contributor to the EPUB.
#[derive(Debug, Default, Clone)]
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

/// Represents the `metadata` block in the OPF
#[derive(Debug, Default, Clone)]
pub struct Metadata {
    pub title: Option<String>,
    pub creators: Vec<Creator>,
    pub language: Option<String>,
    pub identifier: Option<String>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub date: Option<String>,
    pub rights: Option<String>,
    pub subjects: Vec<String>,
    /// Global layout type of the EPUB (reflowable or pre-paginated).
    pub layout: LayoutType,
}

/// Represents an `item` in the `manifest` block of the OPF
#[derive(Debug, Clone)]
pub struct ManifestItem {
    pub id: String,
    pub href: String,
    pub media_type: String,
    pub properties: Option<String>,
}
