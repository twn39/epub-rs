//! Navigation document parsing: TOC, page-list, and landmarks.
//!
//! Supports both EPUB 3 `nav.xhtml` (all three types in one pass)
//! and EPUB 2 `.ncx` (TOC + page-list in one streaming pass).
//!
//! Mirrors go-toolkit's `parser_navdoc.go` and `parser_ncx.go`.

use crate::error::EpubError;
use crate::model::{EpubBook, NavigationDocument, TocEntry};
use crate::provider::EpubProvider;
use kuchikiki::traits::*;
use quick_xml::Reader;
use quick_xml::events::Event;

use super::EpubArchive;

impl<P: EpubProvider> EpubArchive<P> {
    /// Parses **all** navigation data from the EPUB in a **single I/O + single parse** operation.
    ///
    /// Returns a [`NavigationDocument`] containing:
    /// - `toc`       — Table of Contents
    /// - `page_list` — Print page → document position mapping
    /// - `landmarks` — Structural navigation points (EPUB 3 only)
    ///
    /// Priority (identical to go-toolkit):
    /// 1. EPUB 3 `nav.xhtml` → scans **all** `<nav epub:type="…">` elements in one DOM pass
    /// 2. EPUB 2 `.ncx`      → parses `<navMap>` (toc) + `<pageList>` (page-list) in one pass
    ///
    /// Calling both `get_toc()` and `get_page_list()` separately would read and parse the file
    /// twice; use this method when you need more than one navigation list.
    pub fn get_navigation(&mut self, book: &EpubBook) -> Result<NavigationDocument, EpubError> {
        // 1. Prefer EPUB 3 nav.xhtml — one read, all types
        if let Some(nav_item) = book
            .manifest
            .values()
            .find(|i| i.properties.iter().any(|p| p == "nav"))
        {
            let bytes = self.get_resource_by_id(book, &nav_item.id)?;
            let html = String::from_utf8_lossy(&bytes).to_string();
            return Self::parse_nav_xhtml_all(&html);
        }

        // 2. Fallback to EPUB 2 NCX — one read, toc + page-list
        if let Some(toc_id) = &book.toc_id
            && let Some(ncx_item) = book.manifest.get(toc_id)
        {
            let bytes = self.get_resource_by_id(book, &ncx_item.id)?;
            let xml = String::from_utf8_lossy(&bytes).to_string();
            return Self::parse_ncx_all(&xml);
        }

        Ok(NavigationDocument::default())
    }

    /// Returns the Table of Contents.
    ///
    /// This is a convenience wrapper around [`get_navigation`] that reads the nav file
    /// once and returns only `navigation.toc`. Prefer [`get_navigation`] when you also
    /// need the page list or landmarks, to avoid reading the file twice.
    pub fn get_toc(&mut self, book: &EpubBook) -> Result<Vec<TocEntry>, EpubError> {
        Ok(self.get_navigation(book)?.toc)
    }

    /// Returns the Page List (`epub:type="page-list"` or NCX `<pageList>`).
    ///
    /// Each returned `TocEntry` has:
    /// - `title` = page label as printed (`"1"`, `"42"`, `"xii"`, `"A-3"`)
    /// - `href`  = document position, typically with a fragment (`"ch3.xhtml#p42"`)
    /// - `children` = always empty (page lists are flat)
    ///
    /// Returns `Ok(Vec::new())` if no page list is present (page lists are optional
    /// per the EPUB specification and not present in most EPUBs).
    ///
    /// Convenience wrapper around [`get_navigation`].
    pub fn get_page_list(&mut self, book: &EpubBook) -> Result<Vec<TocEntry>, EpubError> {
        Ok(self.get_navigation(book)?.page_list)
    }

    /// Parse a `nav.xhtml` document, extracting **all** `<nav epub:type="…">` elements
    /// in a single DOM traversal.
    ///
    /// Mirrors go-toolkit `ParseNavDoc`:
    /// ```go
    /// for _, nav := range body.SelectElements("//nav") {
    ///     types, links := parseNavElement(nav, ...)
    ///     ret[type] = links   // collects ALL types in one loop
    /// }
    /// ```
    ///
    /// The same `parse_ol_node` method is reused for every nav type (toc, page-list,
    /// landmarks) — no duplication.
    pub(super) fn parse_nav_xhtml_all(html: &str) -> Result<NavigationDocument, EpubError> {
        let document = kuchikiki::parse_html().one(html);
        let mut nav_doc = NavigationDocument::default();

        // Iterate ALL <nav> elements (one DOM parse, multiple results)
        let nav_nodes = document.select("nav").unwrap_or_else(|_| {
            // select() only errors on invalid CSS; "nav" is always valid
            panic!("'nav' is a valid CSS selector")
        });

        for nav in nav_nodes {
            // Read epub:type attribute. kuchikiki stores the attribute name
            // verbatim; the colon is escaped in CSS but stored as-is in attrs.
            let attrs = nav.attributes.borrow();
            let epub_type = attrs.get("epub:type").unwrap_or("").to_string();
            drop(attrs);

            if epub_type.is_empty() {
                continue;
            }

            // Parse the <ol> — identical method for ALL nav types.
            // This is the key reuse: parse_ol_node is called once per nav element.
            let entries = match nav.as_node().select_first("ol") {
                Ok(ol) => Self::parse_ol_node(ol.as_node()),
                Err(_) => continue,
            };

            if entries.is_empty() {
                continue;
            }

            // epub:type may contain multiple space-separated tokens per EPUB spec.
            // We handle each token — e.g. `epub:type="toc landmarks"`.
            for token in epub_type.split_whitespace() {
                // Strip any "epub:" namespace prefix that may appear in the attribute value
                let key = token.trim_start_matches("epub:");
                match key {
                    "toc"       => nav_doc.toc       = entries.clone(),
                    "page-list" => nav_doc.page_list = entries.clone(),
                    "landmarks" => nav_doc.landmarks = entries.clone(),
                    _           => {} // ignore unknown types (forward-compatible)
                }
            }
        }

        // Fallback for malformed nav.xhtml that lacks epub:type:
        // if TOC is still empty, try <nav id="toc"> then first <nav>.
        if nav_doc.toc.is_empty() {
            let toc_node = document
                .select_first("nav#toc")
                .or_else(|_| document.select_first("nav"));
            if let Ok(nav) = toc_node {
                if let Ok(ol) = nav.as_node().select_first("ol") {
                    nav_doc.toc = Self::parse_ol_node(ol.as_node());
                }
            }
        }

        Ok(nav_doc)
    }

    /// Parse an `<ol>` element into a flat or nested list of [`TocEntry`] items.
    ///
    /// Used uniformly for every nav type: TOC (with nesting), page-list (flat),
    /// and landmarks (flat). This is the single shared implementation.
    pub(super) fn parse_ol_node(ol: &kuchikiki::NodeRef) -> Vec<TocEntry> {
        let mut entries = Vec::new();
        // Since `select` might grab deep children, we manually iterate direct children.
        for li in ol.children().filter(|c| {
            c.as_element()
                .is_some_and(|e| e.name.local.as_ref() == "li")
        }) {
            if let Ok(a_node) = li.select_first("a") {
                let raw_href = a_node
                    .attributes
                    .borrow()
                    .get("href")
                    .unwrap_or("")
                    .to_string();
                let href = percent_encoding::percent_decode_str(&raw_href)
                    .decode_utf8_lossy()
                    .into_owned();
                let title = a_node.text_contents().trim().to_string();

                let mut entry = TocEntry::new(title, href);

                // Recursively parse nested <ol> — used by TOC; ignored for page-list/landmarks
                if let Ok(nested_ol) = li.select_first("ol") {
                    entry.children = Self::parse_ol_node(nested_ol.as_node());
                }
                entries.push(entry);
            }
        }
        entries
    }

    /// Parse an NCX document, extracting both `<navMap>` (TOC) and `<pageList>` (page-list)
    /// in a **single streaming pass** over the XML.
    ///
    /// Mirrors go-toolkit `ParseNCX`:
    /// ```go
    /// toc      := document.SelectElement("//navMap")
    /// pageList := document.SelectElement("//pageList")
    /// ret["toc"]       = parseNavMapElement(toc)
    /// ret["page-list"] = parsePageListElement(pageList)
    /// ```
    ///
    /// State machine regions:
    /// - Default scope → `navPoint` stack → TOC
    /// - `in_page_list` scope → `pageTarget` → page-list entries
    pub(super) fn parse_ncx_all(xml: &str) -> Result<NavigationDocument, EpubError> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        /// Shared state for both navPoint and pageTarget accumulation.
        #[derive(Debug, Clone)]
        struct EntryState {
            title: String,
            href: String,
            children: Vec<TocEntry>,
        }

        let mut stack: Vec<EntryState> = Vec::new();
        let mut toc_entries: Vec<TocEntry> = Vec::new();
        let mut page_list: Vec<TocEntry> = Vec::new();

        // Region flags — mutually exclusive during parsing
        let mut in_page_list = false;
        let mut in_text = false;
        // pageTarget accumulator (only used when in_page_list)
        let mut page_target: Option<EntryState> = None;

        let mut event_buf = Vec::new();

        loop {
            match reader.read_event_into(&mut event_buf)? {
                Event::Start(ref e) => {
                    let name = String::from_utf8_lossy(e.name().into_inner()).into_owned();

                    if name.ends_with("pageList") {
                        // Enter page-list region
                        in_page_list = true;
                    } else if in_page_list && name.ends_with("pageTarget") {
                        // Start accumulating a page target entry
                        page_target = Some(EntryState {
                            title: String::new(),
                            href: String::new(),
                            children: Vec::new(), // always empty for page-list
                        });
                    } else if !in_page_list && name.ends_with("navPoint") {
                        // Push a new navPoint onto the TOC stack
                        stack.push(EntryState {
                            title: String::new(),
                            href: String::new(),
                            children: Vec::new(),
                        });
                    } else if name.ends_with("text") {
                        in_text = true;
                    }
                }

                Event::Empty(ref e) => {
                    let name = String::from_utf8_lossy(e.name().into_inner()).into_owned();
                    if name.ends_with("content") {
                        // <content src="…"/> — present in both navPoint and pageTarget
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"src" {
                                let raw = String::from_utf8_lossy(&attr.value);
                                let href = percent_encoding::percent_decode_str(&raw)
                                    .decode_utf8_lossy()
                                    .into_owned();

                                if in_page_list {
                                    if let Some(ref mut pt) = page_target {
                                        pt.href = href;
                                    }
                                } else if let Some(state) = stack.last_mut() {
                                    state.href = href;
                                }
                            }
                        }
                    }
                }

                Event::Text(e) => {
                    if in_text {
                        let text = String::from_utf8_lossy(&e).into_owned();
                        if in_page_list {
                            if let Some(ref mut pt) = page_target {
                                pt.title = text;
                            }
                        } else if let Some(state) = stack.last_mut() {
                            state.title = text;
                        }
                    }
                }

                Event::End(ref e) => {
                    let name = String::from_utf8_lossy(e.name().into_inner()).into_owned();

                    if name.ends_with("text") {
                        in_text = false;
                    } else if name.ends_with("pageList") {
                        // Exit page-list region
                        in_page_list = false;
                    } else if in_page_list && name.ends_with("pageTarget") {
                        // Commit a page-list entry — only if both title and href are present
                        if let Some(pt) = page_target.take() {
                            if !pt.title.is_empty() && !pt.href.is_empty() {
                                page_list.push(TocEntry {
                                    title: pt.title,
                                    href: pt.href,
                                    children: Vec::new(),
                                });
                            }
                        }
                    } else if !in_page_list
                        && name.ends_with("navPoint")
                        && let Some(state) = stack.pop()
                    {
                        // Commit a TOC entry (with any accumulated children)
                        let entry = TocEntry {
                            title: state.title,
                            href: state.href,
                            children: state.children,
                        };
                        if let Some(parent) = stack.last_mut() {
                            parent.children.push(entry);
                        } else {
                            toc_entries.push(entry);
                        }
                    }
                }

                Event::Eof => break,
                _ => {}
            }
            event_buf.clear();
        }

        Ok(NavigationDocument {
            toc: toc_entries,
            page_list,
            landmarks: Vec::new(), // NCX does not support landmarks
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    type TestArchive = EpubArchive<crate::provider::ZipProvider<std::io::Cursor<Vec<u8>>>>;

    // ── NCX tests ─────────────────────────────────────────────────────────────

    #[test]
    fn test_parse_ncx_toc() {
        let xml = r#"
        <?xml version="1.0" encoding="UTF-8"?>
        <ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
          <navMap>
            <navPoint id="navPoint-1" playOrder="1">
              <navLabel><text>Chapter 1</text></navLabel>
              <content src="ch1.xhtml"/>
              <navPoint id="navPoint-2" playOrder="2">
                <navLabel><text>Chapter 1.1</text></navLabel>
                <content src="ch1_1.xhtml"/>
              </navPoint>
            </navPoint>
            <navPoint id="navPoint-3" playOrder="3">
              <navLabel><text>Chapter 2</text></navLabel>
              <content src="ch2.xhtml"/>
            </navPoint>
          </navMap>
        </ncx>
        "#;

        let nav = TestArchive::parse_ncx_all(xml).unwrap();

        assert_eq!(nav.toc.len(), 2);
        assert!(nav.page_list.is_empty());
        assert!(nav.landmarks.is_empty());

        assert_eq!(nav.toc[0].title, "Chapter 1");
        assert_eq!(nav.toc[0].href, "ch1.xhtml");
        assert_eq!(nav.toc[0].children.len(), 1);
        assert_eq!(nav.toc[0].children[0].title, "Chapter 1.1");
        assert_eq!(nav.toc[0].children[0].href, "ch1_1.xhtml");
        assert_eq!(nav.toc[1].title, "Chapter 2");
        assert_eq!(nav.toc[1].href, "ch2.xhtml");
    }

    #[test]
    fn test_parse_ncx_page_list() {
        let xml = r#"
        <?xml version="1.0" encoding="UTF-8"?>
        <ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
          <navMap>
            <navPoint id="np1" playOrder="1">
              <navLabel><text>Chapter 1</text></navLabel>
              <content src="ch1.xhtml"/>
            </navPoint>
          </navMap>
          <pageList>
            <pageTarget type="normal" playOrder="1">
              <navLabel><text>1</text></navLabel>
              <content src="ch1.xhtml#p1"/>
            </pageTarget>
            <pageTarget type="normal" playOrder="2">
              <navLabel><text>42</text></navLabel>
              <content src="ch3.xhtml#p42"/>
            </pageTarget>
            <pageTarget type="front" playOrder="3">
              <navLabel><text>xii</text></navLabel>
              <content src="front.xhtml#pxii"/>
            </pageTarget>
          </pageList>
        </ncx>
        "#;

        let nav = TestArchive::parse_ncx_all(xml).unwrap();

        assert_eq!(nav.toc.len(), 1);
        assert_eq!(nav.toc[0].title, "Chapter 1");
        assert_eq!(nav.page_list.len(), 3);
        assert_eq!(nav.page_list[0].title, "1");
        assert_eq!(nav.page_list[0].href, "ch1.xhtml#p1");
        assert!(nav.page_list[0].children.is_empty());
        assert_eq!(nav.page_list[1].title, "42");
        assert_eq!(nav.page_list[1].href, "ch3.xhtml#p42");
        assert_eq!(nav.page_list[2].title, "xii");
        assert_eq!(nav.page_list[2].href, "front.xhtml#pxii");
    }

    #[test]
    fn test_parse_ncx_page_list_requires_both_title_and_href() {
        let xml = r#"
        <?xml version="1.0" encoding="UTF-8"?>
        <ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
          <navMap/>
          <pageList>
            <pageTarget type="normal">
              <navLabel><text>1</text></navLabel>
              <!-- no content src -->
            </pageTarget>
            <pageTarget type="normal">
              <!-- no navLabel -->
              <content src="ch1.xhtml#p2"/>
            </pageTarget>
            <pageTarget type="normal">
              <navLabel><text>3</text></navLabel>
              <content src="ch1.xhtml#p3"/>
            </pageTarget>
          </pageList>
        </ncx>
        "#;

        let nav = TestArchive::parse_ncx_all(xml).unwrap();
        assert_eq!(nav.page_list.len(), 1);
        assert_eq!(nav.page_list[0].title, "3");
    }

    // ── Nav XHTML tests ───────────────────────────────────────────────────────

    #[test]
    fn test_parse_nav_xhtml_toc_only() {
        let html = r#"<!DOCTYPE html>
        <html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
        <body>
          <nav epub:type="toc">
            <ol>
              <li><a href="ch1.xhtml">Chapter 1</a></li>
              <li><a href="ch2.xhtml">Chapter 2</a>
                <ol><li><a href="ch2s1.xhtml">Section 2.1</a></li></ol>
              </li>
            </ol>
          </nav>
        </body>
        </html>"#;

        let nav = TestArchive::parse_nav_xhtml_all(html).unwrap();
        assert_eq!(nav.toc.len(), 2);
        assert!(nav.page_list.is_empty());
        assert!(nav.landmarks.is_empty());
        assert_eq!(nav.toc[0].title, "Chapter 1");
        assert_eq!(nav.toc[0].href, "ch1.xhtml");
        assert_eq!(nav.toc[1].title, "Chapter 2");
        assert_eq!(nav.toc[1].children.len(), 1);
        assert_eq!(nav.toc[1].children[0].title, "Section 2.1");
    }

    #[test]
    fn test_parse_nav_xhtml_toc_and_page_list() {
        let html = r#"<!DOCTYPE html>
        <html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
        <body>
          <nav epub:type="toc">
            <ol><li><a href="ch1.xhtml">Chapter 1</a></li></ol>
          </nav>
          <nav epub:type="page-list">
            <ol>
              <li><a href="ch1.xhtml#p1">1</a></li>
              <li><a href="ch1.xhtml#p2">2</a></li>
              <li><a href="ch2.xhtml#p42">42</a></li>
            </ol>
          </nav>
        </body>
        </html>"#;

        let nav = TestArchive::parse_nav_xhtml_all(html).unwrap();
        assert_eq!(nav.toc.len(), 1);
        assert_eq!(nav.toc[0].title, "Chapter 1");
        assert_eq!(nav.page_list.len(), 3);
        assert_eq!(nav.page_list[0].title, "1");
        assert_eq!(nav.page_list[0].href, "ch1.xhtml#p1");
        assert!(nav.page_list[0].children.is_empty());
        assert_eq!(nav.page_list[2].title, "42");
        assert_eq!(nav.page_list[2].href, "ch2.xhtml#p42");
    }

    #[test]
    fn test_parse_nav_xhtml_landmarks() {
        let html = r#"<!DOCTYPE html>
        <html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
        <body>
          <nav epub:type="toc"><ol><li><a href="ch1.xhtml">Ch1</a></li></ol></nav>
          <nav epub:type="landmarks">
            <ol>
              <li><a href="cover.xhtml" epub:type="cover">Cover</a></li>
              <li><a href="toc.xhtml" epub:type="toc">Table of Contents</a></li>
              <li><a href="ch1.xhtml" epub:type="bodymatter">Begin Reading</a></li>
            </ol>
          </nav>
        </body>
        </html>"#;

        let nav = TestArchive::parse_nav_xhtml_all(html).unwrap();
        assert_eq!(nav.toc.len(), 1);
        assert!(nav.page_list.is_empty());
        assert_eq!(nav.landmarks.len(), 3);
        assert_eq!(nav.landmarks[0].title, "Cover");
        assert_eq!(nav.landmarks[0].href, "cover.xhtml");
        assert_eq!(nav.landmarks[2].title, "Begin Reading");
    }

    #[test]
    fn test_parse_nav_xhtml_all_three_types() {
        let html = r#"<!DOCTYPE html>
        <html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
        <body>
          <nav epub:type="toc">
            <ol><li><a href="ch1.xhtml">Chapter 1</a></li></ol>
          </nav>
          <nav epub:type="page-list">
            <ol><li><a href="ch1.xhtml#p1">1</a></li></ol>
          </nav>
          <nav epub:type="landmarks">
            <ol><li><a href="ch1.xhtml">Begin Reading</a></li></ol>
          </nav>
        </body>
        </html>"#;

        let nav = TestArchive::parse_nav_xhtml_all(html).unwrap();
        assert_eq!(nav.toc.len(), 1);
        assert_eq!(nav.page_list.len(), 1);
        assert_eq!(nav.landmarks.len(), 1);
        assert!(!nav.is_empty());
    }

    #[test]
    fn test_parse_nav_xhtml_fallback_no_epub_type() {
        let html = r#"<!DOCTYPE html>
        <html>
        <body>
          <nav>
            <ol><li><a href="ch1.xhtml">Chapter 1</a></li></ol>
          </nav>
        </body>
        </html>"#;

        let nav = TestArchive::parse_nav_xhtml_all(html).unwrap();
        assert_eq!(nav.toc.len(), 1);
        assert_eq!(nav.toc[0].title, "Chapter 1");
        assert!(nav.page_list.is_empty());
    }

    #[test]
    fn test_navigation_document_is_empty() {
        let empty = NavigationDocument::default();
        assert!(empty.is_empty());

        let mut nav = NavigationDocument::default();
        nav.toc.push(TocEntry::new("Ch1", "ch1.xhtml"));
        assert!(!nav.is_empty());
    }
}
