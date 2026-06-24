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
