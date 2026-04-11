//! EPUB parser module.

use crate::error::EpubError;
use crate::model::{EpubBook, ManifestItem};
use crate::provider::{DirProvider, EpubProvider, ZipProvider};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::{Read, Seek};

/// A struct that handles unpacking and parsing EPUB files.
pub struct EpubArchive<P: EpubProvider> {
    provider: P,
}

impl<R: Read + Seek> EpubArchive<ZipProvider<R>> {
    /// Create a new `EpubArchive` from a generic reader containing a ZIP file
    pub fn new(reader: R) -> Result<Self, EpubError> {
        let provider = ZipProvider::new(reader)?;
        Ok(Self { provider })
    }
}

impl EpubArchive<DirProvider> {
    /// Create a new `EpubArchive` from an unzipped local directory
    pub fn from_dir<P: AsRef<std::path::Path>>(path: P) -> Self {
        let provider = DirProvider::new(path);
        Self { provider }
    }
}

impl<P: EpubProvider> EpubArchive<P> {
    /// Get all available renditions (rootfiles) in the EPUB container.
    pub fn get_renditions(&mut self) -> Result<Vec<String>, EpubError> {
        self.parse_container()
    }

    /// Parse the default (first) rendition of the EPUB archive.
    pub fn parse(&mut self) -> Result<EpubBook, EpubError> {
        let rootfiles = self.parse_container()?;
        self.parse_rendition(&rootfiles[0])
    }

    /// Parse a specific rendition by its OPF path.
    pub fn parse_rendition(&mut self, opf_path: &str) -> Result<EpubBook, EpubError> {
        self.parse_opf(opf_path)
    }

    /// Reads `META-INF/container.xml` to find the paths of the OPF files
    fn parse_container(&mut self) -> Result<Vec<String>, EpubError> {
        let mut container_file = self
            .provider
            .read_file("META-INF/container.xml")
            .map_err(|_| EpubError::MissingContainer)?;

        let mut buf = String::new();
        container_file.read_to_string(&mut buf)?;

        let mut reader = Reader::from_str(&buf);
        reader.config_mut().trim_text(true);

        let mut rootfiles = Vec::new();
        let mut event_buf = Vec::new();

        loop {
            match reader.read_event_into(&mut event_buf)? {
                Event::Empty(ref e) | Event::Start(ref e) => {
                    if e.name().as_ref() == b"rootfile" {
                        for attr in e.attributes() {
                            let attr = attr?;
                            if attr.key.as_ref() == b"full-path" {
                                rootfiles.push(String::from_utf8_lossy(&attr.value).into_owned());
                            }
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            event_buf.clear();
        }

        if rootfiles.is_empty() {
            Err(EpubError::InvalidFormat("No rootfile full-path found in container.xml".to_string()))
        } else {
            Ok(rootfiles)
        }
    }

    /// Parses the OPF file (usually .opf) to build the domain models
    fn parse_opf(&mut self, opf_path: &str) -> Result<EpubBook, EpubError> {
        let mut opf_file = self.provider.read_file(opf_path)?;
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
        let mut current_id = None; // For elements like <dc:creator id="creator1">

        // A temporary map to store metadata refinements (refines -> property -> value)
        let mut refinements: std::collections::HashMap<String, std::collections::HashMap<String, String>> = std::collections::HashMap::new();
        // A temporary map connecting an ID to the index in the creators vector
        let mut creator_id_to_idx: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

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
                    } else if current_tag.ends_with("creator") {
                        current_id = None;
                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = String::from_utf8_lossy(attr.key.into_inner());
                            if key == "id" {
                                current_id = Some(String::from_utf8_lossy(&attr.value).into_owned());
                            } else if key == "opf:role" {
                                // EPUB 2 style role attribute
                                // We'll handle this in the Event::Text section by updating the last creator
                            }
                        }
                    } else if current_tag == "meta" {
                        // Check for EPUB 3 refinements
                        let mut refines = None;
                        let mut property = None;
                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = String::from_utf8_lossy(attr.key.into_inner());
                            if key == "refines" {
                                let val = String::from_utf8_lossy(&attr.value).into_owned();
                                if val.starts_with('#') {
                                    refines = Some(val[1..].to_string());
                                }
                            } else if key == "property" {
                                property = Some(String::from_utf8_lossy(&attr.value).into_owned());
                            }
                        }
                        let ref_id = refines.clone();
                        let p_name = property.clone();
                        if let (Some(r), Some(p)) = (ref_id, p_name) {
                            // We don't have the text yet, store the state to catch in Event::Text
                            current_id = Some(r); // repurpose current_id to hold the target refines ID
                            current_tag = format!("meta_refines_{}", p);
                        } else if let Some(p) = property {
                            // Global properties like rendition:layout
                            current_tag = format!("meta_global_{}", p);
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
                        let mut properties = None;

                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = String::from_utf8_lossy(attr.key.into_inner());
                            let value = String::from_utf8_lossy(&attr.value).into_owned();

                            match key.as_ref() {
                                "id" => id = value,
                                "href" => {
                                    // URL Decode the href as EPUB paths are URI encoded (e.g. "chapter%201.xhtml")
                                    let decoded = percent_encoding::percent_decode_str(&value).decode_utf8_lossy().into_owned();
                                    href = decoded;
                                },
                                "media-type" => media_type = value,
                                "properties" => properties = Some(value),
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
                                    properties,
                                },
                            );
                        }
                    } else if name_str.ends_with("itemref") {
                        // Extract spine reading order
                        let mut idref = String::new();
                        let mut linear = true;
                        let mut layout_override = None;
                        let mut page_spread = None;
                        
                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = String::from_utf8_lossy(attr.key.into_inner());
                            let val = String::from_utf8_lossy(&attr.value).into_owned();
                            if key == "idref" {
                                idref = val;
                            } else if key == "linear" {
                                linear = val != "no";
                            } else if key == "properties" {
                                if val.contains("rendition:layout-reflowable") {
                                    layout_override = Some(crate::model::LayoutType::Reflowable);
                                } else if val.contains("rendition:layout-pre-paginated") {
                                    layout_override = Some(crate::model::LayoutType::PrePaginated);
                                }
                                
                                if val.contains("page-spread-left") || val.contains("rendition:page-spread-left") {
                                    page_spread = Some(crate::model::PageSpread::Left);
                                } else if val.contains("page-spread-right") || val.contains("rendition:page-spread-right") {
                                    page_spread = Some(crate::model::PageSpread::Right);
                                } else if val.contains("rendition:page-spread-center") {
                                    page_spread = Some(crate::model::PageSpread::Center);
                                }
                            }
                        }
                        
                        if !idref.is_empty() {
                            book.spine.push(crate::model::SpineItem { idref, linear, layout_override, page_spread });
                        }
                    }
                }
                Event::Text(e) => {
                    if in_metadata {
                        let text = String::from_utf8_lossy(&e).into_owned();
                        if current_tag.ends_with("title") {
                            book.metadata.title = Some(text);
                        } else if current_tag.ends_with("creator") {
                            let creator = crate::model::Creator::new(&text);
                            // If this creator tag had an ID, remember its index
                            if let Some(id) = &current_id {
                                creator_id_to_idx.insert(id.clone(), book.metadata.creators.len());
                            }
                            book.metadata.creators.push(creator);
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
                        } else if current_tag.starts_with("meta_refines_") {
                            if let Some(refined_id) = &current_id {
                                let property = current_tag.strip_prefix("meta_refines_").unwrap().to_string();
                                let entry = refinements.entry(refined_id.clone()).or_default();
                                entry.insert(property, text);
                            }
                        } else if current_tag.starts_with("meta_global_") {
                            let property = current_tag.strip_prefix("meta_global_").unwrap();
                            if property == "rendition:layout" {
                                if text == "pre-paginated" {
                                    book.metadata.layout = crate::model::LayoutType::PrePaginated;
                                } else {
                                    book.metadata.layout = crate::model::LayoutType::Reflowable;
                                }
                            }
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
                    current_id = None;
                }
                Event::Eof => break,
                _ => {}
            }
            event_buf.clear();
        }

        // Apply metadata refinements
        for (id, props) in refinements {
            if let Some(&idx) = creator_id_to_idx.get(&id)
                && let Some(creator) = book.metadata.creators.get_mut(idx) {
                    if let Some(role) = props.get("role") {
                        creator.role = Some(role.clone());
                    }
                    if let Some(file_as) = props.get("file-as") {
                        creator.file_as = Some(file_as.clone());
                    }
                }
        }

        Ok(book)
    }

    /// Get a readable stream for a resource given its manifest href
    pub fn read_resource_by_href<'a>(&'a mut self, book: &EpubBook, href: &str) -> Result<Box<dyn Read + 'a>, EpubError> {
        let res_path = if book.opf_dir.is_empty() {
            href.to_string()
        } else {
            format!("{}/{}", book.opf_dir, href)
        };
        
        let file = self.provider.read_file(&res_path)?;
        Ok(file)
    }

    /// Get a readable stream for a resource given its manifest ID
    pub fn read_resource_by_id<'a>(&'a mut self, book: &EpubBook, id: &str) -> Result<Box<dyn Read + 'a>, EpubError> {
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
        let spine_index = book.spine.iter().position(|s| s.idref == id)
            .ok_or_else(|| EpubError::InvalidFormat(format!("ID {} not found in spine", id)))?;
        
        let base_cfi = crate::cfi::EpubCfi::generate_spine_base_cfi(spine_index, id);
        let raw_html = self.get_resource_by_id(book, id)?;
        let html_str = String::from_utf8_lossy(&raw_html);
        
        crate::processor::inject_cfi_dom(&html_str, &base_cfi)
    }
}
