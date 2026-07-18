//! EPUB generator module using Builder pattern.
//!
//! Layout:
//! - [`mod`] (this file) — `EpubBuilder` API, validation, theme / nav orchestration
//! - [`package`] — ZIP assembly (mimetype-first, compression policy)
//! - [`nav`] — `nav.xhtml` / `toc.ncx` serialization
//! - [`opf`] — `content.opf` serialization
use crate::error::EpubError;
use crate::model::{EpubVersion, Metadata, SpineItem, TocEntry};
use std::io::{Read, Seek, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

/// Represents the content of a resource, which can be either fully in-memory or a readable stream.
pub enum ResourceContent {
    Bytes(Vec<u8>),
    Stream(Box<dyn Read + Send + Sync>),
}

// Path helpers live in `crate::path` so generator, parser, and processor share one policy.
use crate::path::epub_relative_path;

mod nav;
mod opf;
mod package;

// ── Built-in themes ──────────────────────────────────────────────────────────

/// Built-in themes for quick, elegant publishing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    /// No CSS injected automatically.
    #[default]
    None,
    /// A clean, modern typography stylesheet suitable for novels and articles,
    /// with built-in dark mode support (media queries).
    Modern,
}

/// Represents a structural landmark (e.g. cover, titlepage, toc, bodymatter).
#[derive(Clone, Debug)]
pub struct Landmark {
    pub epub_type: String, // e.g. "cover", "toc", "bodymatter"
    pub title: String,
    pub href: String,
}

/// Represents a physical page mapping for the EPUB.
#[derive(Clone, Debug)]
pub struct PageListEntry {
    pub name: String, // e.g. "IV", "1", "2"
    pub href: String,
}

/// Represents a file to be added to the EPUB archive.
pub struct Resource {
    id: String,
    href: String,
    media_type: String,
    content: ResourceContent,
    properties: Vec<String>,
}

/// A Builder for creating EPUB files.
pub struct EpubBuilder {
    pub version: EpubVersion,
    pub metadata: Metadata,
    pub theme: Theme,
    pub resources: Vec<Resource>,
    pub spine: Vec<SpineItem>,
    pub toc: Vec<TocEntry>,
    pub landmarks: Vec<Landmark>,
    pub page_list: Vec<PageListEntry>,
    pub cover_id: Option<String>,
}

impl Default for EpubBuilder {
    fn default() -> Self {
        Self::new()
    }
}

const MODERN_THEME_CSS: &str = r#"
/* Modern EPUB-RS Default Theme */
body {
    font-family: "Palatino Linotype", "Book Antiqua", Palatino, serif;
    font-size: 1em;
    line-height: 1.6;
    margin: 5% 5%;
    text-align: justify;
    color: #333;
    background-color: #fff;
}
h1, h2, h3, h4, h5, h6 {
    font-family: "Helvetica Neue", Helvetica, Arial, sans-serif;
    color: #111;
    margin-top: 1.5em;
    margin-bottom: 0.5em;
    text-align: center;
}
p {
    margin: 0 0 1em 0;
    text-indent: 1.5em;
}
blockquote {
    font-style: italic;
    margin: 1em 2em;
    padding-left: 1em;
    border-left: 2px solid #ccc;
}
@media (prefers-color-scheme: dark) {
    body {
        color: #ddd;
        background-color: #121212;
    }
    h1, h2, h3, h4, h5, h6 { color: #fff; }
    blockquote { border-left-color: #444; }
}
"#;

impl EpubBuilder {
    /// Create a new empty EPUB builder
    pub fn new() -> Self {
        Self {
            version: EpubVersion::default(),
            metadata: Metadata::default(),
            theme: Theme::default(),
            resources: Vec::new(),
            spine: Vec::new(),
            toc: Vec::new(),
            landmarks: Vec::new(),
            page_list: Vec::new(),
            cover_id: None,
        }
    }

    /// Set the EPUB version target
    pub fn version(mut self, version: EpubVersion) -> Self {
        self.version = version;
        self
    }

    /// Check if the current EPUB setup is compliant and contains no broken links.
    /// Returns a consolidated list of errors if validation fails.
    pub fn validate(&self) -> Result<(), EpubError> {
        let mut errors = Vec::new();

        // 1. Mandatory Metadata
        if self
            .metadata
            .title
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            errors.push("Missing mandatory metadata: <dc:title>".to_string());
        }
        if self
            .metadata
            .identifier
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            errors.push("Missing mandatory metadata: <dc:identifier>".to_string());
        }
        if self
            .metadata
            .language
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            errors.push("Missing mandatory metadata: <dc:language>".to_string());
        }

        // 2. Resource Lookups
        use std::collections::HashSet;
        let resource_ids: HashSet<&str> = self.resources.iter().map(|r| r.id.as_str()).collect();
        let resource_hrefs: HashSet<&str> =
            self.resources.iter().map(|r| r.href.as_str()).collect();

        // 3. Spine Connectivity
        if self.spine.is_empty() {
            errors.push("The spine (reading order) is completely empty.".to_string());
        }
        for item in &self.spine {
            if !resource_ids.contains(item.idref.as_str()) {
                errors.push(format!(
                    "Spine item idref '{}' does not exist in resources.",
                    item.idref
                ));
            }
        }

        // 4. TOC Nav Links
        fn validate_toc(
            entries: &[TocEntry],
            valid_hrefs: &HashSet<&str>,
            errors: &mut Vec<String>,
        ) {
            for entry in entries {
                let base_href = entry.href.split('#').next().unwrap_or(&entry.href);
                if !valid_hrefs.contains(base_href) {
                    errors.push(format!(
                        "TOC entry '{}' points to missing file: {}",
                        entry.title, base_href
                    ));
                }
                validate_toc(&entry.children, valid_hrefs, errors);
            }
        }
        validate_toc(&self.toc, &resource_hrefs, &mut errors);

        // 5. Cover verification
        if let Some(ref cover_id) = self.cover_id
            && !resource_ids.contains(cover_id.as_str())
        {
            errors.push(format!(
                "Cover image id '{}' is missing in resources.",
                cover_id
            ));
        }

        if !errors.is_empty() {
            return Err(EpubError::ValidationFailed(errors));
        }

        Ok(())
    }

    /// Set the global built-in theme. If set to anything other than `None`,
    /// it will automatically inject a CSS file and link it into all added HTML chapters.
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }

    /// Add a landmark (structural reference like `cover`, `toc`, `bodymatter`).
    pub fn add_landmark(
        mut self,
        epub_type: impl Into<String>,
        href: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        self.landmarks.push(Landmark {
            epub_type: epub_type.into(),
            href: href.into(),
            title: title.into(),
        });
        self
    }

    /// Add a physical page mapping entry (for academic/textbook parity).
    pub fn add_page(mut self, name: impl Into<String>, href: impl Into<String>) -> Self {
        self.page_list.push(PageListEntry {
            name: name.into(),
            href: href.into(),
        });
        self
    }

    /// Set the EPUB metadata
    pub fn metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Set a custom Table of Contents structure (supporting nested chapters).
    pub fn set_toc(mut self, toc: Vec<TocEntry>) -> Self {
        self.toc = toc;
        self
    }

    /// Add a cover image. This automatically creates a resource with `properties="cover-image"`
    /// and configures the EPUB 2 compatible `<meta name="cover" ... />`.
    pub fn set_cover(
        mut self,
        href: impl Into<String>,
        media_type: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> Self {
        let id = "cover-image".to_string();
        self.resources.push(Resource {
            id: id.clone(),
            href: href.into(),
            media_type: media_type.into(),
            content: ResourceContent::Bytes(content.into()),
            properties: vec!["cover-image".to_string()],
        });
        self.cover_id = Some(id);
        self
    }

    /// Add a resource (HTML, image, CSS, etc.) to the EPUB manifest from memory.
    /// This does not add it to the reading order (spine).
    pub fn add_resource(
        mut self,
        id: impl Into<String>,
        href: impl Into<String>,
        media_type: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> Self {
        self.resources.push(Resource {
            id: id.into(),
            href: href.into(),
            media_type: media_type.into(),
            content: ResourceContent::Bytes(content.into()),
            properties: Vec::new(),
        });
        self
    }

    /// Add a resource with specific EPUB 3 properties (e.g. "scripted", "mathml", "svg", "nav").
    pub fn add_resource_with_properties(
        mut self,
        id: impl Into<String>,
        href: impl Into<String>,
        media_type: impl Into<String>,
        content: impl Into<Vec<u8>>,
        properties: impl Into<String>,
    ) -> Self {
        self.resources.push(Resource {
            id: id.into(),
            href: href.into(),
            media_type: media_type.into(),
            content: ResourceContent::Bytes(content.into()),
            properties: properties
                .into()
                .split_whitespace()
                .map(|s| s.to_string())
                .collect(),
        });
        self
    }

    /// Add a large resource (like a high-res video or image) via a readable stream.
    /// The reader will be consumed and copied directly into the ZIP archive during generation.
    pub fn add_resource_stream<R: Read + Send + Sync + 'static>(
        mut self,
        id: impl Into<String>,
        href: impl Into<String>,
        media_type: impl Into<String>,
        reader: R,
    ) -> Self {
        self.resources.push(Resource {
            id: id.into(),
            href: href.into(),
            media_type: media_type.into(),
            content: ResourceContent::Stream(Box::new(reader)),
            properties: Vec::new(),
        });
        self
    }

    /// Add a chapter (HTML/XHTML) to the EPUB manifest AND append it to the spine (reading order).
    pub fn add_chapter(
        mut self,
        id: impl Into<String>,
        href: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> Self {
        let id_str = id.into();
        let content_bytes = content.into();
        let properties = Self::infer_properties(&content_bytes);

        self.spine.push(SpineItem::new(id_str.clone()));
        self.resources.push(Resource {
            id: id_str,
            href: href.into(),
            media_type: "application/xhtml+xml".to_string(),
            content: ResourceContent::Bytes(content_bytes),
            properties,
        });
        self
    }

    /// Add a supplementary chapter (like an answer key or footnote page).
    /// It is added to the spine with `linear="no"`, meaning standard reading flow skips it.
    pub fn add_supplementary_chapter(
        mut self,
        id: impl Into<String>,
        href: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> Self {
        let id_str = id.into();
        self.spine.push(SpineItem {
            idref: id_str.clone(),
            linear: false,
            layout_override: None,
            page_spread: None,
        });
        self.resources.push(Resource {
            id: id_str,
            href: href.into(),
            media_type: "application/xhtml+xml".to_string(),
            content: ResourceContent::Bytes(content.into()),
            properties: Vec::new(),
        });
        self
    }

    /// Add a chapter via a readable stream.
    pub fn add_chapter_stream<R: Read + Send + Sync + 'static>(
        mut self,
        id: impl Into<String>,
        href: impl Into<String>,
        reader: R,
    ) -> Self {
        let id_str = id.into();
        self.spine.push(SpineItem::new(id_str.clone()));
        self.resources.push(Resource {
            id: id_str,
            href: href.into(),
            media_type: "application/xhtml+xml".to_string(),
            content: ResourceContent::Stream(Box::new(reader)),
            properties: Vec::new(),
        });
        self
    }

    /// Add a chapter from an HTML file. Automatically discovers `<img src="...">` and
    /// `<link href="style.css">` tags, loads those files from the local disk, adds them to
    /// the EPUB manifest, and rewrites the HTML to point to the new internal EPUB paths.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn add_chapter_from_html_file<P: AsRef<Path>>(
        mut self,
        id: impl Into<String>,
        file_path: P,
    ) -> Result<Self, EpubError> {
        // Capture id early so we can use it for both spine and resource registration.
        let id_str: String = id.into();

        let path = file_path.as_ref();
        let base_dir = path.parent().unwrap_or_else(|| Path::new(""));

        let mut html_content = std::fs::read(path).map_err(EpubError::Io)?;
        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        let chapter_href = format!("text/{}", filename);

        // Use lol_html to find assets and rewrite links
        use std::cell::RefCell;
        use std::rc::Rc;
        let assets_to_add = Rc::new(RefCell::new(Vec::new()));

        let assets_img = Rc::clone(&assets_to_add);
        let assets_link = Rc::clone(&assets_to_add);

        // A mapper that records local paths and rewrites to OEBPS internal paths
        let rewritten_html = crate::processor::rewrite_links(&html_content, move |tag, url| {
            // Ignore absolute URLs (http://, https://, data:)
            if url.starts_with("http") || url.starts_with("data:") {
                return None;
            }

            // Clean URL from anchors or query strings (e.g. img.jpg?v=1)
            let clean_url = url
                .split('?')
                .next()
                .unwrap_or(url)
                .split('#')
                .next()
                .unwrap_or(url);

            if tag == "img" {
                let internal_path = format!("images/{}", clean_url);
                assets_img.borrow_mut().push((
                    clean_url.to_string(),
                    internal_path.clone(),
                    "image/jpeg".to_string(),
                )); // Simplified mime
                return Some(format!("../{}", internal_path)); // relative to text/ folder
            } else if tag == "link" {
                let internal_path = format!("styles/{}", clean_url);
                assets_link.borrow_mut().push((
                    clean_url.to_string(),
                    internal_path.clone(),
                    "text/css".to_string(),
                ));
                return Some(format!("../{}", internal_path));
            }
            None
        })?;

        html_content = rewritten_html;

        // Consume the assets to add
        let assets = Rc::try_unwrap(assets_to_add).unwrap().into_inner();
        for (local_path, internal_path, mime) in assets {
            // Read from local disk relative to the HTML file
            let absolute_path = base_dir.join(&local_path);
            if let Ok(bytes) = std::fs::read(absolute_path) {
                // Generate a safe ID for the asset
                let asset_id = local_path.replace(['/', '.', '\\'], "_");

                // Add asset to builder
                self.resources.push(Resource {
                    id: asset_id,
                    href: internal_path,
                    media_type: mime,
                    content: ResourceContent::Bytes(bytes),
                    properties: Vec::new(),
                });
            }
        }

        // Add the rewritten chapter using the caller-provided id for both spine and manifest.
        self.spine.push(SpineItem::new(id_str.clone()));
        self.resources.push(Resource {
            id: id_str,
            href: chapter_href,
            media_type: "application/xhtml+xml".to_string(),
            content: ResourceContent::Bytes(html_content),
            properties: Vec::new(),
        });

        Ok(self)
    }
    /// Add a chapter with a specific layout or page spread behavior (ideal for comics or picture books).
    pub fn add_chapter_with_layout(
        mut self,
        id: impl Into<String>,
        href: impl Into<String>,
        content: impl Into<Vec<u8>>,
        layout: Option<crate::model::LayoutType>,
        spread: Option<crate::model::PageSpread>,
    ) -> Self {
        let id_str = id.into();
        let content_bytes = content.into();
        let properties = Self::infer_properties(&content_bytes);

        let mut spine_item = SpineItem::new(id_str.clone());
        spine_item.layout_override = layout;
        spine_item.page_spread = spread;

        self.spine.push(spine_item);
        self.resources.push(Resource {
            id: id_str,
            href: href.into(),
            media_type: "application/xhtml+xml".to_string(),
            content: ResourceContent::Bytes(content_bytes),
            properties,
        });
        self
    }

    /// Add a chapter and also add it to the Table of Contents (TOC).
    /// Note: If you want a nested TOC, use `set_toc()` instead.
    pub fn add_chapter_with_nav(
        mut self,
        id: impl Into<String>,
        href: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> Self {
        let id_str = id.into();
        let href_str = href.into();
        let title_str = title.into();
        let content_bytes = content.into();
        let properties = Self::infer_properties(&content_bytes);

        self.toc.push(TocEntry::new(title_str, href_str.clone()));

        self.spine.push(SpineItem::new(id_str.clone()));
        self.resources.push(Resource {
            id: id_str,
            href: href_str,
            media_type: "application/xhtml+xml".to_string(),
            content: ResourceContent::Bytes(content_bytes),
            properties,
        });
        self
    }

    /// Build the EPUB and write it to the provided writer (e.g., `std::fs::File` or `Vec<u8>`).
    pub fn generate<W: Write + Seek>(mut self, writer: W) -> Result<(), EpubError> {
        self.validate()?;
        self.preprocess_resources()?;

        let opf_content = self.generate_opf(!self.toc.is_empty())?;
        let resources = self
            .resources
            .into_iter()
            .map(|res| (res.href, res.content));
        package::write_epub_zip(writer, resources, &opf_content)
    }

    /// Preprocess resources: sets up dynamic stylesheet, generates navigation,
    /// and injects theme link tags into HTML files.
    fn preprocess_resources(&mut self) -> Result<(), EpubError> {
        let theme_href = self.inject_theme_stylesheet();
        self.generate_navigation_resources()?;
        if let Some(href) = theme_href {
            self.inject_html_theme_links(&href)?;
        }
        Ok(())
    }

    /// Step 1: Inject theme stylesheet resource if a theme is active.
    fn inject_theme_stylesheet(&mut self) -> Option<String> {
        if self.theme == Theme::Modern {
            let href = "styles/epub-rs-modern.css".to_string();
            self.resources.push(Resource {
                id: "epub-rs-theme-modern".to_string(),
                href: href.clone(),
                media_type: "text/css".to_string(),
                content: ResourceContent::Bytes(MODERN_THEME_CSS.as_bytes().to_vec()),
                properties: Vec::new(),
            });
            Some(href)
        } else {
            None
        }
    }

    /// Step 2: Auto-generate Navigation documents (nav.xhtml, toc.ncx) and push them to resources.
    fn generate_navigation_resources(&mut self) -> Result<(), EpubError> {
        if self.toc.is_empty() {
            return Ok(());
        }

        // EPUB 3 requires nav.xhtml
        if self.version == EpubVersion::V30 {
            let nav_html = self.generate_nav_xhtml()?;
            self.resources.push(Resource {
                id: "nav".to_string(),
                href: "nav.xhtml".to_string(),
                media_type: "application/xhtml+xml".to_string(),
                content: ResourceContent::Bytes(nav_html.into_bytes()),
                properties: vec!["nav".to_string()],
            });
        }

        // Fallback NCX for EPUB 2 or backwards compatibility in EPUB 3
        let has_ncx = self.version == EpubVersion::V20 || self.version == EpubVersion::V30;
        if has_ncx {
            let ncx_xml = self.generate_ncx()?;
            self.resources.push(Resource {
                id: "ncx".to_string(),
                href: "toc.ncx".to_string(),
                media_type: "application/x-dtbncx+xml".to_string(),
                content: ResourceContent::Bytes(ncx_xml.into_bytes()),
                properties: Vec::new(),
            });
        }

        Ok(())
    }

    /// Step 3: Inject link tags into HTML files referencing the theme CSS.
    fn inject_html_theme_links(&mut self, css_href: &str) -> Result<(), EpubError> {
        for res in &mut self.resources {
            if res.media_type == "application/xhtml+xml" {
                let is_nav = res.properties.iter().any(|p| p == "nav");
                if !is_nav && let ResourceContent::Bytes(ref mut bytes) = res.content {
                    let relative_css_path = epub_relative_path(&res.href, css_href);
                    let link_tag = format!(
                        "<link rel=\"stylesheet\" type=\"text/css\" href=\"{}\" />\n",
                        relative_css_path
                    );
                    let mut new_html = Vec::new();
                    if crate::processor::inject_head_content(&bytes[..], &mut new_html, &link_tag)
                        .is_ok()
                        && !new_html.is_empty()
                    {
                        *bytes = new_html;
                    }
                }
            }
        }
        Ok(())
    }

    /// Helper to infer EPUB 3 properties (scripted, mathml, svg) from HTML content.
    fn infer_properties(content: &[u8]) -> Vec<String> {
        let mut has_script = false;
        let mut has_svg = false;
        let mut has_math = false;

        // Zero-allocation, single-pass byte-level heuristic search
        for i in 0..content.len() {
            if content[i] == b'<' {
                let remain = content.len() - i;

                if !has_script && remain >= 7 && content[i..i + 7].eq_ignore_ascii_case(b"<script")
                {
                    has_script = true;
                } else if !has_svg && remain >= 4 && content[i..i + 4].eq_ignore_ascii_case(b"<svg")
                {
                    has_svg = true;
                } else if !has_math
                    && remain >= 5
                    && content[i..i + 5].eq_ignore_ascii_case(b"<math")
                {
                    has_math = true;
                }

                // Early exit if all properties are found
                if has_script && has_svg && has_math {
                    break;
                }
            }
        }

        let mut props = Vec::new();
        if has_script {
            props.push("scripted".to_string());
        }
        if has_svg {
            props.push("svg".to_string());
        }
        if has_math {
            props.push("mathml".to_string());
        }

        props
    }
}

#[cfg(test)]
mod tests {
    // epub_relative_path coverage lives in `crate::path` (shared policy module).

    use super::*;

    #[test]
    fn test_infer_properties() {
        // Pure text/html should yield no properties
        assert_eq!(
            EpubBuilder::infer_properties(b"<html><body><h1>Title</h1></body></html>"),
            Vec::<String>::new()
        );

        // Scripts
        let script = EpubBuilder::infer_properties(
            b"<html><head><script src='test.js'></script></head></html>",
        );
        assert_eq!(script, vec!["scripted".to_string()]);

        // Case insensitivity
        let script_upper = EpubBuilder::infer_properties(b"<SCRIPT>alert(1);</SCRIPT>");
        assert_eq!(script_upper, vec!["scripted".to_string()]);

        // SVG
        let svg = EpubBuilder::infer_properties(b"<p>Graphic: <svg></svg></p>");
        assert_eq!(svg, vec!["svg".to_string()]);

        // MathML
        let math = EpubBuilder::infer_properties(
            b"<math xmlns='http://www.w3.org/1998/Math/MathML'><mi>x</mi></math>",
        );
        assert_eq!(math, vec!["mathml".to_string()]);

        // Multiple properties
        let combo = EpubBuilder::infer_properties(b"<svg></svg><div><script></script></div>");
        assert!(combo.contains(&"scripted".to_string()));
        assert!(combo.contains(&"svg".to_string()));
    }

    #[test]
    fn test_xml_entity_escaping() {
        use crate::model::{EpubVersion, Metadata};
        use std::io::Cursor;

        let metadata = Metadata {
            title: Some("Me & You <3".to_string()),
            identifier: Some("urn:uuid:12345".to_string()),
            language: Some("en".to_string()),
            ..Default::default()
        };

        let builder = EpubBuilder::new()
            .version(EpubVersion::V30)
            .metadata(metadata)
            .add_chapter("chapter1", "text/ch1.xhtml", b"Hello".to_vec());

        let mut buffer = Cursor::new(Vec::new());
        builder
            .generate(&mut buffer)
            .expect("Failed to generate EPUB");

        let data = buffer.into_inner();
        let reader = Cursor::new(data);
        let mut archive = zip::ZipArchive::new(reader).unwrap();
        let mut file = archive.by_name("OEBPS/content.opf").unwrap();
        let mut opf_content = String::new();
        file.read_to_string(&mut opf_content).unwrap();

        // Should be correctly escaped in the raw XML
        assert!(opf_content.contains("<dc:title>Me &amp; You &lt;3</dc:title>"));
    }
}
