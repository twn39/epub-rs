//! EPUB Canonical Fragment Identifier (CFI) implementation.
//!
//! CFI allows pinpointing a specific location within an EPUB document without modifying the underlying files.
//! Format Example: `epubcfi(/6/4[chap01ref]!/4[body01]/10[para05]/2/1:3)`
//! Range Example: `epubcfi(/6/4[chap01ref]!/4[body01]/10[para05],/2/1:1,/3:4)`

use crate::error::EpubError;
use std::fmt;

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

/// Represents a path sequence in a CFI.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CfiPath {
    pub steps: Vec<CfiStep>,
    pub local_steps: Option<Vec<CfiStep>>,
    pub character_offset: Option<u32>,
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
        Ok(())
    }
}

/// Represents a Canonical Fragment Identifier (CFI), which can be a single point or a range.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Point CFIs are compared by their full path. Range CFIs and cross-type comparisons return `None`.
impl PartialOrd for EpubCfi {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (EpubCfi::Point(a), EpubCfi::Point(b)) => Some(a.cmp(b)),
            // Comparing a range to a point or two ranges is not well-defined by spec.
            _ => None,
        }
    }
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

impl std::str::FromStr for EpubCfi {
    type Err = EpubError;

    /// Parses a CFI string, supporting both Point and Range formats.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if !s.starts_with("epubcfi(") || !s.ends_with(')') {
            return Err(EpubError::InvalidFormat("Invalid CFI wrapper".to_string()));
        }

        let inner = &s[8..s.len() - 1]; // inside epubcfi(...)
        let parts: Vec<&str> = inner.split(',').collect();

        if parts.len() == 1 {
            let path = parse_path(parts[0])?;
            Ok(EpubCfi::Point(path))
        } else if parts.len() == 3 {
            let parent = parse_path(parts[0])?;
            let start = parse_path(parts[1])?;
            let end = parse_path(parts[2])?;
            Ok(EpubCfi::Range { parent, start, end })
        } else {
            Err(EpubError::InvalidFormat(
                "Invalid CFI range structure".to_string(),
            ))
        }
    }
}

fn parse_path(s: &str) -> Result<CfiPath, EpubError> {
    let mut path = CfiPath::default();
    let parts: Vec<&str> = s.split('!').collect();

    if parts.len() == 1 {
        let (steps, offset) = parse_steps_and_offset(parts[0])?;
        path.steps = steps;
        path.character_offset = offset;
    } else if parts.len() == 2 {
        let (base_steps, _) = parse_steps_and_offset(parts[0])?;
        path.steps = base_steps;

        let (local_steps, offset) = parse_steps_and_offset(parts[1])?;
        path.local_steps = Some(local_steps);
        path.character_offset = offset;
    } else {
        return Err(EpubError::InvalidFormat(
            "Invalid CFI path structure".to_string(),
        ));
    }

    Ok(path)
}

fn parse_steps_and_offset(s: &str) -> Result<(Vec<CfiStep>, Option<u32>), EpubError> {
    // We must find the colon that denotes the character offset.
    // However, colons can appear inside `[id:assert]` or be escaped `^:`.
    // We scan from the end or just do a single pass parsing.
    let mut offset = None;
    let mut path_str = s;

    // Find unescaped ':' that is not inside an assertion '[' ']'
    let mut in_assertion = false;
    let mut is_escaped = false;
    let mut colon_idx = None;

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
        let offset_str = &s[idx + 1..];
        offset = Some(
            offset_str
                .parse::<u32>()
                .map_err(|_| EpubError::InvalidFormat("Invalid character offset".to_string()))?,
        );
        path_str = &s[..idx];
    }

    let steps = parse_steps(path_str)?;
    Ok((steps, offset))
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
        };
        let p10 = CfiPath {
            steps: vec![CfiStep::new(6, None), CfiStep::new(10, None)],
            local_steps: None,
            character_offset: None,
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
}
