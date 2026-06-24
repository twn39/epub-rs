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
    pub const A10_WCAG_20_A: &'static str =
        "http://www.idpf.org/epub/a11y/accessibility-20170105.html#wcag-a";
    pub const A10_WCAG_20_AA: &'static str =
        "http://www.idpf.org/epub/a11y/accessibility-20170105.html#wcag-aa";
    pub const A10_WCAG_20_AAA: &'static str =
        "http://www.idpf.org/epub/a11y/accessibility-20170105.html#wcag-aaa";

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
            s if s == Self::A10_WCAG_20_A => 1,
            s if s == Self::A10_WCAG_20_AA => 2,
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
                        "A" => 1,
                        "AA" => 2,
                        "AAA" => 3,
                        _ => 4,
                    };
                    4u8.saturating_add(wcag_base)
                        .saturating_add(level_offset - 1)
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
            "auditory" => Self::Auditory,
            "chartOnVisual" => Self::ChartOnVisual,
            "chemOnVisual" => Self::ChemOnVisual,
            "colorDependent" => Self::ColorDependent,
            "diagramOnVisual" => Self::DiagramOnVisual,
            "mathOnVisual" => Self::MathOnVisual,
            "musicOnVisual" => Self::MusicOnVisual,
            "tactile" => Self::Tactile,
            "textOnVisual" => Self::TextOnVisual,
            "textual" => Self::Textual,
            "visual" => Self::Visual,
            _ => Self::Other(s),
        })
    }
}

impl A11yAccessMode {
    #[allow(clippy::should_implement_trait)] // intentional: infallible, returns Self not Result
    pub fn from_str(s: &str) -> Self {
        match s {
            "auditory" => Self::Auditory,
            "chartOnVisual" => Self::ChartOnVisual,
            "chemOnVisual" => Self::ChemOnVisual,
            "colorDependent" => Self::ColorDependent,
            "diagramOnVisual" => Self::DiagramOnVisual,
            "mathOnVisual" => Self::MathOnVisual,
            "musicOnVisual" => Self::MusicOnVisual,
            "tactile" => Self::Tactile,
            "textOnVisual" => Self::TextOnVisual,
            "textual" => Self::Textual,
            "visual" => Self::Visual,
            _ => Self::Other(s.to_owned()),
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
            "tactile" => Self::Tactile,
            "textual" => Self::Textual,
            "visual" => Self::Visual,
            _ => Self::Other(s),
        })
    }
}

impl A11yPrimaryAccessMode {
    #[allow(clippy::should_implement_trait)] // intentional: infallible, returns Self not Result
    pub fn from_str(s: &str) -> Self {
        match s {
            "auditory" => Self::Auditory,
            "tactile" => Self::Tactile,
            "textual" => Self::Textual,
            "visual" => Self::Visual,
            _ => Self::Other(s.to_owned()),
        }
    }
}

/// Content feature of the resource that contributes to accessibility.
///
/// Values from <https://www.w3.org/2021/a11y-discov-vocab/latest/#accessibilityFeature>.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub enum A11yFeature {
    #[serde(rename = "annotations")]
    Annotations,
    #[serde(rename = "ARIA")]
    Aria,
    #[serde(rename = "bookmark")]
    Bookmarks,
    #[serde(rename = "index")]
    Index,
    #[serde(rename = "pageBreakMarkers")]
    PageBreakMarkers,
    #[serde(rename = "pageNavigation")]
    PageNavigation,
    #[serde(rename = "readingOrder")]
    ReadingOrder,
    #[serde(rename = "structuralNavigation")]
    StructuralNavigation,
    #[serde(rename = "tableOfContents")]
    TableOfContents,
    #[serde(rename = "taggedPDF")]
    TaggedPdf,
    #[serde(rename = "alternativeText")]
    AlternativeText,
    #[serde(rename = "audioDescription")]
    AudioDescription,
    #[serde(rename = "captions")]
    Captions,
    #[serde(rename = "describedMath")]
    DescribedMath,
    #[serde(rename = "longDescription")]
    LongDescription,
    #[serde(rename = "rubyAnnotations")]
    RubyAnnotations,
    #[serde(rename = "signLanguage")]
    SignLanguage,
    #[serde(rename = "transcript")]
    Transcript,
    #[serde(rename = "displayTransformability")]
    DisplayTransformability,
    #[serde(rename = "synchronizedAudioText")]
    SynchronizedAudioText,
    #[serde(rename = "timingControl")]
    TimingControl,
    #[serde(rename = "unlocked")]
    Unlocked,
    #[serde(rename = "ChemML")]
    ChemMl,
    #[serde(rename = "latex")]
    Latex,
    #[serde(rename = "MathML")]
    MathMl,
    #[serde(rename = "ttsMarkup")]
    TtsMarkup,
    #[serde(rename = "highContrastAudio")]
    HighContrastAudio,
    #[serde(rename = "highContrastDisplay")]
    HighContrastDisplay,
    #[serde(rename = "largePrint")]
    LargePrint,
    #[serde(rename = "braille")]
    Braille,
    #[serde(rename = "tactileGraphic")]
    TactileGraphic,
    #[serde(rename = "tactileObject")]
    TactileObject,
    #[serde(rename = "none")]
    None,
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
    #[allow(clippy::should_implement_trait)] // intentional: infallible, returns Self not Result
    pub fn from_str(s: &str) -> Self {
        match s {
            "annotations" => Self::Annotations,
            "ARIA" => Self::Aria,
            "bookmark" => Self::Bookmarks,
            "index" => Self::Index,
            "pageBreakMarkers" => Self::PageBreakMarkers,
            "pageNavigation" => Self::PageNavigation,
            "readingOrder" => Self::ReadingOrder,
            "structuralNavigation" => Self::StructuralNavigation,
            "tableOfContents" => Self::TableOfContents,
            "taggedPDF" => Self::TaggedPdf,
            "alternativeText" => Self::AlternativeText,
            "audioDescription" => Self::AudioDescription,
            "captions" => Self::Captions,
            "describedMath" => Self::DescribedMath,
            "longDescription" => Self::LongDescription,
            "rubyAnnotations" => Self::RubyAnnotations,
            "signLanguage" => Self::SignLanguage,
            "transcript" => Self::Transcript,
            "displayTransformability" => Self::DisplayTransformability,
            "synchronizedAudioText" => Self::SynchronizedAudioText,
            "timingControl" => Self::TimingControl,
            "unlocked" => Self::Unlocked,
            "ChemML" => Self::ChemMl,
            "latex" => Self::Latex,
            "MathML" => Self::MathMl,
            "ttsMarkup" => Self::TtsMarkup,
            "highContrastAudio" => Self::HighContrastAudio,
            "highContrastDisplay" => Self::HighContrastDisplay,
            "largePrint" => Self::LargePrint,
            "braille" => Self::Braille,
            "tactileGraphic" => Self::TactileGraphic,
            "tactileObject" => Self::TactileObject,
            "none" => Self::None,
            _ => Self::Other(s.to_owned()),
        }
    }
}

/// A physiological hazard the content may present to some users.
///
/// Values from <https://www.w3.org/2021/a11y-discov-vocab/latest/#accessibilityHazard>.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub enum A11yHazard {
    #[serde(rename = "flashing")]
    Flashing,
    #[serde(rename = "noFlashingHazard")]
    NoFlashingHazard,
    #[serde(rename = "motionSimulation")]
    MotionSimulation,
    #[serde(rename = "noMotionSimulationHazard")]
    NoMotionSimulationHazard,
    #[serde(rename = "sound")]
    Sound,
    #[serde(rename = "noSoundHazard")]
    NoSoundHazard,
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "none")]
    None,
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
    #[allow(clippy::should_implement_trait)] // intentional: infallible, returns Self not Result
    pub fn from_str(s: &str) -> Self {
        match s {
            "flashing" => Self::Flashing,
            "noFlashingHazard" => Self::NoFlashingHazard,
            "motionSimulation" => Self::MotionSimulation,
            "noMotionSimulationHazard" => Self::NoMotionSimulationHazard,
            "sound" => Self::Sound,
            "noSoundHazard" => Self::NoSoundHazard,
            "unknown" => Self::Unknown,
            "none" => Self::None,
            _ => Self::Other(s.to_owned()),
        }
    }
}

/// Regulatory exemption justifying non-conformance (EU EAA).
///
/// Values from the EPUB Accessibility Exemptions vocabulary.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub enum A11yExemption {
    #[serde(rename = "eaa-disproportionate-burden")]
    EaaDisproportionateBurden,
    #[serde(rename = "eaa-fundamental-alteration")]
    EaaFundamentalAlteration,
    #[serde(rename = "eaa-microenterprise")]
    EaaMicroenterprise,
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
    #[allow(clippy::should_implement_trait)] // intentional: infallible, returns Self not Result
    pub fn from_str(s: &str) -> Self {
        match s {
            "eaa-disproportionate-burden" => Self::EaaDisproportionateBurden,
            "eaa-fundamental-alteration" => Self::EaaFundamentalAlteration,
            "eaa-microenterprise" => Self::EaaMicroenterprise,
            _ => Self::Other(s.to_owned()),
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
