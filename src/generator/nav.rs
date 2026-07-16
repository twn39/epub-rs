//! Navigation document writers (EPUB 3 nav.xhtml + EPUB 2/3 NCX).
//!
//! Kept separate from the builder facade so packaging orchestration in `mod.rs`
//! does not grow with every nav/NCX markup change.

use super::EpubBuilder;
use crate::error::EpubError;
use crate::model::TocEntry;
use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use std::io::Write;

impl EpubBuilder {
    /// Generate EPUB 3 `nav.xhtml`
    pub(super) fn generate_nav_xhtml(&self) -> Result<String, EpubError> {
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
    pub(super) fn generate_ncx(&self) -> Result<String, EpubError> {
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
}
