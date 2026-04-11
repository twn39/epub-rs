//! EPUB Canonical Fragment Identifier (CFI) implementation.
//! 
//! CFI allows pinpointing a specific location within an EPUB document without modifying the underlying files.
//! Format Example: `epubcfi(/6/4[chap01ref]!/4[body01]/10[para05]/2/1:3)`
//! Range Example: `epubcfi(/6/4[chap01ref]!/4[body01]/10[para05],/2/1:1,/3:4)`

use crate::error::EpubError;
use std::fmt;

/// Represents a single step in a Canonical Fragment Identifier path.
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CfiPath {
    pub steps: Vec<CfiStep>,
    pub local_steps: Option<Vec<CfiStep>>,
    pub character_offset: Option<u32>,
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
#[derive(Debug, Clone, PartialEq)]
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
    /// Assuming a standard OPF structure where `<spine>` is the 3rd element (/6/6).
    /// 
    /// # Arguments
    /// * `spine_index` - The 0-based index of the item in the spine.
    /// * `item_id` - The OPF manifest ID of the item for the assertion.
    pub fn generate_spine_base_cfi(spine_index: usize, item_id: &str) -> String {
        // Spine itemref elements are even numbers starting from 2
        let item_cfi_index = (spine_index + 1) * 2;
        format!("/6/6/{}[{}]!", item_cfi_index, item_id)
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
            Err(EpubError::InvalidFormat("Invalid CFI range structure".to_string()))
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
        return Err(EpubError::InvalidFormat("Invalid CFI path structure".to_string()));
    }
    
    Ok(path)
}

fn parse_steps_and_offset(s: &str) -> Result<(Vec<CfiStep>, Option<u32>), EpubError> {
    let mut offset = None;
    let mut path_str = s;
    if let Some(colon_idx) = s.find(':') {
        let offset_str = &s[colon_idx + 1..];
        offset = Some(
            offset_str.parse::<u32>()
                .map_err(|_| EpubError::InvalidFormat("Invalid character offset".to_string()))?
        );
        path_str = &s[..colon_idx];
    }
    
    let steps = parse_steps(path_str)?;
    Ok((steps, offset))
}

fn parse_steps(path: &str) -> Result<Vec<CfiStep>, EpubError> {
    let mut steps = Vec::new();
    if path.is_empty() {
        return Ok(steps);
    }
    
    // Skip first slash if it exists
    let path = if path.starts_with('/') { &path[1..] } else { path };
    
    for part in path.split('/') {
        if part.is_empty() {
            continue;
        }
        
        // Check for assertions `[id]`
        let (index_str, assertion) = if let Some(bracket_start) = part.find('[') {
            if let Some(bracket_end) = part.find(']') {
                let index_str = &part[..bracket_start];
                let assertion = &part[bracket_start + 1..bracket_end];
                (index_str, Some(assertion.to_string()))
            } else {
                (part, None)
            }
        } else {
            (part, None)
        };
        
        let index = index_str.parse::<u32>().map_err(|_| EpubError::InvalidFormat(format!("Invalid CFI index: {}", index_str)))?;
        steps.push(CfiStep::new(index, assertion));
    }
    
    Ok(steps)
}
