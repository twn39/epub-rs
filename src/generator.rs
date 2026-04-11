//! EPUB generator module using Builder pattern.

use crate::error::EpubError;
use crate::model::Metadata;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Writer;
use quick_xml::escape::escape;
use std::io::{Seek, Write};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;
use zip::ZipWriter;

/// Represents a table of contents entry.
#[derive(Clone, Debug)]
pub struct TocEntry {
    pub title: String,
    pub href: String,
}

/// Represents a file to be added to the EPUB archive.
#[derive(Clone)]
struct Resource {
    id: String,
    href: String,
    media_type: String,
    content: Vec<u8>,
    properties: Option<String>,
}

/// A Builder for creating EPUB files.
pub struct EpubBuilder {
    metadata: Metadata,
    resources: Vec<Resource>,
    spine: Vec<String>, // list of resource IDs
    toc: Vec<TocEntry>,
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
            metadata: Metadata::default(),
            resources: Vec::new(),
            spine: Vec::new(),
            toc: Vec::new(),
        }
    }

    /// Set the EPUB metadata
    pub fn metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Add a resource (HTML, image, CSS, etc.) to the EPUB manifest.
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
            content: content.into(),
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
        self.spine.push(id_str.clone());
        self.resources.push(Resource {
            id: id_str,
            href: href.into(),
            media_type: "application/xhtml+xml".to_string(),
            content: content.into(),
            properties: None,
        });
        self
    }

    /// Add a chapter and also add it to the Table of Contents (TOC).
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

        self.toc.push(TocEntry {
            title: title_str,
            href: href_str.clone(),
        });

        self.spine.push(id_str.clone());
        self.resources.push(Resource {
            id: id_str,
            href: href_str,
            media_type: "application/xhtml+xml".to_string(),
            content: content.into(),
            properties: None,
        });
        self
    }

    /// Build the EPUB and write it to the provided writer (e.g., `std::fs::File` or `Vec<u8>`).
    pub fn generate<W: Write + Seek>(mut self, writer: W) -> Result<(), EpubError> {
        let mut zip = ZipWriter::new(writer);

        // Auto-generate Navigation documents if we have TOC entries
        let has_toc = !self.toc.is_empty();
        if has_toc {
            // EPUB 3 nav.xhtml
            let nav_html = self.generate_nav_xhtml();
            self.resources.push(Resource {
                id: "nav".to_string(),
                href: "nav.xhtml".to_string(),
                media_type: "application/xhtml+xml".to_string(),
                content: nav_html.into_bytes(),
                properties: Some("nav".to_string()),
            });

            // EPUB 2 toc.ncx (for fallback compatibility)
            let ncx_xml = self.generate_ncx();
            self.resources.push(Resource {
                id: "ncx".to_string(),
                href: "toc.ncx".to_string(),
                media_type: "application/x-dtbncx+xml".to_string(),
                content: ncx_xml.into_bytes(),
                properties: None,
            });
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

        // 3. Write resources
        for res in &self.resources {
            let zip_path = format!("OEBPS/{}", res.href);
            zip.start_file(&zip_path, options_deflated)?;
            zip.write_all(&res.content)?;
        }

        // 4. Write `OEBPS/content.opf`
        zip.start_file("OEBPS/content.opf", options_deflated)?;
        let opf_content = self.generate_opf(has_toc)?;
        zip.write_all(&opf_content)?;

        zip.finish()?;
        Ok(())
    }

    /// Generate EPUB 3 `nav.xhtml`
    fn generate_nav_xhtml(&self) -> String {
        let mut html = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n<head><title>Navigation</title></head>\n<body>\n<nav epub:type=\"toc\" id=\"toc\">\n<h1>Table of Contents</h1>\n<ol>\n");
        for entry in &self.toc {
            html.push_str(&format!("  <li><a href=\"{}\">{}</a></li>\n", escape(&entry.href), escape(&entry.title)));
        }
        html.push_str("</ol>\n</nav>\n</body>\n</html>");
        html
    }

    /// Generate EPUB 2 compatible `toc.ncx`
    fn generate_ncx(&self) -> String {
        let title = self.metadata.title.as_deref().unwrap_or("Untitled");
        let mut ncx = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\">\n  <head>\n    <meta name=\"dtb:uid\" content=\"urn:uuid:default-epub-rs-id\"/>\n    <meta name=\"dtb:depth\" content=\"1\"/>\n    <meta name=\"dtb:totalPageCount\" content=\"0\"/>\n    <meta name=\"dtb:maxPageNumber\" content=\"0\"/>\n  </head>\n  <docTitle><text>{}</text></docTitle>\n  <navMap>\n", escape(title));
        
        for (i, entry) in self.toc.iter().enumerate() {
            let order = i + 1;
            ncx.push_str(&format!(
                "    <navPoint id=\"navPoint-{}\" playOrder=\"{}\">\n      <navLabel><text>{}</text></navLabel>\n      <content src=\"{}\"/>\n    </navPoint>\n",
                order, order, escape(&entry.title), escape(&entry.href)
            ));
        }
        ncx.push_str("  </navMap>\n</ncx>");
        ncx
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
        for creator in &self.metadata.creators {
            Self::write_text_element(&mut writer, "dc:creator", creator)?;
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
        for idref in &self.spine {
            let mut itemref = BytesStart::new("itemref");
            itemref.push_attribute(("idref", idref.as_str()));
            writer.write_event(Event::Empty(itemref))?;
        }
        writer.write_event(Event::End(BytesEnd::new("spine")))?;

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
