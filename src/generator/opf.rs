//! OPF package document writer.
//!
//! Isolated so metadata/manifest/spine serialization can evolve without
//! bloating the `EpubBuilder` orchestration surface in `mod.rs`.

use super::EpubBuilder;
use crate::error::EpubError;
use crate::model::EpubVersion;
use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};

impl EpubBuilder {
    /// Helper to generate the OPF XML content using quick-xml.
    pub(super) fn generate_opf(&self, has_toc: bool) -> Result<Vec<u8>, EpubError> {
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
