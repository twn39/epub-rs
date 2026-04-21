//! EPUB generator module using Builder pattern.
use crate::error::EpubError;
use crate::model::{EpubVersion, Metadata, SpineItem, TocEntry};
use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use std::io::{Read, Seek, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use zip::CompressionMethod;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

/// Represents the content of a resource, which can be either fully in-memory or a readable stream.
pub enum ResourceContent {
    Bytes(Vec<u8>),
    Stream(Box<dyn Read + Send + Sync>),
}

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

        let mut zip = ZipWriter::new(writer);

        let mut theme_href = None;
        if self.theme == Theme::Modern {
            theme_href = Some("styles/epub-rs-modern.css");
            self.resources.push(Resource {
                id: "epub-rs-theme-modern".to_string(),
                href: theme_href.unwrap().to_string(),
                media_type: "text/css".to_string(),
                content: ResourceContent::Bytes(MODERN_THEME_CSS.as_bytes().to_vec()),
                properties: Vec::new(),
            });
        }

        // Auto-generate Navigation documents if we have TOC entries
        let has_toc = !self.toc.is_empty();
        let mut has_ncx = false;

        if has_toc {
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

                // Fallback NCX for backwards compatibility
                has_ncx = true;
            } else if self.version == EpubVersion::V20 {
                has_ncx = true;
            }

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
        }

        // 1. Write `mimetype` (MUST be first, MUST be uncompressed)
        let options_stored =
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("mimetype", options_stored)?;
        zip.write_all(b"application/epub+zip")?;

        // Standard compression for the rest of the files
        let options_deflated =
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        // 2. Write `META-INF/container.xml`
        zip.start_file("META-INF/container.xml", options_deflated)?;
        let container_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
    <rootfiles>
        <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
    </rootfiles>
</container>"#;
        zip.write_all(container_xml.as_bytes())?;

        // Generate OPF content BEFORE consuming self.resources
        let opf_content = self.generate_opf(has_ncx)?;

        // 3. Write resources
        for mut res in self.resources {
            let zip_path = format!("OEBPS/{}", res.href);
            zip.start_file(&zip_path, options_deflated)?;
            match res.content {
                ResourceContent::Bytes(mut bytes) => {
                    // Inject CSS reference if it's an HTML file and theme is enabled
                    if res.media_type == "application/xhtml+xml"
                        && let Some(css_href) = theme_href
                    {
                        // Calculate relative path from this HTML file to the styles dir.
                        // Simplified logic: Count slashes in `res.href` to figure out depth
                        let depth = res.href.chars().filter(|&c| c == '/').count();
                        let up_path = "../".repeat(depth);
                        let relative_css_path = format!("{}{}", up_path, css_href);

                        let link_tag = format!(
                            "<link rel=\"stylesheet\" type=\"text/css\" href=\"{}\" />\n",
                            relative_css_path
                        );
                        let mut new_html = Vec::new();
                        if crate::processor::inject_head_content(
                            &bytes[..],
                            &mut new_html,
                            &link_tag,
                        )
                        .is_ok()
                            && !new_html.is_empty()
                        {
                            bytes = new_html;
                        }
                    }
                    zip.write_all(&bytes)?;
                }
                ResourceContent::Stream(ref mut stream) => {
                    // Note: We don't auto-inject themes into streamed HTML to save memory.
                    // If a user streams HTML, they are responsible for their own themes.
                    std::io::copy(stream, &mut zip)?;
                }
            }
        }

        // 4. Write `OEBPS/content.opf`
        zip.start_file("OEBPS/content.opf", options_deflated)?;
        zip.write_all(&opf_content)?;

        zip.finish()?;
        Ok(())
    }

    /// Generate EPUB 3 `nav.xhtml`
    fn generate_nav_xhtml(&self) -> Result<String, EpubError> {
        let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);

        // <?xml version="1.0" encoding="utf-8"?>
        writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)))?;

        // <!DOCTYPE html>
        writer
            .get_mut()
            .write_all(
                b"
<!DOCTYPE html>
",
            )
            .map_err(EpubError::Io)?;

        let mut html = BytesStart::new("html");
        html.push_attribute(("xmlns", "http://www.w3.org/1999/xhtml"));
        html.push_attribute(("xmlns:epub", "http://www.idpf.org/2007/ops"));
        writer.write_event(Event::Start(html))?;

        writer.write_event(Event::Start(BytesStart::new("head")))?;
        writer.write_event(Event::Start(BytesStart::new("title")))?;
        writer.write_event(Event::Text(BytesText::new("Navigation")))?;
        writer.write_event(Event::End(BytesEnd::new("title")))?;
        writer.write_event(Event::End(BytesEnd::new("head")))?;

        writer.write_event(Event::Start(BytesStart::new("body")))?;

        let mut nav_toc = BytesStart::new("nav");
        nav_toc.push_attribute(("epub:type", "toc"));
        nav_toc.push_attribute(("id", "toc"));
        writer.write_event(Event::Start(nav_toc))?;

        writer.write_event(Event::Start(BytesStart::new("h1")))?;
        writer.write_event(Event::Text(BytesText::new("Table of Contents")))?;
        writer.write_event(Event::End(BytesEnd::new("h1")))?;

        Self::build_nav_list(&self.toc, &mut writer)?;

        writer.write_event(Event::End(BytesEnd::new("nav")))?;

        // Landmarks
        if !self.landmarks.is_empty() {
            let mut nav_landmarks = BytesStart::new("nav");
            nav_landmarks.push_attribute(("epub:type", "landmarks"));
            nav_landmarks.push_attribute(("id", "landmarks"));
            writer.write_event(Event::Start(nav_landmarks))?;

            writer.write_event(Event::Start(BytesStart::new("h2")))?;
            writer.write_event(Event::Text(BytesText::new("Landmarks")))?;
            writer.write_event(Event::End(BytesEnd::new("h2")))?;

            writer.write_event(Event::Start(BytesStart::new("ol")))?;
            for landmark in &self.landmarks {
                writer.write_event(Event::Start(BytesStart::new("li")))?;
                let mut a = BytesStart::new("a");
                a.push_attribute(("epub:type", landmark.epub_type.as_str()));
                a.push_attribute(("href", landmark.href.as_str()));
                writer.write_event(Event::Start(a))?;
                writer.write_event(Event::Text(BytesText::new(&landmark.title)))?;
                writer.write_event(Event::End(BytesEnd::new("a")))?;
                writer.write_event(Event::End(BytesEnd::new("li")))?;
            }
            writer.write_event(Event::End(BytesEnd::new("ol")))?;
            writer.write_event(Event::End(BytesEnd::new("nav")))?;
        }

        // Page List
        if !self.page_list.is_empty() {
            let mut nav_pages = BytesStart::new("nav");
            nav_pages.push_attribute(("epub:type", "page-list"));
            nav_pages.push_attribute(("id", "page-list"));
            writer.write_event(Event::Start(nav_pages))?;

            writer.write_event(Event::Start(BytesStart::new("h2")))?;
            writer.write_event(Event::Text(BytesText::new("Page List")))?;
            writer.write_event(Event::End(BytesEnd::new("h2")))?;

            writer.write_event(Event::Start(BytesStart::new("ol")))?;
            for page in &self.page_list {
                writer.write_event(Event::Start(BytesStart::new("li")))?;
                let mut a = BytesStart::new("a");
                a.push_attribute(("href", page.href.as_str()));
                writer.write_event(Event::Start(a))?;
                writer.write_event(Event::Text(BytesText::new(&page.name)))?;
                writer.write_event(Event::End(BytesEnd::new("a")))?;
                writer.write_event(Event::End(BytesEnd::new("li")))?;
            }
            writer.write_event(Event::End(BytesEnd::new("ol")))?;
            writer.write_event(Event::End(BytesEnd::new("nav")))?;
        }

        writer.write_event(Event::End(BytesEnd::new("body")))?;
        writer.write_event(Event::End(BytesEnd::new("html")))?;

        let result = String::from_utf8(writer.into_inner())
            .map_err(|e| EpubError::InvalidFormat(e.to_string()))?;
        Ok(result)
    }

    fn build_nav_list(entries: &[TocEntry], writer: &mut Writer<Vec<u8>>) -> Result<(), EpubError> {
        if entries.is_empty() {
            return Ok(());
        }
        writer.write_event(Event::Start(BytesStart::new("ol")))?;

        for entry in entries {
            writer.write_event(Event::Start(BytesStart::new("li")))?;
            let mut a = BytesStart::new("a");
            a.push_attribute(("href", entry.href.as_str()));
            writer.write_event(Event::Start(a))?;
            writer.write_event(Event::Text(BytesText::new(&entry.title)))?;
            writer.write_event(Event::End(BytesEnd::new("a")))?;

            if !entry.children.is_empty() {
                Self::build_nav_list(&entry.children, writer)?;
            }
            writer.write_event(Event::End(BytesEnd::new("li")))?;
        }

        writer.write_event(Event::End(BytesEnd::new("ol")))?;
        Ok(())
    }

    /// Generate EPUB 2 compatible `toc.ncx`
    fn generate_ncx(&self) -> Result<String, EpubError> {
        let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);

        let title = self.metadata.title.as_deref().unwrap_or("Untitled");
        let max_page = self.page_list.len().to_string();
        let uid = self
            .metadata
            .identifier
            .as_deref()
            .unwrap_or("urn:uuid:epub-rs-default");

        writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;

        let mut ncx = BytesStart::new("ncx");
        ncx.push_attribute(("xmlns", "http://www.daisy.org/z3986/2005/ncx/"));
        ncx.push_attribute(("version", "2005-1"));
        writer.write_event(Event::Start(ncx))?;

        writer.write_event(Event::Start(BytesStart::new("head")))?;

        let mut meta_uid = BytesStart::new("meta");
        meta_uid.push_attribute(("name", "dtb:uid"));
        meta_uid.push_attribute(("content", uid));
        writer.write_event(Event::Empty(meta_uid))?;

        let mut meta_depth = BytesStart::new("meta");
        meta_depth.push_attribute(("name", "dtb:depth"));
        meta_depth.push_attribute(("content", "1")); // 1 or dynamic depth
        writer.write_event(Event::Empty(meta_depth))?;

        let mut meta_total = BytesStart::new("meta");
        meta_total.push_attribute(("name", "dtb:totalPageCount"));
        meta_total.push_attribute(("content", max_page.as_str()));
        writer.write_event(Event::Empty(meta_total))?;

        let mut meta_max = BytesStart::new("meta");
        meta_max.push_attribute(("name", "dtb:maxPageNumber"));
        meta_max.push_attribute(("content", max_page.as_str()));
        writer.write_event(Event::Empty(meta_max))?;

        writer.write_event(Event::End(BytesEnd::new("head")))?;

        writer.write_event(Event::Start(BytesStart::new("docTitle")))?;
        writer.write_event(Event::Start(BytesStart::new("text")))?;
        writer.write_event(Event::Text(BytesText::new(title)))?;
        writer.write_event(Event::End(BytesEnd::new("text")))?;
        writer.write_event(Event::End(BytesEnd::new("docTitle")))?;

        let navmap = BytesStart::new("navMap");
        writer.write_event(Event::Start(navmap))?;
        let mut play_order = 0;
        Self::build_ncx_navpoints(&self.toc, &mut writer, &mut play_order)?;
        writer.write_event(Event::End(BytesEnd::new("navMap")))?;

        if !self.page_list.is_empty() {
            writer.write_event(Event::Start(BytesStart::new("pageList")))?;
            writer.write_event(Event::Start(BytesStart::new("navLabel")))?;
            writer.write_event(Event::Start(BytesStart::new("text")))?;
            writer.write_event(Event::Text(BytesText::new("Pages")))?;
            writer.write_event(Event::End(BytesEnd::new("text")))?;
            writer.write_event(Event::End(BytesEnd::new("navLabel")))?;

            for (i, page) in self.page_list.iter().enumerate() {
                play_order += 1;

                let mut target = BytesStart::new("pageTarget");
                target.push_attribute(("id", format!("page-{}", i + 1).as_str()));
                target.push_attribute(("type", "normal"));
                target.push_attribute(("value", page.name.as_str()));
                target.push_attribute(("playOrder", play_order.to_string().as_str()));
                writer.write_event(Event::Start(target))?;

                writer.write_event(Event::Start(BytesStart::new("navLabel")))?;
                writer.write_event(Event::Start(BytesStart::new("text")))?;
                writer.write_event(Event::Text(BytesText::new(&page.name)))?;
                writer.write_event(Event::End(BytesEnd::new("text")))?;
                writer.write_event(Event::End(BytesEnd::new("navLabel")))?;

                let mut content = BytesStart::new("content");
                content.push_attribute(("src", page.href.as_str()));
                writer.write_event(Event::Empty(content))?;

                writer.write_event(Event::End(BytesEnd::new("pageTarget")))?;
            }
            writer.write_event(Event::End(BytesEnd::new("pageList")))?;
        }

        writer.write_event(Event::End(BytesEnd::new("ncx")))?;

        let result = String::from_utf8(writer.into_inner())
            .map_err(|e| EpubError::InvalidFormat(e.to_string()))?;
        Ok(result)
    }

    fn build_ncx_navpoints(
        entries: &[TocEntry],
        writer: &mut Writer<Vec<u8>>,
        play_order: &mut usize,
    ) -> Result<(), EpubError> {
        for entry in entries {
            *play_order += 1;
            let current_order = play_order.to_string();

            let mut nav_point = BytesStart::new("navPoint");
            nav_point.push_attribute(("id", format!("navPoint-{}", current_order).as_str()));
            nav_point.push_attribute(("playOrder", current_order.as_str()));
            writer.write_event(Event::Start(nav_point))?;

            writer.write_event(Event::Start(BytesStart::new("navLabel")))?;
            writer.write_event(Event::Start(BytesStart::new("text")))?;
            writer.write_event(Event::Text(BytesText::new(&entry.title)))?;
            writer.write_event(Event::End(BytesEnd::new("text")))?;
            writer.write_event(Event::End(BytesEnd::new("navLabel")))?;

            let mut content = BytesStart::new("content");
            content.push_attribute(("src", entry.href.as_str()));
            writer.write_event(Event::Empty(content))?;

            if !entry.children.is_empty() {
                Self::build_ncx_navpoints(&entry.children, writer, play_order)?;
            }

            writer.write_event(Event::End(BytesEnd::new("navPoint")))?;
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

    /// Helper to generate the OPF XML content using quick-xml.
    fn generate_opf(&self, has_toc: bool) -> Result<Vec<u8>, EpubError> {
        let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);

        // <?xml version="1.0" encoding="UTF-8"?>
        writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;

        // <package version="..." unique-identifier="pub-id" xmlns="http://www.idpf.org/2007/opf">
        let mut package = BytesStart::new("package");
        let version_str = match self.version {
            EpubVersion::V20 => "2.0",
            EpubVersion::V30 => "3.0",
        };
        package.push_attribute(("version", version_str));
        package.push_attribute(("unique-identifier", "pub-id"));
        package.push_attribute(("xmlns", "http://www.idpf.org/2007/opf"));
        writer.write_event(Event::Start(package.clone()))?;

        // <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
        let mut metadata = BytesStart::new("metadata");
        metadata.push_attribute(("xmlns:dc", "http://purl.org/dc/elements/1.1/"));
        writer.write_event(Event::Start(metadata))?;

        if let Some(title) = &self.metadata.title {
            Self::write_text_element(&mut writer, "dc:title", title)?;
        }
        // Output creators with optional refinements
        for (i, creator) in self.metadata.creators.iter().enumerate() {
            let id = format!("creator_{}", i);
            let mut c_start = BytesStart::new("dc:creator");
            c_start.push_attribute(("id", id.as_str()));
            writer.write_event(Event::Start(c_start))?;
            writer.write_event(Event::Text(BytesText::new(&creator.name)))?;
            writer.write_event(Event::End(BytesEnd::new("dc:creator")))?;

            if let Some(role) = &creator.role {
                let mut m_start = BytesStart::new("meta");
                m_start.push_attribute(("refines", format!("#{}", id).as_str()));
                m_start.push_attribute(("property", "role"));
                m_start.push_attribute(("scheme", "marc:relators"));
                writer.write_event(Event::Start(m_start))?;
                writer.write_event(Event::Text(BytesText::new(role)))?;
                writer.write_event(Event::End(BytesEnd::new("meta")))?;
            }
            if let Some(file_as) = &creator.file_as {
                let mut m_start = BytesStart::new("meta");
                m_start.push_attribute(("refines", format!("#{}", id).as_str()));
                m_start.push_attribute(("property", "file-as"));
                writer.write_event(Event::Start(m_start))?;
                writer.write_event(Event::Text(BytesText::new(file_as)))?;
                writer.write_event(Event::End(BytesEnd::new("meta")))?;
            }
        }
        if let Some(lang) = &self.metadata.language {
            Self::write_text_element(&mut writer, "dc:language", lang)?;
        } else {
            Self::write_text_element(&mut writer, "dc:language", "en")?; // Fallback
        }
        if let Some(id) = &self.metadata.identifier {
            let mut id_start = BytesStart::new("dc:identifier");
            id_start.push_attribute(("id", "pub-id"));
            writer.write_event(Event::Start(id_start))?;
            writer.write_event(Event::Text(BytesText::new(id)))?;
            writer.write_event(Event::End(BytesEnd::new("dc:identifier")))?;
        } else {
            let mut id_start = BytesStart::new("dc:identifier");
            id_start.push_attribute(("id", "pub-id"));
            writer.write_event(Event::Start(id_start))?;
            writer.write_event(Event::Text(BytesText::new("urn:uuid:default-epub-rs-id")))?;
            writer.write_event(Event::End(BytesEnd::new("dc:identifier")))?;
        }

        if let Some(publisher) = &self.metadata.publisher {
            Self::write_text_element(&mut writer, "dc:publisher", publisher)?;
        }
        if let Some(description) = &self.metadata.description {
            Self::write_text_element(&mut writer, "dc:description", description)?;
        }
        if let Some(date) = &self.metadata.date {
            Self::write_text_element(&mut writer, "dc:date", date)?;
        }
        if let Some(rights) = &self.metadata.rights {
            Self::write_text_element(&mut writer, "dc:rights", rights)?;
        }
        for subject in &self.metadata.subjects {
            Self::write_text_element(&mut writer, "dc:subject", subject)?;
        }

        if self.version == EpubVersion::V30 {
            let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
            let mut m_start = BytesStart::new("meta");
            m_start.push_attribute(("property", "dcterms:modified"));
            writer.write_event(Event::Start(m_start))?;
            writer.write_event(Event::Text(BytesText::new(&now)))?;
            writer.write_event(Event::End(BytesEnd::new("meta")))?;

            if self.metadata.layout == crate::model::LayoutType::PrePaginated {
                let mut m_layout = BytesStart::new("meta");
                m_layout.push_attribute(("property", "rendition:layout"));
                writer.write_event(Event::Start(m_layout))?;
                writer.write_event(Event::Text(BytesText::new("pre-paginated")))?;
                writer.write_event(Event::End(BytesEnd::new("meta")))?;
            }
        }

        // EPUB 2 Cover Meta
        if let Some(cover_id) = &self.cover_id {
            let mut meta_cover = BytesStart::new("meta");
            meta_cover.push_attribute(("name", "cover"));
            meta_cover.push_attribute(("content", cover_id.as_str()));
            writer.write_event(Event::Empty(meta_cover))?;
        }

        writer.write_event(Event::End(BytesEnd::new("metadata")))?;

        // <manifest>
        writer.write_event(Event::Start(BytesStart::new("manifest")))?;
        for res in &self.resources {
            let mut item = BytesStart::new("item");
            item.push_attribute(("id", res.id.as_str()));
            item.push_attribute(("href", res.href.as_str()));
            item.push_attribute(("media-type", res.media_type.as_str()));
            if !res.properties.is_empty() {
                let prop_str = res.properties.join(" ");
                item.push_attribute(("properties", prop_str.as_str()));
            }
            writer.write_event(Event::Empty(item))?;
        }
        writer.write_event(Event::End(BytesEnd::new("manifest")))?;

        // <spine toc="ncx">
        let mut spine = BytesStart::new("spine");
        if has_toc {
            spine.push_attribute(("toc", "ncx"));
        }
        writer.write_event(Event::Start(spine))?;
        for item in &self.spine {
            let mut itemref = BytesStart::new("itemref");
            itemref.push_attribute(("idref", item.idref.as_str()));
            if !item.linear {
                itemref.push_attribute(("linear", "no"));
            }
            if self.version == EpubVersion::V30 {
                let mut properties = Vec::new();
                if item.layout_override == Some(crate::model::LayoutType::Reflowable) {
                    properties.push("rendition:layout-reflowable");
                } else if item.layout_override == Some(crate::model::LayoutType::PrePaginated) {
                    properties.push("rendition:layout-pre-paginated");
                }

                if let Some(spread) = item.page_spread {
                    match spread {
                        crate::model::PageSpread::Left => properties.push("page-spread-left"),
                        crate::model::PageSpread::Right => properties.push("page-spread-right"),
                        crate::model::PageSpread::Center => {
                            properties.push("rendition:page-spread-center")
                        }
                        crate::model::PageSpread::None => (),
                    }
                }

                if !properties.is_empty() {
                    let prop_str = properties.join(" ");
                    itemref.push_attribute(("properties", prop_str.as_str()));
                }
            }
            writer.write_event(Event::Empty(itemref))?;
        }
        writer.write_event(Event::End(BytesEnd::new("spine")))?;

        // <guide> for EPUB 2 fallback landmarks
        if !self.landmarks.is_empty() {
            writer.write_event(Event::Start(BytesStart::new("guide")))?;
            for landmark in &self.landmarks {
                let mut reference = BytesStart::new("reference");
                reference.push_attribute(("type", landmark.epub_type.as_str()));
                reference.push_attribute(("title", landmark.title.as_str()));
                reference.push_attribute(("href", landmark.href.as_str()));
                writer.write_event(Event::Empty(reference))?;
            }
            writer.write_event(Event::End(BytesEnd::new("guide")))?;
        }

        // </package>
        writer.write_event(Event::End(BytesEnd::new("package")))?;

        Ok(writer.into_inner())
    }

    fn write_text_element(
        writer: &mut Writer<Vec<u8>>,
        tag: &str,
        text: &str,
    ) -> Result<(), EpubError> {
        writer.write_event(Event::Start(BytesStart::new(tag)))?;
        writer.write_event(Event::Text(BytesText::new(text)))?;
        writer.write_event(Event::End(BytesEnd::new(tag)))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
