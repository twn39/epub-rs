/// Represents a table of contents entry.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct TocEntry {
    pub title: String,
    pub href: String,
    pub children: Vec<TocEntry>,
}

impl TocEntry {
    pub fn new(title: impl Into<String>, href: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            href: href.into(),
            children: Vec::new(),
        }
    }

    pub fn add_child(mut self, child: TocEntry) -> Self {
        self.children.push(child);
        self
    }
}

/// The complete navigation information extracted from a single `nav.xhtml` or `.ncx` file,
/// parsed in one I/O + one parse operation.
///
/// All fields share the [`TocEntry`] type (mirroring go-toolkit's unified `Link`).
/// TOC entries and page-list entries are structurally identical:
/// - `title` = chapter name (TOC) **or** page label (page-list: `"42"`, `"xii"`, `"A-3"`)
/// - `href`  = spine document path, optionally with a fragment anchor (`"ch3.xhtml#p42"`)
/// - `children` = nested entries (TOC only; always empty for page-list / landmarks)
///
/// Mirrors go-toolkit's `ParseNavDoc` / `ParseNCX` which both return
/// `map[string]manifest.LinkList` with keys `"toc"`, `"page-list"`, `"landmarks"`, etc.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct NavigationDocument {
    /// Table of Contents — `epub:type="toc"` or NCX `<navMap>`.
    pub toc: Vec<TocEntry>,

    /// Page List — `epub:type="page-list"` or NCX `<pageList>/<pageTarget>`.
    ///
    /// Each entry: `title` = page label, `href` = document position with fragment.
    /// Entries are always flat (no children).
    pub page_list: Vec<TocEntry>,

    /// Landmarks — `epub:type="landmarks"` (EPUB 3 only; empty for EPUB 2 NCX).
    ///
    /// Structural navigation points such as "Begin Reading", "Table of Contents", "Index".
    pub landmarks: Vec<TocEntry>,
}

impl NavigationDocument {
    /// Returns `true` if all navigation lists are empty.
    pub fn is_empty(&self) -> bool {
        self.toc.is_empty() && self.page_list.is_empty() && self.landmarks.is_empty()
    }
}
