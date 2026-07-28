//! Navigation document parsing: TOC, page-list, and landmarks.
//!
//! Supports both EPUB 3 `nav.xhtml` (all three types in one pass)
//! and EPUB 2 `.ncx` (TOC + page-list in one streaming pass).
//!
//! Mirrors go-toolkit's `parser_navdoc.go` and `parser_ncx.go`.

use crate::error::EpubError;
use crate::model::{EpubBook, NavigationDocument, TocEntry};
use crate::path::resolve_href;
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
    ///
    /// The result is memoized per rendition (cleared by
    /// [`EpubArchive::parse_rendition`]), so repeated TOC / landmarks / title
    /// lookups do not re-parse the navigation file.
    pub fn get_navigation(&mut self, book: &EpubBook) -> Result<NavigationDocument, EpubError> {
        if let Some(cached) = &self.nav_cache {
            return Ok(cached.clone());
        }
        let nav = self.load_navigation(book)?;
        self.nav_cache = Some(nav.clone());
        Ok(nav)
    }

    /// Read + parse the navigation document (no cache; see [`Self::get_navigation`]).
    fn load_navigation(&mut self, book: &EpubBook) -> Result<NavigationDocument, EpubError> {
        // nav.xhtml supersedes the NCX in EPUB 3: it expresses all three navigation
        // types (toc, page-list, landmarks) in a single HTML file.  Using it first
        // avoids loading the NCX at all for the vast majority of modern EPUBs.
        if let Some(nav_item) = book
            .manifest
            .values()
            .find(|i| i.properties.iter().any(|p| p == "nav"))
        {
            // The nav item's href is relative to the OPF directory.
            // Build the EPUB-root-relative path so we can derive the nav document's
            // own directory — hrefs inside nav.xhtml are resolved relative to *it*,
            // not relative to the OPF or the EPUB root (RFC 3986 §5.2).
            let nav_root_href = if book.opf_dir.is_empty() {
                nav_item.href.clone()
            } else {
                format!("{}/{}", book.opf_dir, nav_item.href)
            };
            let nav_dir = match nav_root_href.rfind('/') {
                Some(i) => nav_root_href[..i].to_string(),
                None => String::new(), // nav.xhtml is at the EPUB root
            };

            let bytes = self.get_resource_by_id(book, &nav_item.id)?;
            let html = String::from_utf8_lossy(&bytes).to_string();
            return Self::parse_nav_xhtml_all(&html, &nav_dir);
        }

        // EPUB 2 publications (and some EPUB 3 publications that omit nav.xhtml for
        // backward compatibility) carry an NCX file instead.  toc_id points to the
        // manifest item referenced by the spine's toc attribute.
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

    /// Extracts all navigation lists from a `nav.xhtml` document in one DOM traversal.
    ///
    /// A single pass is used deliberately: re-parsing the document for each nav type
    /// (toc, page-list, landmarks) would triple the DOM construction cost for a file
    /// that is frequently several hundred kilobytes in real-world EPUBs.
    ///
    /// `nav_dir` is the EPUB-root-relative directory of the nav document itself
    /// (e.g. `"OEBPS"` when the file lives at `"OEBPS/nav.xhtml"`). Hrefs inside
    /// the nav document are relative to that file, not to the OPF or the EPUB root,
    /// so we resolve them here to produce consistent EPUB-root-relative paths for
    /// all callers regardless of where the nav file is stored.
    pub(super) fn parse_nav_xhtml_all(
        html: &str,
        nav_dir: &str,
    ) -> Result<NavigationDocument, EpubError> {
        let document = kuchikiki::parse_html().one(html);
        let mut nav_doc = NavigationDocument::default();

        let nav_nodes = document
            .select("nav")
            .unwrap_or_else(|_| panic!("'nav' is a valid CSS selector"));

        for nav in nav_nodes {
            let attrs = nav.attributes.borrow();
            let epub_type = attrs.get("epub:type").unwrap_or("").to_string();
            drop(attrs);

            if epub_type.is_empty() {
                continue;
            }

            let entries = match nav.as_node().select_first("ol") {
                Ok(ol) => Self::parse_ol_node(ol.as_node(), nav_dir),
                Err(_) => continue,
            };

            if entries.is_empty() {
                continue;
            }

            for token in epub_type.split_whitespace() {
                let key = token.trim_start_matches("epub:");
                match key {
                    "toc" => nav_doc.toc = entries.clone(),
                    "page-list" => nav_doc.page_list = entries.clone(),
                    "landmarks" => nav_doc.landmarks = entries.clone(),
                    _ => {}
                }
            }
        }

        // epub:type is required by the EPUB 3 spec but commonly omitted by older
        // authoring tools. Fall back gracefully so the TOC is never silently lost.
        if nav_doc.toc.is_empty() {
            let toc_node = document
                .select_first("nav#toc")
                .or_else(|_| document.select_first("nav"));
            if let Ok(nav) = toc_node
                && let Ok(ol) = nav.as_node().select_first("ol")
            {
                nav_doc.toc = Self::parse_ol_node(ol.as_node(), nav_dir);
            }
        }

        Ok(nav_doc)
    }

    /// Parse an `<ol>` element into a flat or nested list of [`TocEntry`] items.
    ///
    /// `nav_dir` is the EPUB-root-relative directory of the nav document; it is used
    /// to resolve relative `href` values into EPUB-root-relative paths (RFC 3986 §5.2).
    pub(super) fn parse_ol_node(ol: &kuchikiki::NodeRef, nav_dir: &str) -> Vec<TocEntry> {
        let mut entries = Vec::new();
        // kuchikiki's `select("li")` descends into nested <ol> elements, which would
        // return grandchild <li> items alongside top-level ones.  We iterate direct
        // children only and recurse explicitly to maintain the correct nesting depth.
        for li in ol.children().filter(|c| {
            c.as_element()
                .is_some_and(|e| e.name.local.as_ref() == "li")
        }) {
            if let Ok(a_node) = li.select_first("a") {
                let attrs = a_node.attributes.borrow();
                let raw_href = attrs.get("href").unwrap_or("").to_string();
                // Landmark anchors often carry epub:type="bodymatter" | "cover" | …
                let role = attrs
                    .get("epub:type")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                drop(attrs);

                // EPUB hrefs may be percent-encoded in authoring tools; decode before
                // any path arithmetic so "..%2F" is never treated as a literal segment.
                let decoded = percent_encoding::percent_decode_str(&raw_href)
                    .decode_utf8_lossy()
                    .into_owned();

                // Shared path policy: root-relative join, fragment preserved, externals as-is.
                let href = resolve_href(nav_dir, &decoded);

                let title = a_node.text_contents().trim().to_string();

                let mut entry = TocEntry::new(title, href);
                entry.role = role;

                // Nesting is meaningful only for TOC hierarchies; page-list and landmarks
                // are flat by spec, so if they carry a nested <ol> it is ignored here.
                if let Ok(nested_ol) = li.select_first("ol") {
                    entry.children = Self::parse_ol_node(nested_ol.as_node(), nav_dir);
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

                Event::Text(e) if in_text => {
                    let text = String::from_utf8_lossy(&e).into_owned();
                    if in_page_list {
                        if let Some(ref mut pt) = page_target {
                            pt.title = text;
                        }
                    } else if let Some(state) = stack.last_mut() {
                        state.title = text;
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
                        if let Some(pt) = page_target.take()
                            && !pt.title.is_empty()
                            && !pt.href.is_empty()
                        {
                            page_list.push(TocEntry {
                                title: pt.title,
                                href: pt.href,
                                children: Vec::new(),
                                role: None,
                            });
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
                            role: None,
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

        let nav = TestArchive::parse_nav_xhtml_all(html, "").unwrap();
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

        let nav = TestArchive::parse_nav_xhtml_all(html, "").unwrap();
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

        let nav = TestArchive::parse_nav_xhtml_all(html, "").unwrap();
        assert_eq!(nav.toc.len(), 1);
        assert!(nav.page_list.is_empty());
        assert_eq!(nav.landmarks.len(), 3);
        assert_eq!(nav.landmarks[0].title, "Cover");
        assert_eq!(nav.landmarks[0].href, "cover.xhtml");
        assert_eq!(nav.landmarks[0].role.as_deref(), Some("cover"));
        assert_eq!(nav.landmarks[2].title, "Begin Reading");
        assert_eq!(nav.landmarks[2].role.as_deref(), Some("bodymatter"));
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

        let nav = TestArchive::parse_nav_xhtml_all(html, "").unwrap();
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

        let nav = TestArchive::parse_nav_xhtml_all(html, "").unwrap();
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

    // ── P0-2: nav href relative-path resolution ───────────────────────────────

    #[test]
    fn test_nav_xhtml_href_resolved_from_subdir() {
        // nav.xhtml lives at OEBPS/nav/nav.xhtml; hrefs relative to OEBPS/nav/
        // so "../text/ch1.xhtml" must resolve to "OEBPS/text/ch1.xhtml".
        let html = r#"<!DOCTYPE html>
        <html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
        <body>
          <nav epub:type="toc">
            <ol>
              <li><a href="../text/ch1.xhtml">Chapter 1</a></li>
              <li><a href="../text/ch2.xhtml#section-2">Chapter 2 §2</a></li>
            </ol>
          </nav>
        </body>
        </html>"#;

        let nav = TestArchive::parse_nav_xhtml_all(html, "OEBPS/nav").unwrap();
        assert_eq!(nav.toc.len(), 2);
        assert_eq!(nav.toc[0].href, "OEBPS/text/ch1.xhtml");
        // Fragment must be preserved after path resolution
        assert_eq!(nav.toc[1].href, "OEBPS/text/ch2.xhtml#section-2");
    }

    #[test]
    fn test_nav_xhtml_href_root_nav_unchanged() {
        // nav_dir = "" means nav is at EPUB root; simple paths pass through as-is.
        let html = r#"<!DOCTYPE html>
        <html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
        <body>
          <nav epub:type="toc">
            <ol><li><a href="text/ch1.xhtml">Ch1</a></li></ol>
          </nav>
        </body>
        </html>"#;

        let nav = TestArchive::parse_nav_xhtml_all(html, "").unwrap();
        assert_eq!(
            nav.toc[0].href, "text/ch1.xhtml",
            "root-level nav: href must not be modified"
        );
    }

    #[test]
    fn test_nav_xhtml_fragment_only_href_unchanged() {
        let html = r##"<!DOCTYPE html>
        <html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
        <body>
          <nav epub:type="toc">
            <ol><li><a href="#intro">Intro</a></li></ol>
          </nav>
        </body>
        </html>"##;

        let nav = TestArchive::parse_nav_xhtml_all(html, "OEBPS/nav").unwrap();
        assert_eq!(
            nav.toc[0].href, "#intro",
            "fragment-only href must pass through unchanged"
        );
    }

    // resolve_href unit coverage lives in `crate::path` (shared policy module).
}
