use std::fmt;

/// Whether a CFI step targets an element node or a text node.
///
/// From epub.js `parseStep`: even CFI indices = element, odd = text.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    /// The step refers to an element child (even CFI index).
    /// Traverse using `parent.children[index]` (HTMLCollection, elements only).
    Element,
    /// The step refers to a text node (odd CFI index).
    /// Traverse using `textNodes(parent)[index]` as in epub.js.
    Text,
}

/// A single decoded CFI step, ready for JS-side `walkToNode` execution.
///
/// Mirrors epub.js's internal step object `{type, index, id}` produced by `parseStep()`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResolvedStep {
    /// The DOM node kind to look up.
    pub node_type: NodeType,

    /// 0-based index within the appropriate sibling collection.
    ///
    /// For `Element`: index into `parent.children` (element siblings only).
    /// Formula: `(cfi_step / 2) - 1`
    ///
    /// For `Text`: index into `textNodes(parent)` (text siblings only).
    /// Formula: `(cfi_step - 1) / 2`
    pub index: usize,

    /// Optional element `id` from the CFI assertion `[id]`.
    /// When present, the JS side should prefer `getElementById(id)` for O(1) lookup.
    pub id: Option<String>,
}

/// A fully resolved CFI location descriptor for one endpoint (start or end).
///
/// Contains three complementary resolution strategies, ordered from fastest to most robust:
/// 1. `id_shortcut` — `getElementById()` O(1) fast path (when available)
/// 2. `xpath`       — `doc.evaluate(xpath)` semantically exact, element-only counting
/// 3. `steps`       — JS `walkToNode` using `children[index]` / `textNodes[index]`
///
/// The `css_selector` intentionally omitted: `*:nth-child(N)` counts all sibling nodes
/// (including text nodes), making it semantically incompatible with CFI's element-only
/// index. See epub.js issue #561.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CfiResolved {
    /// 0-based spine index extracted from the CFI base path (`/6/N[id]`).
    /// Use this to determine which spine document to load before resolving.
    pub spine_index: usize,

    /// Decoded steps for the local path (after `!`).
    /// Feed directly to a JS `walkToNode` implementation for precise resolution.
    /// This is the most semantically faithful representation of the CFI.
    pub steps: Vec<ResolvedStep>,

    /// Character offset within the final text node (from CFI `:N` terminal).
    /// Corresponds to epub.js `terminal.offset`. Use with `Range.setStart/setEnd`.
    pub character_offset: Option<u32>,

    /// Whether the last step targets a text node (odd CFI index).
    /// When `true`, apply `character_offset` to the resolved text node.
    pub is_text_node: bool,

    /// XPath expression (semantically correct: `*[N]` counts element siblings only).
    /// Use with `document.evaluate()` as the primary resolution strategy.
    /// Equivalent to epub.js `stepsToXpath(steps)`.
    pub xpath: String,

    /// Fully namespace-agnostic XPath expression (using `*[local-name()]` to ignore
    /// any default XML/XHTML namespaces like `xmlns="http://www.w3.org/1999/xhtml"`).
    /// Safe for JSDOM or environments without a registered namespace resolver.
    pub xpath_ns_agnostic: String,

    /// The `id` attribute of the **deepest** element step that carries an assertion.
    /// Enables an O(1) `getElementById()` shortcut.
    /// When set, JS can jump directly to this element and skip earlier steps.
    pub id_shortcut: Option<String>,

    /// Side bias from the CFI terminal `:before` / `:after`.
    /// `None` means the CFI has no explicit side annotation (spec default is "before").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<CfiSide>,

    /// Temporal position in seconds, from the `~N` terminus (CFI spec §3.1.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporal_offset: Option<f64>,

    /// Spatial position within an image or video, from the `@x:y` terminus (CFI spec §3.1.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spatial_offset: Option<SpatialOffset>,
}

/// The complete result of resolving a CFI string, covering both Point and Range forms.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CfiResolution {
    /// Resolution for the start point (always present).
    pub start: CfiResolved,
    /// Resolution for the end point; `Some` only for Range CFIs.
    pub end: Option<CfiResolved>,
}

/// A 2D spatial position within an image or video (EPUB CFI spec §3.1.6).
///
/// Coordinates are percentages in `[0, 100]` where `0:0` = upper-left,
/// `100:100` = lower-right.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpatialOffset {
    /// Horizontal position, 0 = left, 100 = right.
    pub x: f64,
    /// Vertical position, 0 = top, 100 = bottom.
    /// Sorting rule §3.2 Rule 5: y is more significant than x.
    pub y: f64,
}

/// Represents a single step in a Canonical Fragment Identifier path.
#[derive(Debug, Clone)]
pub struct CfiStep {
    /// The numeric index of the child element (even numbers represent elements, odd numbers represent text/character data)
    pub index: u32,
    /// An optional element ID assertion for robustness (e.g. `[chap01ref]`)
    pub assertion: Option<String>,
}

impl CfiStep {
    pub fn new(index: u32, assertion: Option<String>) -> Self {
        Self { index, assertion }
    }

    /// Returns `true` if this step refers to a text node (odd CFI index).
    ///
    /// From epub.js `parseStep`: odd numbers represent text/character data.
    pub fn is_text_node(&self) -> bool {
        !self.index.is_multiple_of(2)
    }

    /// Returns the 0-based child index within the appropriate sibling collection.
    ///
    /// Mirrors epub.js `parseStep` formulas exactly:
    /// - element (even): `index = cfi_step / 2 - 1`  → into `parent.children`
    /// - text    (odd):  `index = (cfi_step - 1) / 2` → into `textNodes(parent)`
    pub fn child_index(&self) -> usize {
        if self.is_text_node() {
            ((self.index - 1) / 2) as usize
        } else {
            // A CFI step of 2 is the first element (index 0).
            // Guard against malformed CFIs where index could be 0.
            (self.index / 2).saturating_sub(1) as usize
        }
    }

    /// Converts this step into a [`ResolvedStep`] ready for JS `walkToNode`.
    pub fn to_resolved_step(&self) -> ResolvedStep {
        ResolvedStep {
            node_type: if self.is_text_node() {
                NodeType::Text
            } else {
                NodeType::Element
            },
            index: self.child_index(),
            id: self.assertion.clone(),
        }
    }
}

// ── Comparison traits — all ignore `assertion` per CFI spec §3.2 Rule 2 ────
//
// The spec requires assertions to be stripped before any comparison.
// Rust also requires: a == b ⟹ hash(a) == hash(b) AND cmp(a,b) == Equal.
// All four traits must agree.

impl PartialEq for CfiStep {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}
impl Eq for CfiStep {}

impl std::hash::Hash for CfiStep {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.index.hash(state); // assertion excluded — consistent with PartialEq
    }
}

impl PartialOrd for CfiStep {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CfiStep {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.index.cmp(&other.index)
    }
}

impl fmt::Display for CfiStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}", self.index)?;
        if let Some(ref assert) = self.assertion {
            write!(f, "[{}]", assert)?;
        }
        Ok(())
    }
}

/// Side bias for a CFI character-offset terminal (`:before` / `:after`).
///
/// From EPUB CFI spec §2.2 terminus grammar:
/// `terminus := ":" ( offset [ side ] | side )`
///
/// This field is intentionally absent from the EPUB 3 implementation in epub.js,
/// which does not parse side bias.  epub-rs stores and round-trips it faithfully.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CfiSide {
    /// The position is before the indexed character (spec default when omitted).
    Before,
    /// The position is after the indexed character.
    After,
}

/// Represents a path sequence in a CFI.
#[derive(Debug, Clone, Default)]
pub struct CfiPath {
    pub steps: Vec<CfiStep>,
    pub local_steps: Option<Vec<CfiStep>>,
    pub character_offset: Option<u32>,
    /// Side bias from the CFI terminal `:before` / `:after`.
    /// `None` means the CFI string had no explicit side annotation.
    pub side: Option<CfiSide>,
    /// Temporal offset in seconds, from the `~N` terminus (CFI spec §3.1.5).
    pub temporal_offset: Option<f64>,
    /// Spatial offset within an image or video, from the `@x:y` terminus (CFI spec §3.1.6).
    pub spatial_offset: Option<SpatialOffset>,
}

// CfiPath equality is field-by-field. f64 uses IEEE PartialEq (NaN≠NaN),
// which is safe because validly-parsed CFIs never contain NaN.
impl PartialEq for CfiPath {
    fn eq(&self, other: &Self) -> bool {
        self.steps == other.steps
            && self.local_steps == other.local_steps
            && self.character_offset == other.character_offset
            && self.side == other.side
            && self.temporal_offset == other.temporal_offset
            && self.spatial_offset == other.spatial_offset
    }
}
impl Eq for CfiPath {}

/// CfiPaths are ordered step-by-step per EPUB CFI spec §3.2 Sorting Rules.
impl PartialOrd for CfiPath {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CfiPath {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering::*;
        // Rule 3: steps that come earlier in the sequence are more important.
        // 1. Compare base steps numerically
        for (a, b) in self.steps.iter().zip(other.steps.iter()) {
            match a.cmp(b) {
                Equal => continue,
                ord => return ord,
            }
        }
        match self.steps.len().cmp(&other.steps.len()) {
            Equal => {}
            ord => return ord,
        }

        // 2. Compare local steps (path after '!')
        let self_local = self.local_steps.as_deref().unwrap_or(&[]);
        let other_local = other.local_steps.as_deref().unwrap_or(&[]);
        for (a, b) in self_local.iter().zip(other_local.iter()) {
            match a.cmp(b) {
                Equal => continue,
                ord => return ord,
            }
        }
        match self_local.len().cmp(&other_local.len()) {
            Equal => {}
            ord => return ord,
        }

        // 3. Character offsets (Rule 4: natural order)
        match self.character_offset.cmp(&other.character_offset) {
            Equal => {}
            ord => return ord,
        }

        // 4. Temporal offset (Rule 7: omitted < any; Rule 4: natural order).
        // NaN cannot appear: the parser rejects non-finite f64 values.
        match (self.temporal_offset, other.temporal_offset) {
            (None, None) => {}
            (None, Some(_)) => return Less,
            (Some(_), None) => return Greater,
            (Some(a), Some(b)) => match a.partial_cmp(&b).unwrap_or(Equal) {
                Equal => {}
                ord => return ord,
            },
        }

        // 5. Spatial offset (Rule 8: temporal > spatial; Rule 6: omitted < any;
        //    Rule 5: y more significant than x).
        match (&self.spatial_offset, &other.spatial_offset) {
            (None, None) => {}
            (None, Some(_)) => return Less,
            (Some(_), None) => return Greater,
            (Some(a), Some(b)) => {
                match a.y.partial_cmp(&b.y).unwrap_or(Equal) {
                    Equal => {}
                    ord => return ord,
                }
                match a.x.partial_cmp(&b.x).unwrap_or(Equal) {
                    Equal => {}
                    ord => return ord,
                }
            }
        }

        Equal
    }
}

impl fmt::Display for CfiPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for step in &self.steps {
            write!(f, "{}", step)?;
        }
        if let Some(local) = &self.local_steps {
            write!(f, "!")?;
            for step in local {
                write!(f, "{}", step)?;
            }
        }
        if let Some(offset) = self.character_offset {
            write!(f, ":{}", offset)?;
        }
        // Temporal offset: ~N (CFI spec §3.1.5; §2.2 number format).
        if let Some(t) = self.temporal_offset {
            write!(f, "~{}", super::parser::format_cfi_number(t))?;
        }
        // Spatial offset: @x:y (CFI spec §3.1.6).
        if let Some(ref s) = self.spatial_offset {
            write!(
                f,
                "@{}:{}",
                super::parser::format_cfi_number(s.x),
                super::parser::format_cfi_number(s.y)
            )?;
        }
        // Emit :before / :after side-bias annotation when present.
        if let Some(ref side) = self.side {
            match side {
                CfiSide::Before => write!(f, ":before")?,
                CfiSide::After => write!(f, ":after")?,
            }
        }
        Ok(())
    }
}

/// Represents a Canonical Fragment Identifier (CFI), which can be a single point or a range.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[allow(clippy::large_enum_variant)] // Range is intentionally larger: it holds 3 paths
pub enum EpubCfi {
    /// A single point in the EPUB.
    Point(CfiPath),
    /// A range in the EPUB.
    Range {
        parent: CfiPath,
        start: CfiPath,
        end: CfiPath,
    },
}

impl Default for EpubCfi {
    fn default() -> Self {
        Self::new()
    }
}

impl EpubCfi {
    /// Creates a new empty CFI Point.
    pub fn new() -> Self {
        EpubCfi::Point(CfiPath::default())
    }

    /// Add a step to the base path (before '!').
    pub fn add_base_step(mut self, step: CfiStep) -> Self {
        if let EpubCfi::Point(ref mut path) = self {
            path.steps.push(step);
        }
        self
    }

    /// Add a step to the local path (after '!').
    pub fn add_local_step(mut self, step: CfiStep) -> Self {
        if let EpubCfi::Point(ref mut path) = self {
            if path.local_steps.is_none() {
                path.local_steps = Some(Vec::new());
            }
            path.local_steps.as_mut().unwrap().push(step);
        }
        self
    }

    /// Set the final character offset.
    pub fn character_offset(mut self, offset: u32) -> Self {
        if let EpubCfi::Point(ref mut path) = self {
            path.character_offset = Some(offset);
        }
        self
    }
}

impl fmt::Display for EpubCfi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "epubcfi(")?;
        match self {
            EpubCfi::Point(path) => write!(f, "{}", path)?,
            EpubCfi::Range { parent, start, end } => write!(f, "{},{},{}", parent, start, end)?,
        }
        write!(f, ")")
    }
}
