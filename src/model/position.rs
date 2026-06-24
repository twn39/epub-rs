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
