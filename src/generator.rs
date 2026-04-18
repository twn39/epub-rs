//! EPUB generator module using Builder pattern.
use crate::error::EpubError;
use crate::model::{EpubVersion, Metadata, SpineItem, TocEntry};
use quick_xml::Writer;
use quick_xml::escape::escape;
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
struct Resource {
    id: String,
    href: String,
    media_type: String,
    content: ResourceContent,
    properties: Option<String>,
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
            properties: Some("cover-image".to_string()),
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
            properties: None,
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
            properties: Some(properties.into()),
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
            properties: None,
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
            properties: None,
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
            properties: None,
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
                    properties: None,
                });
            }
        }

        // Add the rewritten chapter
        self.spine.push(SpineItem::new(id.into()));
        self.resources.push(Resource {
            id: filename.to_string(),
            href: chapter_href,
            media_type: "application/xhtml+xml".to_string(),
            content: ResourceContent::Bytes(html_content),
            properties: None,
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
        let mut zip = ZipWriter::new(writer);

        let mut theme_href = None;
        if self.theme == Theme::Modern {
            theme_href = Some("styles/epub-rs-modern.css");
            self.resources.push(Resource {
                id: "epub-rs-theme-modern".to_string(),
                href: theme_href.unwrap().to_string(),
                media_type: "text/css".to_string(),
                content: ResourceContent::Bytes(MODERN_THEME_CSS.as_bytes().to_vec()),
                properties: None,
            });
        }

        // Auto-generate Navigation documents if we have TOC entries
        let has_toc = !self.toc.is_empty();
        let mut has_ncx = false;

        if has_toc {
            // EPUB 3 requires nav.xhtml
            if self.version == EpubVersion::V30 {
                let nav_html = self.generate_nav_xhtml();
                self.resources.push(Resource {
                    id: "nav".to_string(),
                    href: "nav.xhtml".to_string(),
                    media_type: "application/xhtml+xml".to_string(),
                    content: ResourceContent::Bytes(nav_html.into_bytes()),
                    properties: Some("nav".to_string()),
                });

                // Fallback NCX for backwards compatibility
                has_ncx = true;
            } else if self.version == EpubVersion::V20 {
                has_ncx = true;
            }

            if has_ncx {
                let ncx_xml = self.generate_ncx();
                self.resources.push(Resource {
                    id: "ncx".to_string(),
                    href: "toc.ncx".to_string(),
                    media_type: "application/x-dtbncx+xml".to_string(),
                    content: ResourceContent::Bytes(ncx_xml.into_bytes()),
                    properties: None,
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
    fn generate_nav_xhtml(&self) -> String {
        let mut html = String::from(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n<head><title>Navigation</title></head>\n<body>\n<nav epub:type=\"toc\" id=\"toc\">\n<h1>Table of Contents</h1>\n",
        );
        Self::build_nav_list(&self.toc, &mut html);
        html.push_str("</nav>\n");

        if !self.landmarks.is_empty() {
            html.push_str(
                "<nav epub:type=\"landmarks\" id=\"landmarks\">\n<h2>Landmarks</h2>\n<ol>\n",
            );
            for landmark in &self.landmarks {
                html.push_str(&format!(
                    "  <li><a epub:type=\"{}\" href=\"{}\">{}</a></li>\n",
                    escape(&landmark.epub_type),
                    escape(&landmark.href),
                    escape(&landmark.title)
                ));
            }
            html.push_str("</ol>\n</nav>\n");
        }

        if !self.page_list.is_empty() {
            html.push_str(
                "<nav epub:type=\"page-list\" id=\"page-list\">\n<h2>Page List</h2>\n<ol>\n",
            );
            for page in &self.page_list {
                html.push_str(&format!(
                    "  <li><a href=\"{}\">{}</a></li>\n",
                    escape(&page.href),
                    escape(&page.name)
                ));
            }
            html.push_str("</ol>\n</nav>\n");
        }

        html.push_str("</body>\n</html>");
        html
    }

    fn build_nav_list(entries: &[TocEntry], html: &mut String) {
        if entries.is_empty() {
            return;
        }
        html.push_str("<ol>\n");
        for entry in entries {
            html.push_str(&format!(
                "  <li><a href=\"{}\">{}</a>",
                escape(&entry.href),
                escape(&entry.title)
            ));
            if !entry.children.is_empty() {
                html.push('\n');
                Self::build_nav_list(&entry.children, html);
            }
            html.push_str("</li>\n");
        }
        html.push_str("</ol>\n");
    }

    /// Generate EPUB 2 compatible `toc.ncx`
    fn generate_ncx(&self) -> String {
        let title = self.metadata.title.as_deref().unwrap_or("Untitled");

        let max_page = self.page_list.len(); // A rough estimate for total/max pages

        let mut ncx = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\">\n  <head>\n    <meta name=\"dtb:uid\" content=\"urn:uuid:default-epub-rs-id\"/>\n    <meta name=\"dtb:depth\" content=\"1\"/>\n    <meta name=\"dtb:totalPageCount\" content=\"{}\"/>\n    <meta name=\"dtb:maxPageNumber\" content=\"{}\"/>\n  </head>\n  <docTitle><text>{}</text></docTitle>\n  <navMap>\n",
            max_page,
            max_page,
            escape(title)
        );

        let mut play_order = 0;
        Self::build_ncx_navpoints(&self.toc, &mut ncx, &mut play_order);

        ncx.push_str("  </navMap>\n");

        if !self.page_list.is_empty() {
            ncx.push_str("  <pageList>\n    <navLabel><text>Pages</text></navLabel>\n");
            for (i, page) in self.page_list.iter().enumerate() {
                play_order += 1;
                ncx.push_str(&format!(
                    "    <pageTarget id=\"page-{}\" type=\"normal\" value=\"{}\" playOrder=\"{}\">\n      <navLabel><text>{}</text></navLabel>\n      <content src=\"{}\"/>\n    </pageTarget>\n",
                    i + 1, escape(&page.name), play_order, escape(&page.name), escape(&page.href)
                ));
            }
            ncx.push_str("  </pageList>\n");
        }

        ncx.push_str("</ncx>");
        ncx
    }

    fn build_ncx_navpoints(entries: &[TocEntry], ncx: &mut String, play_order: &mut usize) {
        for entry in entries {
            *play_order += 1;
            let current_order = *play_order;
            ncx.push_str(&format!(
                "    <navPoint id=\"navPoint-{}\" playOrder=\"{}\">\n      <navLabel><text>{}</text></navLabel>\n      <content src=\"{}\"/>\n",
                current_order, current_order, escape(&entry.title), escape(&entry.href)
            ));

            if !entry.children.is_empty() {
                Self::build_ncx_navpoints(&entry.children, ncx, play_order);
            }

            ncx.push_str("    </navPoint>\n");
        }
    }

    /// Helper to infer EPUB 3 properties (scripted, mathml, svg) from HTML content.
    fn infer_properties(content: &[u8]) -> Option<String> {
        let mut props = Vec::new();
        // A simple heuristic search. For production, a proper DOM traversal could be used,
        // but string scanning is much faster and sufficient for detecting these tags.
        let html_str = String::from_utf8_lossy(content);
        let lower = html_str.to_lowercase();

        if lower.contains("<script") {
            props.push("scripted");
        }
        if lower.contains("<svg") {
            props.push("svg");
        }
        if lower.contains("<math") {
            props.push("mathml");
        }

        if props.is_empty() {
            None
        } else {
            Some(props.join(" "))
        }
    }

    /// Helper to generate the OPF XML content using quick-xml.
    fn generate_opf(&self, has_toc: bool) -> Result<Vec<u8>, EpubError> {
        let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);

        // <?xml version="1.0" encoding="UTF-8"?>
        writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;

        // <package version="3.0" unique-identifier="pub-id" xmlns="http://www.idpf.org/2007/opf">
        let mut package = BytesStart::new("package");
        package.push_attribute(("version", "3.0"));
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
            if let Some(prop) = &res.properties {
                item.push_attribute(("properties", prop.as_str()));
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
            None
        );

        // Scripts
        let script = EpubBuilder::infer_properties(
            b"<html><head><script src='test.js'></script></head></html>",
        );
        assert_eq!(script.as_deref(), Some("scripted"));

        // Case insensitivity
        let script_upper = EpubBuilder::infer_properties(b"<SCRIPT>alert(1);</SCRIPT>");
        assert_eq!(script_upper.as_deref(), Some("scripted"));

        // SVG
        let svg = EpubBuilder::infer_properties(b"<p>Graphic: <svg></svg></p>");
        assert_eq!(svg.as_deref(), Some("svg"));

        // MathML
        let math = EpubBuilder::infer_properties(
            b"<math xmlns='http://www.w3.org/1998/Math/MathML'><mi>x</mi></math>",
        );
        assert_eq!(math.as_deref(), Some("mathml"));

        // Multiple properties
        let combo = EpubBuilder::infer_properties(b"<svg></svg><div><script></script></div>");
        let combo_str = combo.unwrap();
        // Since we push to a Vec and join, order matters for the exact string, but typically it's scripted svg mathml
        assert!(combo_str.contains("scripted"));
        assert!(combo_str.contains("svg"));
    }

    #[test]
    fn test_xml_entity_escaping() {
        use crate::model::{EpubVersion, Metadata};
        use std::io::Cursor;

        let metadata = Metadata {
            title: Some("Me & You <3".to_string()),
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
