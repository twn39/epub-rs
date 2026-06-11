use crate::cfi::model::{
    CfiPath, CfiResolution, CfiResolved, CfiStep, EpubCfi, ResolvedStep,
};
use crate::error::EpubError;


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
        let mut xpath_ns_agnostic_parts: Vec<String> = vec![".".into(), "*[local-name()]".into()];

        for step in steps {
            let pos = step.child_index() + 1; // 1-based for XPath

            if step.is_text_node() {
                // text()[N] — XPath can address text nodes directly
                xpath_parts.push(format!("text()[{pos}]"));
                xpath_ns_agnostic_parts.push(format!("text()[{pos}]"));
                // text steps carry no id; stop XPath building here
            } else {
                if let Some(ref id) = step.assertion {
                    // epub.js: "*[position()=N and @id='id']"
                    xpath_parts.push(format!("*[position()={pos} and @id='{id}']"));
                    xpath_ns_agnostic_parts.push(format!(
                        "*[local-name()][position()={pos} and @id='{id}']"
                    ));
                    // Track the deepest id for the getElementById shortcut
                    id_shortcut = Some(id.clone());
                } else {
                    xpath_parts.push(format!("*[{pos}]"));
                    xpath_ns_agnostic_parts.push(format!("*[local-name()][{pos}]"));
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
            xpath_ns_agnostic: xpath_ns_agnostic_parts.join("/"),
            id_shortcut,
            side: self.side.clone(),
            temporal_offset: self.temporal_offset,
            spatial_offset: self.spatial_offset.clone(),
        })
    }
}

impl EpubCfi {
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
                        temporal_offset: half.temporal_offset,
                        spatial_offset: half.spatial_offset.clone(),
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
