//! EPUB Canonical Fragment Identifier (CFI) implementation.
//!
//! CFI allows pinpointing a specific location within an EPUB document without modifying the underlying files.
//! Format Example: `epubcfi(/6/4[chap01ref]!/4[body01]/10[para05]/2/1:3)`
//! Range Example: `epubcfi(/6/4[chap01ref]!/4[body01]/10[para05],/2/1:1,/3:4)`

use crate::error::EpubError;
use std::fmt;

// ─────────────────────────────────────────────────────────────────────────────
// CFI Resolution types  (CFI → structured DOM descriptors)
// ─────────────────────────────────────────────────────────────────────────────

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

    /// The `id` attribute of the **deepest** element step that carries an assertion.
    /// Enables an O(1) `getElementById()` shortcut.
    /// When set, JS can jump directly to this element and skip earlier steps.
    pub id_shortcut: Option<String>,

    /// Side bias from the CFI terminal `:before` / `:after`.
    /// `None` means the CFI has no explicit side annotation (spec default is "before").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<CfiSide>,
}

/// The complete result of resolving a CFI string, covering both Point and Range forms.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CfiResolution {
    /// Resolution for the start point (always present).
    pub start: CfiResolved,
    /// Resolution for the end point; `Some` only for Range CFIs.
    pub end: Option<CfiResolved>,
}

/// Represents a single step in a Canonical Fragment Identifier path.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// CFI steps compare only by their numeric index, as per the EPUB CFI spec.
/// Assertions are informational and do not affect ordering.
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
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CfiPath {
    pub steps: Vec<CfiStep>,
    pub local_steps: Option<Vec<CfiStep>>,
    pub character_offset: Option<u32>,
    /// Side bias from the CFI terminal `:before` / `:after`.
    /// `None` means the CFI string had no explicit side annotation.
    pub side: Option<CfiSide>,
}

/// CfiPaths are compared step-by-step numerically (base steps, then local steps, then offset).
impl PartialOrd for CfiPath {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CfiPath {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // 1. Compare base steps numerically
        for (a, b) in self.steps.iter().zip(other.steps.iter()) {
            match a.cmp(b) {
                std::cmp::Ordering::Equal => continue,
                ord => return ord,
            }
        }
        match self.steps.len().cmp(&other.steps.len()) {
            std::cmp::Ordering::Equal => {}
            ord => return ord,
        }

        // 2. Compare local steps (path after '!')
        let self_local = self.local_steps.as_deref().unwrap_or(&[]);
        let other_local = other.local_steps.as_deref().unwrap_or(&[]);
        for (a, b) in self_local.iter().zip(other_local.iter()) {
            match a.cmp(b) {
                std::cmp::Ordering::Equal => continue,
                ord => return ord,
            }
        }
        match self_local.len().cmp(&other_local.len()) {
            std::cmp::Ordering::Equal => {}
            ord => return ord,
        }

        // 3. Compare character offsets
        self.character_offset.cmp(&other.character_offset)
    }
}

impl CfiPath {
    /// Resolves the **local path** (steps after `!`) into a [`CfiResolved`] descriptor.
    ///
    /// Produces three complementary resolution strategies:
    /// - `steps`       — decoded steps for JS `walkToNode` (semantically exact)
    /// - `xpath`       — XPath string for `doc.evaluate()` (semantically exact)
    /// - `id_shortcut` — deepest element id for O(1) `getElementById()`
    ///
    /// The intentionally omitted CSS `nth-child` selector is semantically incompatible
    /// with CFI because `nth-child(N)` counts all sibling nodes (including text nodes),
    /// while CFI indices count element siblings only via `parent.children`.
    /// See epub.js issue #561 and `walkToNode` for the authoritative approach.
    ///
    /// Returns `None` if there are no local steps (CFI without `!`).
    pub fn resolve(&self, spine_index: usize) -> Option<CfiResolved> {
        let steps = self.local_steps.as_deref()?;
        if steps.is_empty() {
            return None;
        }

        let mut resolved_steps: Vec<ResolvedStep> = Vec::with_capacity(steps.len());
        let mut id_shortcut: Option<String> = None;

        // ── XPath (epub.js stepsToXpath, verbatim) ───────────────────────────
        // Start with [".", "*"] matching epub.js's initial xpath array.
        // *[N] and text()[N] both use 1-based positions.
        // *[N] counts ELEMENT siblings only — correct for CFI semantics.
        let mut xpath_parts: Vec<String> = vec![".".into(), "*".into()];

        for step in steps {
            let pos = step.child_index() + 1; // 1-based for XPath

            if step.is_text_node() {
                // text()[N] — XPath can address text nodes directly
                xpath_parts.push(format!("text()[{pos}]"));
                // text steps carry no id; stop XPath building here
            } else {
                if let Some(ref id) = step.assertion {
                    // epub.js: "*[position()=N and @id='id']"
                    xpath_parts.push(format!("*[position()={pos} and @id='{id}']"));
                    // Track the deepest id for the getElementById shortcut
                    id_shortcut = Some(id.clone());
                } else {
                    xpath_parts.push(format!("*[{pos}]"));
                }
            }

            resolved_steps.push(step.to_resolved_step());
        }

        let is_text_node = steps.last().map(|s| s.is_text_node()).unwrap_or(false);

        Some(CfiResolved {
            spine_index,
            steps: resolved_steps,
            character_offset: self.character_offset,
            is_text_node,
            xpath: xpath_parts.join("/"),
            id_shortcut,
            side: self.side.clone(),
        })
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

    /// Generates a standard base CFI path for a spine item.
    ///
    /// In EPUB CFI, `/6` refers to the `<spine>` element (3rd child of `<package>`),
    /// and itemrefs within it are even-numbered starting from 2.
    ///
    /// # Arguments
    /// * `spine_index` - The 0-based index of the item in the spine.
    /// * `item_id` - The OPF manifest ID of the item, used as the assertion.
    pub fn generate_spine_base_cfi(spine_index: usize, item_id: &str) -> String {
        // Spine elements are even-indexed: first item = 2, second = 4, etc.
        let item_cfi_index = (spine_index + 1) * 2;
        format!("/6/{}[{}]!", item_cfi_index, item_id)
    }

    /// Generates a spec-compliant CFI range string from two Point CFIs.
    ///
    /// The output format is `epubcfi(shared_path,start_local,end_local)` where `shared_path`
    /// is the longest common ancestor path of both input CFIs.
    ///
    /// # Errors
    /// Returns `EpubError::InvalidFormat` if either input is a Range CFI (not a Point).
    pub fn generate_range(start: &EpubCfi, end: &EpubCfi) -> Result<String, EpubError> {
        let (s, e) = match (start, end) {
            (EpubCfi::Point(s), EpubCfi::Point(e)) => (s, e),
            _ => {
                return Err(EpubError::InvalidFormat(
                    "generate_range requires two Point CFIs, not Range CFIs".to_string(),
                ));
            }
        };

        // Find the length of the common base path (steps before '!')
        let common_base_len = s
            .steps
            .iter()
            .zip(e.steps.iter())
            .take_while(|(a, b)| a.index == b.index)
            .count();

        let shared_base: String = s.steps[..common_base_len]
            .iter()
            .map(|step| step.to_string())
            .collect();

        if common_base_len == s.steps.len() && common_base_len == e.steps.len() {
            // Both CFIs share the same base path (same document). Parent includes the `!`.
            // Find the common prefix of their local steps.
            let s_local = s.local_steps.as_deref().unwrap_or(&[]);
            let e_local = e.local_steps.as_deref().unwrap_or(&[]);

            let common_local_len = s_local
                .iter()
                .zip(e_local.iter())
                .take_while(|(a, b)| a.index == b.index)
                .count();

            let shared_local: String = s_local[..common_local_len]
                .iter()
                .map(|step| step.to_string())
                .collect();

            // Start relative path: diverging local steps + offset
            let start_rel_steps: String = s_local[common_local_len..]
                .iter()
                .map(|step| step.to_string())
                .collect();
            let start_offset = s
                .character_offset
                .map(|o| format!(":{}", o))
                .unwrap_or_default();
            let start_rel = format!("{}{}", start_rel_steps, start_offset);

            // End relative path: diverging local steps + offset
            let end_rel_steps: String = e_local[common_local_len..]
                .iter()
                .map(|step| step.to_string())
                .collect();
            let end_offset = e
                .character_offset
                .map(|o| format!(":{}", o))
                .unwrap_or_default();
            let end_rel = format!("{}{}", end_rel_steps, end_offset);

            Ok(format!(
                "epubcfi({}!{},{},{})",
                shared_base, shared_local, start_rel, end_rel
            ))
        } else {
            // Cross-document range: shared path ends at the diverging base step.
            // Each side includes its full remaining path.
            let build_full = |path: &CfiPath, after: usize| -> String {
                let remaining_base: String = path.steps[after..]
                    .iter()
                    .map(|step| step.to_string())
                    .collect();
                let local: String = path
                    .local_steps
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|step| step.to_string())
                    .collect();
                let offset = path
                    .character_offset
                    .map(|o| format!(":{}", o))
                    .unwrap_or_default();
                format!("{}!{}{}", remaining_base, local, offset)
            };

            let start_full = build_full(s, common_base_len);
            let end_full = build_full(e, common_base_len);

            Ok(format!(
                "epubcfi({},{},{})",
                shared_base, start_full, end_full
            ))
        }
    }

    /// Extracts the 0-based spine index from a CFI's base path (`/6/N[id]`).
    ///
    /// In the EPUB CFI spec, the base path always starts with `/6` (the `<spine>` element),
    /// followed by `/N[id]` where N is an even integer: `(spine_index + 1) * 2`.
    ///
    /// `steps[1]` (0-based) is the spine item step; `child_index()` recovers the 0-based index.
    fn extract_spine_index(base: &CfiPath) -> usize {
        base.steps
            .get(1) // steps[0] = /6, steps[1] = /N[item_id]
            .map(|s| s.child_index())
            .unwrap_or(0)
    }

    /// Resolves this CFI to structured DOM location descriptor(s).
    ///
    /// Returns a [`CfiResolution`] with:
    /// - `start` — always present, for Point CFIs or the start of a Range
    /// - `end`   — `Some(...)` only for Range CFIs
    ///
    /// Each endpoint carries three resolution strategies (ordered by reliability):
    /// 1. `id_shortcut` — `getElementById(id)` O(1) fast path
    /// 2. `xpath`       — `doc.evaluate(xpath)` element-count-accurate XPath
    /// 3. `steps`       — structured array for JS `walkToNode` (most faithful)
    ///
    /// Returns `None` if the CFI has no local path (i.e., no `!` separator).
    pub fn resolve(&self) -> Option<CfiResolution> {
        match self {
            EpubCfi::Point(path) => {
                let spine_index = Self::extract_spine_index(path);
                let start = path.resolve(spine_index)?;
                Some(CfiResolution { start, end: None })
            }

            EpubCfi::Range { parent, start, end } => {
                let spine_index = Self::extract_spine_index(parent);

                // Combine parent local steps + half-path steps.
                // Mirrors epub.js: startSteps = cfi.path.steps.concat(start.steps)
                let combine = |half: &CfiPath| -> Option<CfiResolved> {
                    let combined_local: Vec<CfiStep> = parent
                        .local_steps
                        .clone()
                        .unwrap_or_default()
                        .into_iter()
                        .chain(half.local_steps.clone().unwrap_or_default())
                        .collect();

                    let combined = CfiPath {
                        steps: parent.steps.clone(),
                        local_steps: Some(combined_local),
                        character_offset: half.character_offset,
                        // The half-path's side bias takes precedence; fall back
                        // to the shared parent's side when the half-path has none.
                        side: half.side.clone().or_else(|| parent.side.clone()),
                    };
                    combined.resolve(spine_index)
                };

                let start_resolved = combine(start)?;
                let end_resolved = combine(end);
                Some(CfiResolution {
                    start: start_resolved,
                    end: end_resolved,
                })
            }
        }
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

/// Splits the inner content of `epubcfi(...)` by top-level commas.
///
/// A "top-level" comma is one that is **not** inside an assertion bracket `[...]`
/// and **not** preceded by the CFI escape character `^`.
///
/// This is necessary because EPUB CFI assertions can legally contain commas
/// (e.g. `[chap,v2]`), which must not be mistaken for the Range separator.
fn split_cfi_top_level(s: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = Vec::new();
    let mut depth = 0u32; // bracket nesting depth
    let mut escaped = false;
    let mut last = 0usize;

    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '^' => escaped = true,
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&s[last..i]);
                last = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[last..]);
    parts
}

impl std::str::FromStr for EpubCfi {
    type Err = EpubError;

    /// Parses a CFI string, supporting both Point and Range formats.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if !s.starts_with("epubcfi(") || !s.ends_with(')') {
            return Err(EpubError::InvalidFormat("Invalid CFI wrapper".to_string()));
        }

        let inner = &s[8..s.len() - 1]; // inside epubcfi(...)
        // Use bracket-aware splitting so commas inside assertion [...] are NOT
        // treated as Range separators (e.g. /6/4[chap,v2]!/4/2,/1:5,/3:10).
        let parts = split_cfi_top_level(inner);

        match parts.len() {
            1 => {
                let path = parse_path(parts[0])?;
                Ok(EpubCfi::Point(path))
            }
            3 => {
                let parent = parse_path(parts[0])?;
                let start = parse_path(parts[1])?;
                let end = parse_path(parts[2])?;
                Ok(EpubCfi::Range { parent, start, end })
            }
            _ => Err(EpubError::InvalidFormat(
                "Invalid CFI range structure".to_string(),
            )),
        }
    }
}

fn parse_path(s: &str) -> Result<CfiPath, EpubError> {
    let mut path = CfiPath::default();
    let parts: Vec<&str> = s.split('!').collect();

    if parts.len() == 1 {
        let (steps, offset, side) = parse_steps_and_offset(parts[0])?;
        path.steps = steps;
        path.character_offset = offset;
        path.side = side;
    } else if parts.len() == 2 {
        // Base path (before '!') carries no terminal offset or side bias.
        let (base_steps, _, _) = parse_steps_and_offset(parts[0])?;
        path.steps = base_steps;

        let (local_steps, offset, side) = parse_steps_and_offset(parts[1])?;
        path.local_steps = Some(local_steps);
        path.character_offset = offset;
        path.side = side;
    } else {
        return Err(EpubError::InvalidFormat(
            "Invalid CFI path structure".to_string(),
        ));
    }

    Ok(path)
}

/// Return type of [`parse_steps_and_offset`]: `(steps, char_offset, side_bias)`.
type ParsedTerminal = (Vec<CfiStep>, Option<u32>, Option<CfiSide>);

/// Scans `s` for the first unescaped `:` that is outside assertion brackets,
/// then splits the terminal string into `(offset, side_bias)`.
///
/// Handles the EPUB CFI terminus grammar:
/// ```text
/// terminus := ":" ( offset [ side ] | side )
/// side     := "before" | "after"
/// ```
///
/// Returns `(steps, character_offset, side_bias)`.  The side bias is `None`
/// when the CFI string has no `:before` / `:after` suffix.
fn parse_steps_and_offset(s: &str) -> Result<ParsedTerminal, EpubError> {
    let mut path_str = s;

    // Find the first unescaped ':' outside assertion brackets — this is the
    // start of the terminal (character-offset + optional side bias).
    let mut in_assertion = false;
    let mut is_escaped = false;
    let mut colon_idx: Option<usize> = None;

    for (i, c) in s.char_indices() {
        if is_escaped {
            is_escaped = false;
        } else if c == '^' {
            is_escaped = true;
        } else if c == '[' {
            in_assertion = true;
        } else if c == ']' {
            in_assertion = false;
        } else if c == ':' && !in_assertion {
            colon_idx = Some(i);
            break;
        }
    }

    if let Some(idx) = colon_idx {
        let terminal_str = &s[idx + 1..];
        path_str = &s[..idx];

        // Check for `:N:before` or `:N:after` (side bias suffix).
        // We use rfind(':') so nested colons inside the offset number (illegal
        // per spec, but defensive) don't cause a false match.
        let (offset_part, side) = if let Some(sep) = terminal_str.rfind(':') {
            match &terminal_str[sep + 1..] {
                "before" => (&terminal_str[..sep], Some(CfiSide::Before)),
                "after" => (&terminal_str[..sep], Some(CfiSide::After)),
                // Not a known side keyword — treat the whole string as offset.
                _ => (terminal_str, None),
            }
        } else {
            (terminal_str, None)
        };

        let offset = offset_part.parse::<u32>().map_err(|_| {
            EpubError::InvalidFormat(format!("Invalid CFI character offset: '{offset_part}'"))
        })?;

        let steps = parse_steps(path_str)?;
        return Ok((steps, Some(offset), side));
    }

    let steps = parse_steps(path_str)?;
    Ok((steps, None, None))
}

fn parse_steps(path: &str) -> Result<Vec<CfiStep>, EpubError> {
    let mut steps = Vec::new();
    if path.is_empty() {
        return Ok(steps);
    }

    let mut chars = path.chars().peekable();

    // Skip leading slash if present
    if chars.peek() == Some(&'/') {
        chars.next();
    }

    while chars.peek().is_some() {
        let mut index_str = String::new();
        let mut assertion_buf = String::new();
        let mut has_assertion = false;
        let mut in_assertion = false;
        let mut is_escaped = false;

        for c in chars.by_ref() {
            if is_escaped {
                if in_assertion {
                    assertion_buf.push(c);
                } else {
                    index_str.push(c);
                }
                is_escaped = false;
            } else if c == '^' {
                is_escaped = true;
            } else if c == '/' && !in_assertion {
                break; // End of this step
            } else if c == '[' && !in_assertion {
                in_assertion = true;
            } else if c == ']' && in_assertion {
                in_assertion = false;
                has_assertion = true;
            } else {
                if in_assertion {
                    assertion_buf.push(c);
                } else {
                    index_str.push(c);
                }
            }
        }

        if index_str.is_empty() && !has_assertion {
            continue;
        }

        let index = index_str
            .parse::<u32>()
            .map_err(|_| EpubError::InvalidFormat(format!("Invalid CFI index: '{}'", index_str)))?;

        let assertion = if has_assertion {
            Some(assertion_buf)
        } else {
            None
        };
        steps.push(CfiStep::new(index, assertion));
    }

    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_spine_base_cfi_format() {
        // Verify format: /6/<even_index>[id]!
        assert_eq!(EpubCfi::generate_spine_base_cfi(0, "ch1"), "/6/2[ch1]!");
        assert_eq!(EpubCfi::generate_spine_base_cfi(1, "ch2"), "/6/4[ch2]!");
        assert_eq!(EpubCfi::generate_spine_base_cfi(4, "ch5"), "/6/10[ch5]!");
    }

    #[test]
    fn test_cfi_step_numeric_ordering() {
        let s2 = CfiStep::new(2, None);
        let s10 = CfiStep::new(10, None);
        // Numeric: 10 > 2, not lexicographic
        assert!(s10 > s2);
        assert!(s2 < s10);
    }

    #[test]
    fn test_cfi_path_ordering() {
        let p4 = CfiPath {
            steps: vec![CfiStep::new(6, None), CfiStep::new(4, None)],
            local_steps: None,
            character_offset: None,
            side: None,
        };
        let p10 = CfiPath {
            steps: vec![CfiStep::new(6, None), CfiStep::new(10, None)],
            local_steps: None,
            character_offset: None,
            side: None,
        };
        assert!(p4 < p10);
    }

    #[test]
    fn test_cfi_point_comparison() {
        let a = EpubCfi::from_str("epubcfi(/6/2!/4/2:5)").unwrap();
        let b = EpubCfi::from_str("epubcfi(/6/2!/4/10:1)").unwrap();
        assert!(a < b);

        let a = EpubCfi::from_str("epubcfi(/6/2!/4/2:5)").unwrap();
        let b = EpubCfi::from_str("epubcfi(/6/4!/4/2:0)").unwrap();
        assert!(a < b); // chapter 2 comes before chapter 4
    }

    #[test]
    fn test_generate_range_same_document() {
        let start = EpubCfi::from_str("epubcfi(/6/4[chap01]!/4/2:5)").unwrap();
        let end = EpubCfi::from_str("epubcfi(/6/4[chap01]!/4/2:10)").unwrap();
        let range = EpubCfi::generate_range(&start, &end).unwrap();
        // Both in same document, same local path → parent = /6/4[chap01]!/4/2
        assert_eq!(range, "epubcfi(/6/4[chap01]!/4/2,:5,:10)");
    }

    #[test]
    fn test_generate_range_cross_document() {
        let start = EpubCfi::from_str("epubcfi(/6/2[ch1]!/4/2:0)").unwrap();
        let end = EpubCfi::from_str("epubcfi(/6/4[ch2]!/4/2:0)").unwrap();
        let range = EpubCfi::generate_range(&start, &end).unwrap();
        // Different base paths → shared = /6
        assert_eq!(range, "epubcfi(/6,/2[ch1]!/4/2:0,/4[ch2]!/4/2:0)");
    }

    #[test]
    fn test_generate_range_requires_points() {
        let range_cfi = EpubCfi::from_str("epubcfi(/6/4,/2:1,/2:5)").unwrap();
        let point = EpubCfi::from_str("epubcfi(/6/4!/4/2:5)").unwrap();
        assert!(EpubCfi::generate_range(&range_cfi, &point).is_err());
    }

    // ── CfiStep helpers ──────────────────────────────────────────────────────

    #[test]
    fn test_is_text_node() {
        // Even CFI index → element
        assert!(!CfiStep::new(2, None).is_text_node());
        assert!(!CfiStep::new(4, None).is_text_node());
        // Odd CFI index → text
        assert!(CfiStep::new(1, None).is_text_node());
        assert!(CfiStep::new(3, None).is_text_node());
    }

    #[test]
    fn test_child_index_element() {
        // epub.js formula: index = cfi_step / 2 - 1
        assert_eq!(CfiStep::new(2, None).child_index(), 0); // first element child
        assert_eq!(CfiStep::new(4, None).child_index(), 1); // second element child
        assert_eq!(CfiStep::new(10, None).child_index(), 4); // fifth element child
    }

    #[test]
    fn test_child_index_text() {
        // epub.js formula: index = (cfi_step - 1) / 2
        assert_eq!(CfiStep::new(1, None).child_index(), 0); // first text node
        assert_eq!(CfiStep::new(3, None).child_index(), 1); // second text node
        assert_eq!(CfiStep::new(5, None).child_index(), 2); // third text node
    }

    #[test]
    fn test_to_resolved_step_element_with_id() {
        let step = CfiStep::new(4, Some("body01".into()));
        let resolved = step.to_resolved_step();
        assert_eq!(resolved.node_type, NodeType::Element);
        assert_eq!(resolved.index, 1);
        assert_eq!(resolved.id, Some("body01".into()));
    }

    #[test]
    fn test_to_resolved_step_text() {
        let step = CfiStep::new(3, None);
        let resolved = step.to_resolved_step();
        assert_eq!(resolved.node_type, NodeType::Text);
        assert_eq!(resolved.index, 1);
        assert_eq!(resolved.id, None);
    }

    // ── CfiPath::resolve ─────────────────────────────────────────────────────

    #[test]
    fn test_resolve_no_local_steps_returns_none() {
        // A base-only CFI (no '!') has no local path → None
        let path = CfiPath {
            steps: vec![CfiStep::new(6, None), CfiStep::new(4, Some("ch1".into()))],
            local_steps: None,
            character_offset: None,
            side: None,
        };
        assert!(path.resolve(0).is_none());
    }

    #[test]
    fn test_resolve_element_steps_xpath() {
        // epubcfi(/6/4[chap01]!/4[body01]/10[para05]/2)
        // local steps: /4[body01] /10[para05] /2
        let path = CfiPath {
            steps: vec![
                CfiStep::new(6, None),
                CfiStep::new(4, Some("chap01".into())),
            ],
            local_steps: Some(vec![
                CfiStep::new(4, Some("body01".into())),
                CfiStep::new(10, Some("para05".into())),
                CfiStep::new(2, None),
            ]),
            character_offset: None,
            side: None,
        };
        let resolved = path.resolve(1).unwrap();

        // XPath: epub.js stepsToXpath verbatim
        assert_eq!(
            resolved.xpath,
            "./*/*[position()=2 and @id='body01']/*[position()=5 and @id='para05']/*[1]"
        );
        // id_shortcut: deepest id (para05, not body01)
        assert_eq!(resolved.id_shortcut, Some("para05".into()));
        assert!(!resolved.is_text_node);
        assert_eq!(resolved.character_offset, None);
        assert_eq!(resolved.spine_index, 1);

        // steps: three decoded steps
        assert_eq!(resolved.steps.len(), 3);
        assert_eq!(resolved.steps[0].node_type, NodeType::Element);
        assert_eq!(resolved.steps[0].index, 1); // step=4 → children[1]
        assert_eq!(resolved.steps[2].node_type, NodeType::Element);
        assert_eq!(resolved.steps[2].index, 0); // step=2 → children[0]
    }

    #[test]
    fn test_resolve_text_step_and_offset() {
        // epubcfi(/6/4[chap01]!/4/2/1:3)
        // local: /4 /2 /1  offset=3
        let path = CfiPath {
            steps: vec![
                CfiStep::new(6, None),
                CfiStep::new(4, Some("chap01".into())),
            ],
            local_steps: Some(vec![
                CfiStep::new(4, None), // element, index=1
                CfiStep::new(2, None), // element, index=0
                CfiStep::new(1, None), // TEXT,    index=0
            ]),
            character_offset: Some(3),
            side: None,
        };
        let resolved = path.resolve(1).unwrap();

        // XPath ends with text()[1]
        assert!(resolved.xpath.ends_with("/text()[1]"));
        // is_text_node must be true
        assert!(resolved.is_text_node);
        // character_offset propagated
        assert_eq!(resolved.character_offset, Some(3));
        // No id anywhere → no shortcut
        assert_eq!(resolved.id_shortcut, None);
        // Last step is text
        assert_eq!(resolved.steps.last().unwrap().node_type, NodeType::Text);
        assert_eq!(resolved.steps.last().unwrap().index, 0);
    }

    #[test]
    fn test_resolve_id_shortcut_deepest_wins() {
        // Two element steps both carrying ids; deepest should be the shortcut
        let path = CfiPath {
            steps: vec![CfiStep::new(6, None), CfiStep::new(4, None)],
            local_steps: Some(vec![
                CfiStep::new(4, Some("section1".into())),
                CfiStep::new(6, Some("para99".into())),
            ]),
            character_offset: None,
            side: None,
        };
        let resolved = path.resolve(0).unwrap();
        assert_eq!(resolved.id_shortcut, Some("para99".into())); // deepest wins
    }

    // ── EpubCfi::resolve ──────────────────────────────────────────────────────

    #[test]
    fn test_epubcfi_resolve_point() {
        // epubcfi(/6/4[chap01]!/4[body01]/10[para05]/2/1:3)
        let cfi = EpubCfi::from_str("epubcfi(/6/4[chap01]!/4[body01]/10[para05]/2/1:3)").unwrap();
        let resolution = cfi.resolve().unwrap();

        assert!(resolution.end.is_none());
        let start = &resolution.start;
        assert_eq!(start.spine_index, 1); // /6/4 → step=4 → child_index=1
        assert_eq!(start.character_offset, Some(3));
        assert!(start.is_text_node); // /1 is odd
        assert_eq!(start.id_shortcut, Some("para05".into()));
    }

    #[test]
    fn test_epubcfi_resolve_spine_index() {
        // spine_index 0 → /6/2, spine_index 2 → /6/6
        let cfi0 = EpubCfi::from_str("epubcfi(/6/2[ch1]!/4/2)").unwrap();
        let cfi2 = EpubCfi::from_str("epubcfi(/6/6[ch3]!/4/2)").unwrap();
        assert_eq!(cfi0.resolve().unwrap().start.spine_index, 0);
        assert_eq!(cfi2.resolve().unwrap().start.spine_index, 2);
    }

    #[test]
    fn test_epubcfi_resolve_range() {
        // epubcfi(/6/4[chap01]!/4/2,/1:5,/3:10)
        // Parse structure:
        //   parent local_steps = [/4, /2]  (elements — the shared ancestor path)
        //   start  steps       = [/1]      (no '!', not local_steps; offset=5)
        //   end    steps       = [/3]      (no '!', not local_steps; offset=10)
        // combine(start): local = parent.local + start.local = [/4,/2] + [] = [/4,/2]
        // → last step is /2 (even = element)
        let cfi = EpubCfi::from_str("epubcfi(/6/4[chap01]!/4/2,/1:5,/3:10)").unwrap();
        let resolution = cfi.resolve().unwrap();

        // Range → both start and end present
        assert!(resolution.end.is_some());
        let start = &resolution.start;
        let end = resolution.end.as_ref().unwrap();

        // character_offset from the half-path (start.character_offset)
        assert_eq!(start.character_offset, Some(5));
        assert_eq!(end.character_offset, Some(10));

        // Last combined step is /2 (even → element, not text)
        assert!(!start.is_text_node);
        assert!(!end.is_text_node);

        // Both endpoints are in the same spine item
        assert_eq!(start.spine_index, end.spine_index);

        // Combined local steps: [/4(elem,idx=1), /2(elem,idx=0)]
        assert_eq!(start.steps.len(), 2);
        assert_eq!(start.steps[0].node_type, NodeType::Element);
        assert_eq!(start.steps[0].index, 1); // step=4 → children[1]
        assert_eq!(start.steps[1].node_type, NodeType::Element);
        assert_eq!(start.steps[1].index, 0); // step=2 → children[0]
    }

    #[test]
    fn test_epubcfi_resolve_range_with_text_steps() {
        // A Range CFI where the shared parent local path ends at a text step.
        // epubcfi(/6/4[chap01]!/4/2/1,/5,:10)
        // parent local = [/4, /2, /1]  → last step /1 is text
        // start  local = [/5]          → after combining: [/4,/2,/1,/5] — odd
        // end    local = []            → combining: [/4,/2,/1] last is text
        //
        // In practice, construct this directly to avoid parser ambiguity:
        let path = CfiPath {
            steps: vec![
                CfiStep::new(6, None),
                CfiStep::new(4, Some("chap01".into())),
            ],
            local_steps: Some(vec![
                CfiStep::new(4, None), // element
                CfiStep::new(2, None), // element
                CfiStep::new(1, None), // TEXT ← last in parent local
            ]),
            character_offset: None,
            side: None,
        };
        // Simulate start half: local = [] + extra text step
        let start_half = CfiPath {
            steps: vec![],
            local_steps: Some(vec![CfiStep::new(3, None)]), // text
            character_offset: Some(7),
            side: None,
        };
        let cfi = EpubCfi::Range {
            parent: path.clone(),
            start: start_half.clone(),
            end: CfiPath {
                steps: vec![],
                local_steps: None,
                character_offset: Some(12),
                side: None,
            },
        };

        let resolution = cfi.resolve().unwrap();
        let start = &resolution.start;
        let end = resolution.end.as_ref().unwrap();

        // start combined: [/4,/2,/1(TEXT),/3(TEXT)] → last is text
        assert!(start.is_text_node);
        assert_eq!(start.character_offset, Some(7));

        // end combined: [/4,/2,/1(TEXT)] + [] → last is /1 (text)
        assert!(end.is_text_node);
        assert_eq!(end.character_offset, Some(12));
    }

    #[test]
    fn test_epubcfi_resolve_no_local_path_returns_none() {
        // A base-only CFI without '!' should return None from resolve()
        let path = CfiPath {
            steps: vec![CfiStep::new(6, None), CfiStep::new(4, None)],
            local_steps: None,
            character_offset: None,
            side: None,
        };
        let cfi = EpubCfi::Point(path);
        assert!(cfi.resolve().is_none());
    }

    // ── Edge-case: assertion-comma must not split Range (P0) ─────────────────

    #[test]
    fn test_cfi_range_assertion_comma_not_split() {
        // [chap,v2] contains a comma inside the assertion — it must NOT be
        // treated as a Range separator.  Before the fix, this parsed as 4 parts
        // and returned an "Invalid CFI range structure" error.
        let cfi = EpubCfi::from_str("epubcfi(/6/4[chap,v2]!/4/2,/1:5,/3:10)").unwrap();
        assert!(
            matches!(cfi, EpubCfi::Range { .. }),
            "Expected Range, got: {cfi:?}"
        );
        if let EpubCfi::Range { parent, .. } = &cfi {
            assert_eq!(
                parent.steps[1].assertion.as_deref(),
                Some("chap,v2"),
                "Assertion must be preserved intact"
            );
        }
    }

    #[test]
    fn test_cfi_range_multiple_commas_in_assertions() {
        // Multiple commas in a single assertion [a,b,c]
        let cfi = EpubCfi::from_str("epubcfi(/6/4[a,b,c]!/4/2,/1:0,/3:5)").unwrap();
        assert!(matches!(cfi, EpubCfi::Range { .. }));
        if let EpubCfi::Range { parent, .. } = &cfi {
            assert_eq!(parent.steps[1].assertion.as_deref(), Some("a,b,c"));
        }
    }

    #[test]
    fn test_cfi_point_assertion_comma_preserved() {
        // Point CFI with comma in assertion: [foo,bar] must survive the parser.
        let cfi = EpubCfi::from_str("epubcfi(/6/4[foo,bar]!/4/2:3)").unwrap();
        if let EpubCfi::Point(path) = &cfi {
            assert_eq!(path.steps[1].assertion.as_deref(), Some("foo,bar"));
        }
    }

    // ── Edge-case: :before / :after side bias (P1) ───────────────────────────

    #[test]
    fn test_cfi_side_before_parsed() {
        // :5:before — before the 5th character
        let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2/1:5:before)").unwrap();
        if let EpubCfi::Point(path) = &cfi {
            assert_eq!(path.character_offset, Some(5));
            assert_eq!(path.side, Some(CfiSide::Before));
        } else {
            panic!("Expected Point CFI");
        }
    }

    #[test]
    fn test_cfi_side_after_parsed() {
        // :3:after — after the 3rd character
        let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2/1:3:after)").unwrap();
        if let EpubCfi::Point(path) = &cfi {
            assert_eq!(path.character_offset, Some(3));
            assert_eq!(path.side, Some(CfiSide::After));
        } else {
            panic!("Expected Point CFI");
        }
    }

    #[test]
    fn test_cfi_no_side_is_none() {
        // Plain offset — no side annotation
        let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2/1:5)").unwrap();
        if let EpubCfi::Point(path) = &cfi {
            assert_eq!(path.character_offset, Some(5));
            assert_eq!(path.side, None);
        } else {
            panic!("Expected Point CFI");
        }
    }

    #[test]
    fn test_cfi_side_round_trips_via_display() {
        // parse → Display → re-parse must preserve side bias
        let original = "epubcfi(/6/4[ch1]!/4/2/1:5:before)";
        let cfi = EpubCfi::from_str(original).unwrap();
        let rendered = cfi.to_string();
        assert_eq!(rendered, original);

        let reparsed = EpubCfi::from_str(&rendered).unwrap();
        if let EpubCfi::Point(path) = &reparsed {
            assert_eq!(path.side, Some(CfiSide::Before));
        }

        // :after
        let original_after = "epubcfi(/6/4[ch1]!/4/2/1:3:after)";
        let cfi_after = EpubCfi::from_str(original_after).unwrap();
        assert_eq!(cfi_after.to_string(), original_after);
    }

    #[test]
    fn test_cfi_side_propagated_to_resolved() {
        // resolve() must expose side on CfiResolved
        let cfi = EpubCfi::from_str("epubcfi(/6/4[ch1]!/4/2/1:5:before)").unwrap();
        let resolution = cfi.resolve().unwrap();
        assert_eq!(resolution.start.side, Some(CfiSide::Before));
        assert_eq!(resolution.start.character_offset, Some(5));
    }

    // ── split_cfi_top_level unit tests ────────────────────────────────────────

    #[test]
    fn test_split_top_level_point() {
        let parts = split_cfi_top_level("/6/4[ch1]!/4/2:5");
        assert_eq!(parts, vec!["/6/4[ch1]!/4/2:5"]);
    }

    #[test]
    fn test_split_top_level_range_no_assertion() {
        let parts = split_cfi_top_level("/6/4!/4/2,/1:5,/3:10");
        assert_eq!(parts, vec!["/6/4!/4/2", "/1:5", "/3:10"]);
    }

    #[test]
    fn test_split_top_level_range_with_assertion_comma() {
        let parts = split_cfi_top_level("/6/4[a,b]!/4/2,/1:5,/3:10");
        assert_eq!(parts, vec!["/6/4[a,b]!/4/2", "/1:5", "/3:10"]);
    }
}
