/// Specifies the version of the EPUB standard to target during generation.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EpubVersion {
    /// EPUB 2.0 (Compatible with older e-readers, relies on NCX)
    V20,
    /// EPUB 3.0 (Modern standard, uses HTML5 navigation and semantic tags)
    #[default]
    V30,
}

/// The layout rendition type of the EPUB.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutType {
    /// Content flows dynamically to fit the screen (default).
    #[default]
    Reflowable,
    /// Content is pre-paginated with fixed dimensions (e.g. comics, children's books).
    PrePaginated,
}

/// Hints for how a fixed-layout spine item should be displayed in a synthetic spread.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSpread {
    None,
    Left,
    Right,
    Center,
}

/// The reading progression direction of the EPUB publication.
///
/// Parsed from `<spine page-progression-direction="...">` in the OPF package document.
/// When not explicitly set, use [`Metadata::effective_reading_progression`] to obtain
/// a language-inferred value.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ReadingProgression {
    /// Left-to-right (default for Latin, CJK horizontal scripts, etc.)
    #[default]
    Ltr,
    /// Right-to-left (Arabic, Hebrew, Persian, Urdu, Japanese vertical, etc.)
    Rtl,
}

/// Describes a single rendition (OPF package document) declared in `META-INF/container.xml`.
///
/// An EPUB container may include multiple renditions of the same publication — for example a
/// reflowable text version and a fixed-layout (comic/manga) version.  Each rendition is
/// identified by its [`opf_path`][RenditionInfo::opf_path] and may carry optional selection
/// attributes that conformant Reading Systems use to automatically choose the most appropriate
/// rendition for the current device or user preference.
///
/// The **first** entry in the list returned by `EpubArchive::get_renditions()` is always the
/// *default rendition*, which every Reading System must be capable of processing (OCF §3.5.1).
///
/// # EPUB Multiple-Rendition Example
/// ```xml
/// <rootfiles>
///   <!-- Default: pre-paginated (manga) edition -->
///   <rootfile full-path="EPUB/manga.opf"
///             media-type="application/oebps-package+xml"
///             rendition:layout="pre-paginated"
///             rendition:label="漫画版"/>
///   <!-- Alternative: reflowable text edition -->
///   <rootfile full-path="EPUB/text.opf"
///             media-type="application/oebps-package+xml"
///             rendition:layout="reflowable"
///             rendition:label="阅读版"
///             rendition:accessMode="textual"/>
/// </rootfiles>
/// ```
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Default)]
pub struct RenditionInfo {
    /// EPUB-root-relative path to the OPF package document (`full-path` attribute).
    pub opf_path: String,

    /// `rendition:layout` selection hint — `"reflowable"` or `"pre-paginated"`.
    ///
    /// `None` means the attribute was absent; per the EPUB Multiple-Rendition spec the
    /// absence is treated the same as `"reflowable"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,

    /// `rendition:media` — CSS media query string (e.g. `"screen and (min-width:800px)"`).
    ///
    /// Reading Systems evaluate this query against the device's capabilities to decide
    /// which rendition best matches the current viewing environment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<String>,

    /// `rendition:language` — RFC 5646 language tag (e.g. `"zh-Hant"`, `"en"`).
    ///
    /// Useful when the container holds different-language editions of the same title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// `rendition:label` — human-readable name for the rendition.
    ///
    /// Suitable for display in a Reading System's rendition-selection UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,

    /// `rendition:accessMode` — primary access mode (`"textual"`, `"visual"`, `"auditory"`).
    ///
    /// Based on ISO 24751-3; allows accessibility-aware Reading Systems to prefer the
    /// rendition that is most appropriate for users who rely on screen readers or other AT.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_mode: Option<String>,
}

impl RenditionInfo {
    /// Returns `true` if this rendition can reflow to fit the reading surface.
    ///
    /// A missing `layout` attribute is treated as reflowable because the EPUB
    /// Multiple-Rendition spec defines reflowable as the default; it is the mode
    /// every legacy EPUB implicitly uses.
    pub fn is_reflowable(&self) -> bool {
        self.layout.as_deref() != Some("pre-paginated")
    }

    /// Returns `true` if this rendition uses a fixed coordinate system per page.
    ///
    /// Fixed-layout EPUBs (comics, children's books, technical manuals with precise
    /// typography) cannot be reflowed and require a different rendering path.
    pub fn is_fixed_layout(&self) -> bool {
        self.layout.as_deref() == Some("pre-paginated")
    }
}
