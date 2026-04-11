//! EPUB parser module.

use crate::error::EpubError;
use crate::model::{EpubBook, ManifestItem};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::{Read, Seek};
use zip::ZipArchive;

/// A struct that handles unpacking and parsing EPUB files.
pub struct EpubArchive<R: Read + Seek> {
    archive: ZipArchive<R>,
}

impl<R: Read + Seek> EpubArchive<R> {
    /// Create a new `EpubArchive` from a generic reader
    pub fn new(reader: R) -> Result<Self, EpubError> {
        let archive = ZipArchive::new(reader)?;
        Ok(Self { archive })
    }

    /// Parse the EPUB archive and extract metadata, manifest, and spine
    pub fn parse(&mut self) -> Result<EpubBook, EpubError> {
        let rootfile_path = self.parse_container()?;
        self.parse_opf(&rootfile_path)
    }

    /// Reads `META-INF/container.xml` to find the path of the primary OPF file
    fn parse_container(&mut self) -> Result<String, EpubError> {
        let mut container_file = self
            .archive
            .by_name("META-INF/container.xml")
            .map_err(|_| EpubError::MissingContainer)?;

        let mut buf = String::new();
        container_file.read_to_string(&mut buf)?;

        let mut reader = Reader::from_str(&buf);
        reader.config_mut().trim_text(true);

        let mut rootfile_path = None;
        let mut event_buf = Vec::new();

        loop {
            match reader.read_event_into(&mut event_buf)? {
                Event::Empty(ref e) | Event::Start(ref e) => {
                    if e.name().as_ref() == b"rootfile" {
                        for attr in e.attributes() {
                            let attr = attr?;
                            if attr.key.as_ref() == b"full-path" {
                                rootfile_path =
                                    Some(String::from_utf8_lossy(&attr.value).into_owned());
                                break;
                            }
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            event_buf.clear();
        }

        rootfile_path.ok_or_else(|| {
            EpubError::InvalidFormat("No rootfile full-path found in container.xml".to_string())
        })
    }

    /// Parses the OPF file (usually .opf) to build the domain models
    fn parse_opf(&mut self, opf_path: &str) -> Result<EpubBook, EpubError> {
        let mut opf_file = self.archive.by_name(opf_path)?;
        let mut buf = String::new();
        opf_file.read_to_string(&mut buf)?;

        let mut reader = Reader::from_str(&buf);
        reader.config_mut().trim_text(true);

        let mut book = EpubBook::default();
        if let Some(pos) = opf_path.rfind('/') {
            book.opf_dir = opf_path[..pos].to_string();
        } else {
            book.opf_dir = String::new();
        }
        let mut event_buf = Vec::new();

        // State tracking
        let mut in_metadata = false;
        let mut current_tag = String::new();

        loop {
            match reader.read_event_into(&mut event_buf)? {
                Event::Start(ref e) => {
                    let name = e.name();
                    let name_str = String::from_utf8_lossy(name.into_inner()).into_owned();
                    current_tag = name_str.clone();

                    if name_str.ends_with("metadata") {
                        in_metadata = true;
                    } else if name_str.ends_with("spine") {
                        // Extract toc attribute from spine
                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = String::from_utf8_lossy(attr.key.into_inner());
                            if key == "toc" {
                                book.toc_id = Some(String::from_utf8_lossy(&attr.value).into_owned());
                            }
                        }
                    }
                }
                Event::Empty(ref e) => {
                    let name = e.name();
                    let name_str = String::from_utf8_lossy(name.into_inner()).into_owned();

                    if name_str.ends_with("item") {
                        // Extract manifest item
                        let mut id = String::new();
                        let mut href = String::new();
                        let mut media_type = String::new();

                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = String::from_utf8_lossy(attr.key.into_inner());
                            let value = String::from_utf8_lossy(&attr.value).into_owned();

                            match key.as_ref() {
                                "id" => id = value,
                                "href" => href = value,
                                "media-type" => media_type = value,
                                _ => {}
                            }
                        }

                        if !id.is_empty() && !href.is_empty() {
                            book.manifest.insert(
                                id.clone(),
                                ManifestItem {
                                    id,
                                    href,
                                    media_type,
                                },
                            );
                        }
                    } else if name_str.ends_with("itemref") {
                        // Extract spine reading order
                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = String::from_utf8_lossy(attr.key.into_inner());
                            if key == "idref" {
                                book.spine
                                    .push(String::from_utf8_lossy(&attr.value).into_owned());
                            }
                        }
                    }
                }
                Event::Text(e) => {
                    if in_metadata {
                        let text = String::from_utf8_lossy(&e).into_owned();
                        if current_tag.ends_with("title") {
                            book.metadata.title = Some(text);
                        } else if current_tag.ends_with("creator") {
                            book.metadata.creators.push(text);
                        } else if current_tag.ends_with("language") {
                            book.metadata.language = Some(text);
                        } else if current_tag.ends_with("identifier") {
                            book.metadata.identifier = Some(text);
                        } else if current_tag.ends_with("publisher") {
                            book.metadata.publisher = Some(text);
                        } else if current_tag.ends_with("description") {
                            book.metadata.description = Some(text);
                        } else if current_tag.ends_with("date") {
                            book.metadata.date = Some(text);
                        } else if current_tag.ends_with("rights") {
                            book.metadata.rights = Some(text);
                        } else if current_tag.ends_with("subject") {
                            book.metadata.subjects.push(text);
                        }
                    }
                }
                Event::End(ref e) => {
                    let name = e.name();
                    let name_str = String::from_utf8_lossy(name.into_inner());
                    if name_str.ends_with("metadata") {
                        in_metadata = false;
                    }
                    current_tag.clear();
                }
                Event::Eof => break,
                _ => {}
            }
            event_buf.clear();
        }

        Ok(book)
    }

    /// Get a readable stream for a resource given its manifest href
    pub fn read_resource_by_href<'a>(&'a mut self, book: &EpubBook, href: &str) -> Result<zip::read::ZipFile<'a, R>, EpubError> {
        let zip_path = if book.opf_dir.is_empty() {
            href.to_string()
        } else {
            format!("{}/{}", book.opf_dir, href)
        };
        
        let file = self.archive.by_name(&zip_path)?;
        Ok(file)
    }

    /// Get a readable stream for a resource given its manifest ID
    pub fn read_resource_by_id<'a>(&'a mut self, book: &EpubBook, id: &str) -> Result<zip::read::ZipFile<'a, R>, EpubError> {
        let href = book.manifest.get(id)
            .ok_or_else(|| EpubError::InvalidFormat(format!("ID {} not found in manifest", id)))?
            .href.clone();
        self.read_resource_by_href(book, &href)
    }

    /// Read the raw bytes of a resource from the archive given its manifest href
    pub fn get_resource_by_href(&mut self, book: &EpubBook, href: &str) -> Result<Vec<u8>, EpubError> {
        let mut file = self.read_resource_by_href(book, href)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// Helper to get a resource by its manifest ID
    pub fn get_resource_by_id(&mut self, book: &EpubBook, id: &str) -> Result<Vec<u8>, EpubError> {
        let mut file = self.read_resource_by_id(book, id)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// Reads a chapter's HTML and automatically injects `data-cfi` attributes into all DOM nodes.
    /// This is a high-level method designed for building Web Readers.
    /// 
    /// It automatically calculates the `base_cfi` (OPF context) for the given spine item.
    pub fn get_chapter_with_cfi(&mut self, book: &EpubBook, id: &str) -> Result<String, EpubError> {
        let spine_index = book.spine.iter().position(|s| s == id)
            .ok_or_else(|| EpubError::InvalidFormat(format!("ID {} not found in spine", id)))?;
        
        let base_cfi = crate::cfi::EpubCfi::generate_spine_base_cfi(spine_index, id);
        let raw_html = self.get_resource_by_id(book, id)?;
        let html_str = String::from_utf8_lossy(&raw_html);
        
        crate::processor::inject_cfi_dom(&html_str, &base_cfi)
    }
}
