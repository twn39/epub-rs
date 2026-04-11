//! EPUB generator module using Builder pattern.

use crate::error::EpubError;
use crate::model::{EpubVersion, Metadata, SpineItem, TocEntry};
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Writer;
use quick_xml::escape::escape;
use std::io::{Read, Seek, Write};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;
use zip::ZipWriter;

/// Represents the content of a resource, which can be either fully in-memory or a readable stream.
pub enum ResourceContent {
    Bytes(Vec<u8>),
    Stream(Box<dyn Read + Send + Sync>),
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
    version: EpubVersion,
    metadata: Metadata,
    resources: Vec<Resource>,
    spine: Vec<SpineItem>, // list of spine items
    toc: Vec<TocEntry>,
    landmarks: Vec<Landmark>,
    page_list: Vec<PageListEntry>,
    cover_id: Option<String>,
}

impl Default for EpubBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl EpubBuilder {
    /// Create a new empty EPUB builder
    pub fn new() -> Self {
        Self {
            version: EpubVersion::default(),
            metadata: Metadata::default(),
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
    pub fn add_page(
        mut self,
        name: impl Into<String>,
        href: impl Into<String>,
    ) -> Self {
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
        let mut spine_item = SpineItem::new(id_str.clone());
        spine_item.layout_override = layout;
        spine_item.page_spread = spread;
        
        self.spine.push(spine_item);
        self.resources.push(Resource {
            id: id_str,
            href: href.into(),
            media_type: "application/xhtml+xml".to_string(),
            content: ResourceContent::Bytes(content.into()),
            properties: None,
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
        let options_stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("mimetype", options_stored)?;
        zip.write_all(b"application/epub+zip")?;

        // Standard compression for the rest of the files
        let options_deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

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
                ResourceContent::Bytes(bytes) => {
                    zip.write_all(&bytes)?;
                }
                ResourceContent::Stream(ref mut stream) => {
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
        let mut html = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n<head><title>Navigation</title></head>\n<body>\n<nav epub:type=\"toc\" id=\"toc\">\n<h1>Table of Contents</h1>\n");
        Self::build_nav_list(&self.toc, &mut html);
        html.push_str("</nav>\n");

        if !self.landmarks.is_empty() {
            html.push_str("<nav epub:type=\"landmarks\" id=\"landmarks\">\n<h2>Landmarks</h2>\n<ol>\n");
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
            html.push_str("<nav epub:type=\"page-list\" id=\"page-list\">\n<h2>Page List</h2>\n<ol>\n");
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
        if entries.is_empty() { return; }
        html.push_str("<ol>\n");
        for entry in entries {
            html.push_str(&format!("  <li><a href=\"{}\">{}</a>", escape(&entry.href), escape(&entry.title)));
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
        
        let mut ncx = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\">\n  <head>\n    <meta name=\"dtb:uid\" content=\"urn:uuid:default-epub-rs-id\"/>\n    <meta name=\"dtb:depth\" content=\"1\"/>\n    <meta name=\"dtb:totalPageCount\" content=\"{}\"/>\n    <meta name=\"dtb:maxPageNumber\" content=\"{}\"/>\n  </head>\n  <docTitle><text>{}</text></docTitle>\n  <navMap>\n", max_page, max_page, escape(title));
        
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
                        crate::model::PageSpread::Center => properties.push("rendition:page-spread-center"),
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
