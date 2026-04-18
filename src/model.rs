//! EPUB Domain Models

use std::collections::HashMap;

/// Specifies the version of the EPUB standard to target during generation.
#[derive(serde::Serialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EpubVersion {
    /// EPUB 2.0 (Compatible with older e-readers, relies on NCX)
    V20,
    /// EPUB 3.0 (Modern standard, uses HTML5 navigation and semantic tags)
    #[default]
    V30,
}

/// The layout rendition type of the EPUB.
#[derive(serde::Serialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutType {
    /// Content flows dynamically to fit the screen (default).
    #[default]
    Reflowable,
    /// Content is pre-paginated with fixed dimensions (e.g. comics, children's books).
    PrePaginated,
}

/// Hints for how a fixed-layout spine item should be displayed in a synthetic spread.
#[derive(serde::Serialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSpread {
    None,
    Left,
    Right,
    Center,
}

/// Represents an item in the reading order (spine).
#[derive(serde::Serialize, Debug, Clone)]
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
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
pub struct Position {
    /// The index of the spine item this position belongs to.
    pub spine_index: usize,
    /// The href of the manifest item.
    pub href: String,
    /// The exact CFI pointing to this position.
    pub cfi: String,
    /// The global position index (1-based) across the entire EPUB.
    pub global_position: usize,
    /// The progression within the current chapter (0.0 to 1.0).
    pub chapter_progression: f32,
    /// The overall progression within the entire EPUB (0.0 to 1.0).
    pub total_progression: f32,
}

/// A structured, semantic representation of a content block (e.g., a paragraph or heading).
/// Useful for Text-To-Speech (TTS), accessibility (A11Y), or advanced reading interfaces.
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
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
#[derive(serde::Serialize, Debug, Default, Clone)]
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
    /// Map of encrypted files and their algorithm (ZIP relative path -> ObfuscationAlgorithm)
    pub encryptions: HashMap<String, crate::crypto::ObfuscationAlgorithm>,
}

/// Represents a creator or contributor to the EPUB.
#[derive(serde::Serialize, Debug, Default, Clone)]
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
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
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

/// Represents the `metadata` block in the OPF
#[derive(serde::Serialize, Debug, Default, Clone)]
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
    /// EPUB 2 compatible cover image ID reference
    pub cover_id: Option<String>,
}

/// Represents an `item` in the `manifest` block of the OPF
#[derive(serde::Serialize, Debug, Clone)]
pub struct ManifestItem {
    pub id: String,
    pub href: String,
    pub media_type: String,
    pub properties: Option<String>,
}
