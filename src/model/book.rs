use super::guide::GuideReference;
use super::manifest::ManifestItem;
use super::metadata::Metadata;
use super::spine::SpineItem;
use std::collections::HashMap;

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
    /// EPUB 2 OPF `<guide>` references (empty for pure EPUB 3 books without a guide).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guide: Vec<GuideReference>,
    /// Map of encrypted files and their full encryption metadata (ZIP relative path → EncryptionInfo).
    ///
    /// Populated from `META-INF/encryption.xml`. Each entry carries the obfuscation algorithm
    /// and, when present, the original plaintext byte length from `<Compression OriginalLength="N">`.
    /// The latter is used by the [`crate::parser::OriginalLength`] position strategy to compute
    /// accurate reading positions for LCP / AES-CBC encrypted EPUBs.
    pub encryptions: HashMap<String, crate::crypto::EncryptionInfo>,
}
