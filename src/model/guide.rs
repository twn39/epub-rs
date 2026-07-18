//! EPUB 2 OPF `<guide>` references.

/// One entry from the EPUB 2 OPF `<guide>` section.
///
/// Common `type` values: `cover`, `title-page`, `toc`, `index`, `glossary`,
/// `acknowledgements`, `bibliography`, `colophon`, `copyright-page`,
/// `dedication`, `epigraph`, `foreword`, `loi`, `lot`, `notes`, `preface`,
/// `text` (start of main content).
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct GuideReference {
    /// Structural role (`type` attribute).
    #[serde(rename = "type")]
    pub ref_type: String,
    /// EPUB-root-relative path (fragment preserved when present).
    pub href: String,
    /// Optional human-readable title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}
