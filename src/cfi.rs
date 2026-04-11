//! EPUB Canonical Fragment Identifier (CFI) implementation.
//! 
//! CFI allows pinpointing a specific location within an EPUB document without modifying the underlying files.
//! Format Example: `epubcfi(/6/4[chap01ref]!/4[body01]/10[para05]/2/1:3)`

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

/// Represents a Canonical Fragment Identifier (CFI).
#[derive(Debug, Clone, PartialEq)]
pub struct EpubCfi {
    /// The base path within the EPUB package (resolving through the OPF to the specific item).
    pub base_path: Vec<CfiStep>,
    /// The path within the specific document (e.g., inside the HTML file).
    pub local_path: Vec<CfiStep>,
    /// The character offset within the final text node, if applicable.
    pub character_offset: Option<u32>,
}

impl EpubCfi {
    /// Creates a new empty CFI.
    pub fn new() -> Self {
        Self {
            base_path: Vec::new(),
            local_path: Vec::new(),
            character_offset: None,
        }
    }

    /// Add a step to the base path (OPF context).
    pub fn add_base_step(mut self, step: CfiStep) -> Self {
        self.base_path.push(step);
        self
    }

    /// Add a step to the local path (HTML context).
    pub fn add_local_step(mut self, step: CfiStep) -> Self {
        self.local_path.push(step);
        self
    }

    /// Set the final character offset.
    pub fn character_offset(mut self, offset: u32) -> Self {
        self.character_offset = Some(offset);
        self
    }
}

impl fmt::Display for EpubCfi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "epubcfi(")?;
        for step in &self.base_path {
            write!(f, "{}", step)?;
        }
        if !self.local_path.is_empty() {
            write!(f, "!")?;
            for step in &self.local_path {
                write!(f, "{}", step)?;
            }
        }
        if let Some(offset) = self.character_offset {
            write!(f, ":{}", offset)?;
        }
        write!(f, ")")
    }
}

impl std::str::FromStr for EpubCfi {
    type Err = EpubError;

    /// Extremely basic string parsing for CFI. Only supports `epubcfi(...)` format.
    /// Does not implement full range, temporal, or spatial parsing.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if !s.starts_with("epubcfi(") || !s.ends_with(')') {
            return Err(EpubError::InvalidFormat("Invalid CFI wrapper".to_string()));
        }

        let inner = &s[8..s.len() - 1]; // inside epubcfi(...)
        let mut cfi = EpubCfi::new();
        
        let parts: Vec<&str> = inner.split('!').collect();
        if parts.is_empty() || parts.len() > 2 {
            return Err(EpubError::InvalidFormat("Invalid CFI structure".to_string()));
        }

        // Parse base steps
        cfi.base_path = parse_steps(parts[0])?;

        // Parse local steps if `!` is present
        if parts.len() == 2 {
            let mut local_str = parts[1];
            
            // Check if there is a character offset `:3`
            if let Some(colon_idx) = local_str.find(':') {
                let offset_str = &local_str[colon_idx + 1..];
                cfi.character_offset = Some(
                    offset_str.parse::<u32>()
                        .map_err(|_| EpubError::InvalidFormat("Invalid character offset".to_string()))?
                );
                local_str = &local_str[..colon_idx];
            }
            
            cfi.local_path = parse_steps(local_str)?;
        }

        Ok(cfi)
    }
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
