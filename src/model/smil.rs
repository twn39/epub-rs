use std::collections::HashMap;

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
    /// Enables reading systems to chain overlays for continuous playback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_smil_href: Option<String>,
}
