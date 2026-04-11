//! EPUB Domain Models

use std::collections::HashMap;

/// The central EPUB document structure
#[derive(Debug, Default, Clone)]
pub struct EpubBook {
    /// EPUB metadata (title, creators, etc.)
    pub metadata: Metadata,
    /// Resource manifest mapping IDs to files
    pub manifest: HashMap<String, ManifestItem>,
    /// The reading order of the document using item IDs
    pub spine: Vec<String>,
    /// The directory containing the OPF file, used to resolve relative paths
    pub opf_dir: String,
    /// ID of the NCX table of contents if available
    pub toc_id: Option<String>,
}

/// Represents the `metadata` block in the OPF
#[derive(Debug, Default, Clone)]
pub struct Metadata {
    pub title: Option<String>,
    pub creators: Vec<String>,
    pub language: Option<String>,
    pub identifier: Option<String>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub date: Option<String>,
    pub rights: Option<String>,
    pub subjects: Vec<String>,
}

/// Represents an `item` in the `manifest` block of the OPF
#[derive(Debug, Clone)]
pub struct ManifestItem {
    pub id: String,
    pub href: String,
    pub media_type: String,
}
