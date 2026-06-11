use crate::cfi::model::{CfiPath, CfiSide, CfiStep, EpubCfi, SpatialOffset};
use crate::error::EpubError;
use std::str::FromStr;

/// Structured result of [`parse_steps_and_offset`].
pub(crate) struct ParsedTerminal {
    pub(crate) steps: Vec<CfiStep>,
    pub(crate) char_offset: Option<u32>,
    pub(crate) side: Option<CfiSide>,
    pub(crate) temporal_offset: Option<f64>,
    pub(crate) spatial_offset: Option<SpatialOffset>,
}

/// Formats an f64 per CFI spec §2.2: integers without decimal point, fractions as-is.
pub(crate) fn format_cfi_number(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{}", n)
    }
}

/// Splits the inner content of `epubcfi(...)` by top-level commas.
///
/// A "top-level" comma is one that is **not** inside an assertion bracket `[...]`
/// and **not** preceded by the CFI escape character `^`.
///
/// This is necessary because EPUB CFI assertions can legally contain commas
/// (e.g. `[chap,v2]`), which must not be mistaken for the Range separator.
pub(crate) fn split_cfi_top_level(s: &str) -> Vec<&str> {
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

impl FromStr for EpubCfi {
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

pub(crate) fn parse_path(s: &str) -> Result<CfiPath, EpubError> {
    let mut path = CfiPath::default();
    let parts: Vec<&str> = s.split('!').collect();

    if parts.len() == 1 {
        let t = parse_steps_and_offset(parts[0])?;
        path.steps = t.steps;
        path.character_offset = t.char_offset;
        path.side = t.side;
        path.temporal_offset = t.temporal_offset;
        path.spatial_offset = t.spatial_offset;
    } else if parts.len() == 2 {
        // Base path (before '!') carries no terminal offset.
        path.steps = parse_steps_and_offset(parts[0])?.steps;
        let t = parse_steps_and_offset(parts[1])?;
        path.local_steps = Some(t.steps);
        path.character_offset = t.char_offset;
        path.side = t.side;
        path.temporal_offset = t.temporal_offset;
        path.spatial_offset = t.spatial_offset;
    } else {
        return Err(EpubError::InvalidFormat(
            "Invalid CFI path structure".to_string(),
        ));
    }

    Ok(path)
}

/// Scans `s` for the first unescaped terminus character (`:`, `~`, or `@`)
/// outside assertion brackets, then parses all CFI §3.1.4–3.1.7 terminus forms.
pub(crate) fn parse_steps_and_offset(s: &str) -> Result<ParsedTerminal, EpubError> {
    let mut in_assertion = false;
    let mut is_escaped = false;
    let mut terminus: Option<(usize, char)> = None;

    for (i, c) in s.char_indices() {
        if is_escaped {
            is_escaped = false;
        } else if c == '^' {
            is_escaped = true;
        } else if c == '[' {
            in_assertion = true;
        } else if c == ']' {
            in_assertion = false;
        } else if !in_assertion && matches!(c, ':' | '~' | '@') {
            terminus = Some((i, c));
            break;
        }
    }

    let Some((idx, tc)) = terminus else {
        return Ok(ParsedTerminal {
            steps: parse_steps(s)?,
            char_offset: None,
            side: None,
            temporal_offset: None,
            spatial_offset: None,
        });
    };

    let steps = parse_steps(&s[..idx])?;
    let rest = &s[idx + 1..];

    match tc {
        ':' => {
            // `:N` or `:N:before` / `:N:after`
            let (offset_part, side) = if let Some(sep) = rest.rfind(':') {
                match &rest[sep + 1..] {
                    "before" => (&rest[..sep], Some(CfiSide::Before)),
                    "after" => (&rest[..sep], Some(CfiSide::After)),
                    _ => (rest, None),
                }
            } else {
                (rest, None)
            };
            let offset = offset_part.parse::<u32>().map_err(|_| {
                EpubError::InvalidFormat(format!("Invalid CFI character offset: '{offset_part}'"))
            })?;
            Ok(ParsedTerminal {
                steps,
                char_offset: Some(offset),
                side,
                temporal_offset: None,
                spatial_offset: None,
            })
        }
        '~' => {
            // `~N` or `~N@x:y`
            let (t_str, sp_str) = match rest.find('@') {
                Some(at) => (&rest[..at], Some(&rest[at + 1..])),
                None => (rest, None),
            };
            let temporal = t_str.parse::<f64>().map_err(|_| {
                EpubError::InvalidFormat(format!("Invalid CFI temporal offset: '{t_str}'"))
            })?;
            let spatial_offset = sp_str.map(parse_spatial_coords).transpose()?;
            Ok(ParsedTerminal {
                steps,
                char_offset: None,
                side: None,
                temporal_offset: Some(temporal),
                spatial_offset,
            })
        }
        '@' => {
            // pure `@x:y`
            Ok(ParsedTerminal {
                steps,
                char_offset: None,
                side: None,
                temporal_offset: None,
                spatial_offset: Some(parse_spatial_coords(rest)?),
            })
        }
        _ => unreachable!(),
    }
}

pub(crate) fn parse_spatial_coords(s: &str) -> Result<SpatialOffset, EpubError> {
    let mut it = s.splitn(2, ':');
    let xs = it.next().unwrap_or("");
    let ys = it.next().unwrap_or("");
    Ok(SpatialOffset {
        x: xs
            .parse()
            .map_err(|_| EpubError::InvalidFormat(format!("Bad CFI x: '{xs}'")))?,
        y: ys
            .parse()
            .map_err(|_| EpubError::InvalidFormat(format!("Bad CFI y: '{ys}'")))?,
    })
}

pub(crate) fn parse_steps(path: &str) -> Result<Vec<CfiStep>, EpubError> {
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
