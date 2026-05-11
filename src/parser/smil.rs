//! SMIL 3.0 / EPUB Media Overlays parser.
//!
//! # Design decisions
//!
//! - Uses a single-pass stack-based state machine (via explicit [`SeqFrame`] /
//!   [`ParFrame`] accumulators) to correctly handle arbitrarily-nested `<seq>`
//!   elements without any sub-Reader instantiation.  This sidesteps the
//!   `quick-xml` `IllFormed::UnmatchedEndTag` / `MismatchedEndTag` errors that
//!   arise when `read_to_end_into` is used on inner buffers that lack a
//!   synthetic wrapper root.
//!
//! - Clock values are converted to `f64` seconds and formatted as W3C Media
//!   Fragment URI temporal fragments (`#t=begin,end`) per
//!   <https://www.w3.org/TR/media-frags/>.
//!
//! - A missing `epub:textref` on a `<seq>` element is tolerated (warn-and-
//!   continue) rather than treated as an error.  Many real-world EPUB
//!   audiobooks omit this attribute even though the spec marks it REQUIRED.
//!
//! - Namespace prefixes are stripped when comparing element names, so both the
//!   EPUB 3 SMIL 3.0 namespace and the EPUB 2 SMIL 2.0 namespace are handled
//!   transparently without any extra branching.

use crate::{error::EpubError, model::SmilObject};
use quick_xml::{Reader, events::Event};

// ── Public entry point ────────────────────────────────────────────────────────

/// Parses the XML content of a `.smil` file and returns an ordered list of
/// [`SmilObject`] nodes that represent the synchronization structure.
///
/// `smil_dir` is the EPUB-root-relative directory that contains the SMIL file
/// (e.g. `"OEBPS/audio"`).  It is used to resolve relative `src` attribute
/// values to EPUB-root-relative paths.
pub(super) fn parse_smil(xml: &str, smil_dir: &str) -> Result<Vec<SmilObject>, EpubError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    // Allow unmatched end-tags so malformed real-world EPUB SMIL files don't abort.
    reader.config_mut().allow_unmatched_ends = true;

    let mut event_buf = Vec::new();

    // Stack of seq frames — root frame is always present and collects top-level nodes.
    let mut stack: Vec<SeqFrame> = vec![SeqFrame::root()];
    let mut in_body = false;
    // Accumulator for the <par> element currently being built.
    let mut cur_par: Option<ParFrame> = None;

    loop {
        match reader.read_event_into(&mut event_buf)? {
            // ── Opening tags ─────────────────────────────────────────────────
            Event::Start(ref e) => {
                let local = local_name_str(e.name().into_inner());
                match local.as_str() {
                    "body" => {
                        in_body = true;
                    }
                    "seq" if in_body => {
                        let text_ref =
                            resolve_src(attr_value(e, "epub:textref").as_deref(), smil_dir);
                        let role = epub_type_roles(e);
                        stack.push(SeqFrame {
                            text_ref: text_ref.unwrap_or_default(),
                            role,
                            children: Vec::new(),
                        });
                    }
                    "par" if in_body => {
                        cur_par = Some(ParFrame {
                            role: epub_type_roles(e),
                            text_ref: String::new(),
                            audio_src: None,
                            clip_begin: None,
                            clip_end: None,
                        });
                    }
                    // <text> and <audio> inside a <par>
                    "text" => {
                        if let Some(ref mut p) = cur_par {
                            if let Some(src) = attr_value(e, "src") {
                                p.text_ref = resolve_src(Some(&src), smil_dir).unwrap_or(src);
                            }
                        }
                    }
                    "audio" => {
                        if let Some(ref mut p) = cur_par {
                            p.fill_audio(e, smil_dir);
                        }
                    }
                    _ => {}
                }
            }
            // ── Self-closing / empty tags ────────────────────────────────────
            Event::Empty(ref e) => {
                let local = local_name_str(e.name().into_inner());
                match local.as_str() {
                    "text" => {
                        if let Some(ref mut p) = cur_par {
                            if let Some(src) = attr_value(e, "src") {
                                p.text_ref = resolve_src(Some(&src), smil_dir).unwrap_or(src);
                            }
                        }
                    }
                    "audio" => {
                        if let Some(ref mut p) = cur_par {
                            p.fill_audio(e, smil_dir);
                        }
                    }
                    // Self-closing <par/> (uncommon but valid; produces an empty node)
                    "par" if in_body => {
                        // A self-closing <par/> with no children is a degenerate case;
                        // only push it if the caller wants to track it (currently skipped).
                    }
                    _ => {}
                }
            }
            // ── Closing tags ─────────────────────────────────────────────────
            Event::End(ref e) => {
                let local = local_name_str(e.name().into_inner());
                match local.as_str() {
                    "body" => {
                        in_body = false;
                    }
                    "par" => {
                        if let Some(p) = cur_par.take() {
                            if !p.text_ref.is_empty() || p.audio_src.is_some() {
                                let audio_ref = p.audio_src.map(|src| {
                                    format_media_fragment(&src, p.clip_begin, p.clip_end)
                                });
                                let obj = SmilObject {
                                    text_ref: p.text_ref,
                                    audio_ref,
                                    role: p.role,
                                    children: Vec::new(),
                                };
                                if let Some(frame) = stack.last_mut() {
                                    frame.children.push(obj);
                                }
                            }
                        }
                    }
                    "seq" => {
                        // Pop the innermost seq frame and fold into its parent.
                        if stack.len() > 1 {
                            let frame = stack.pop().unwrap();
                            let obj = SmilObject {
                                text_ref: frame.text_ref,
                                audio_ref: None,
                                role: frame.role,
                                children: frame.children,
                            };
                            if let Some(parent) = stack.last_mut() {
                                parent.children.push(obj);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        event_buf.clear();
    }

    Ok(stack.into_iter().next().map(|f| f.children).unwrap_or_default())
}

// ── State machine frames ──────────────────────────────────────────────────────

/// Stack frame for a `<seq>` element being built.
struct SeqFrame {
    text_ref: String,
    role: Vec<String>,
    children: Vec<SmilObject>,
}

impl SeqFrame {
    fn root() -> Self {
        SeqFrame {
            text_ref: String::new(),
            role: Vec::new(),
            children: Vec::new(),
        }
    }
}

/// Accumulator for a `<par>` element being built.
struct ParFrame {
    role: Vec<String>,
    text_ref: String,
    audio_src: Option<String>,
    clip_begin: Option<f64>,
    clip_end: Option<f64>,
}

impl ParFrame {
    /// Extracts `src`, `clipBegin`, and `clipEnd` from an `<audio>` element.
    fn fill_audio(&mut self, e: &quick_xml::events::BytesStart<'_>, smil_dir: &str) {
        if let Some(src) = attr_value(e, "src") {
            self.audio_src = Some(resolve_src(Some(&src), smil_dir).unwrap_or(src));
        }
        self.clip_begin = attr_value(e, "clipBegin")
            .as_deref()
            .and_then(parse_clock_value);
        self.clip_end = attr_value(e, "clipEnd")
            .as_deref()
            .and_then(parse_clock_value);
    }
}

// ── Clock value parsing ───────────────────────────────────────────────────────

/// Parses a SMIL clock value string into seconds (`f64`).
///
/// Supports all three formats defined in EPUB 3.3 Appendix H.4:
///
/// | Format | Example | Rule |
/// |--------|---------|------|
/// | Full clock | `"0:23:22.000"` | `HH:MM:SS[.mmm]` |
/// | Partial clock | `"02:33.345"` | `MM:SS[.mmm]` |
/// | Timecount | `"3.45s"` / `"345ms"` / `"2.5min"` / `"1.5h"` | bare number + unit |
///
/// Returns `None` if the value cannot be parsed.
pub(super) fn parse_clock_value(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    // Full clock: HH:MM:SS[.mmm]  or  Partial clock: MM:SS[.mmm]
    if s.contains(':') {
        let parts: Vec<&str> = s.split(':').collect();
        return match parts.len() {
            2 => {
                let m: f64 = parts[0].parse().ok()?;
                let sec: f64 = parts[1].parse().ok()?;
                Some(m * 60.0 + sec)
            }
            3 => {
                let h: f64 = parts[0].parse().ok()?;
                let m: f64 = parts[1].parse().ok()?;
                let sec: f64 = parts[2].parse().ok()?;
                Some(h * 3600.0 + m * 60.0 + sec)
            }
            _ => None,
        };
    }

    // Timecount with unit suffix — order matters: check "ms" before "s"
    if let Some(v) = s.strip_suffix("ms") {
        return v.trim().parse::<f64>().ok().map(|n| n / 1000.0);
    }
    if let Some(v) = s.strip_suffix("min") {
        return v.trim().parse::<f64>().ok().map(|n| n * 60.0);
    }
    if let Some(v) = s.strip_suffix('h') {
        return v.trim().parse::<f64>().ok().map(|n| n * 3600.0);
    }
    if let Some(v) = s.strip_suffix('s') {
        return v.trim().parse::<f64>().ok();
    }

    // Bare number — treat as seconds (common in practice, spec does not define this)
    s.parse::<f64>().ok()
}

// ── Media Fragment URI ────────────────────────────────────────────────────────

/// Formats a W3C Media Fragment URI temporal fragment.
///
/// Spec: <https://www.w3.org/TR/media-frags/#naming-time>
/// Output: `"path/to/audio.mp3#t=1.000,3.450"`
fn format_media_fragment(src: &str, begin: Option<f64>, end: Option<f64>) -> String {
    match (begin, end) {
        (Some(b), Some(e)) => format!("{src}#t={b:.3},{e:.3}"),
        (Some(b), None)    => format!("{src}#t={b:.3}"),
        (None, Some(e))    => format!("{src}#t=0.000,{e:.3}"),
        (None, None)       => src.to_string(),
    }
}

// ── XML attribute helpers ─────────────────────────────────────────────────────

/// Returns the local name (stripping any namespace prefix) as a `String`.
fn local_name_str(name: &[u8]) -> String {
    let s = std::str::from_utf8(name).unwrap_or("");
    s.rsplit_once(':').map(|(_, local)| local).unwrap_or(s).to_string()
}

/// Returns the value of a named attribute from a start/empty element.
///
/// Matches both the bare local name (e.g. `"src"`) and the fully-prefixed form
/// (e.g. `"epub:textref"`) since callers may supply either.
fn attr_value(e: &quick_xml::events::BytesStart<'_>, name: &str) -> Option<String> {
    for attr in e.attributes().flatten() {
        let key_raw = std::str::from_utf8(attr.key.into_inner()).unwrap_or("");
        let key_local = key_raw.rsplit_once(':').map(|(_, l)| l).unwrap_or(key_raw);
        if key_local == name || key_raw == name {
            return Some(String::from_utf8_lossy(&attr.value).into_owned());
        }
    }
    None
}

/// Extracts `epub:type` semantic role tokens from an element's attributes.
fn epub_type_roles(e: &quick_xml::events::BytesStart<'_>) -> Vec<String> {
    for attr in e.attributes().flatten() {
        let key = std::str::from_utf8(attr.key.into_inner()).unwrap_or("");
        if key == "epub:type" || key.ends_with(":type") {
            let val = String::from_utf8_lossy(&attr.value);
            return val.split_whitespace().map(|s| s.to_string()).collect();
        }
    }
    Vec::new()
}

// ── Path helpers ──────────────────────────────────────────────────────────────

/// Resolves a (possibly relative) `src` attribute value to an EPUB-root-relative path.
fn resolve_src(src: Option<&str>, smil_dir: &str) -> Option<String> {
    let src = src?;
    if src.is_empty() {
        return None;
    }
    if src.starts_with('#') {
        return Some(src.to_string());
    }
    Some(normalize_epub_path(smil_dir, src))
}

/// Resolves a relative href against a base directory to an EPUB-root-relative path.
///
/// Mirrors [`super::EpubArchive::normalize_path`] without requiring a provider type parameter.
fn normalize_epub_path(base_dir: &str, href: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for comp in base_dir.split('/').chain(href.split('/')) {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." {
            parts.pop();
        } else {
            parts.push(comp);
        }
    }
    parts.join("/")
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_clock_value ─────────────────────────────────────────────────────

    #[test]
    fn clock_full_hms() {
        assert_eq!(parse_clock_value("0:02:37.970"), Some(157.97));
    }

    #[test]
    fn clock_full_hms_no_ms() {
        assert_eq!(parse_clock_value("1:00:00"), Some(3600.0));
    }

    #[test]
    fn clock_partial_ms() {
        assert_eq!(parse_clock_value("02:33.345"), Some(153.345));
    }

    #[test]
    fn clock_timecount_seconds() {
        assert_eq!(parse_clock_value("3.45s"), Some(3.45));
    }

    #[test]
    fn clock_timecount_milliseconds() {
        assert_eq!(parse_clock_value("345ms"), Some(0.345));
    }

    #[test]
    fn clock_timecount_minutes() {
        assert_eq!(parse_clock_value("2.5min"), Some(150.0));
    }

    #[test]
    fn clock_timecount_hours() {
        assert_eq!(parse_clock_value("1.5h"), Some(5400.0));
    }

    #[test]
    fn clock_bare_number() {
        assert_eq!(parse_clock_value("5.0"), Some(5.0));
    }

    #[test]
    fn clock_invalid() {
        assert_eq!(parse_clock_value("not-a-time"), None);
    }

    // ── format_media_fragment ─────────────────────────────────────────────────

    #[test]
    fn media_fragment_both() {
        assert_eq!(
            format_media_fragment("audio/ch1.mp3", Some(0.0), Some(3.45)),
            "audio/ch1.mp3#t=0.000,3.450"
        );
    }

    #[test]
    fn media_fragment_none() {
        assert_eq!(
            format_media_fragment("audio/ch1.mp3", None, None),
            "audio/ch1.mp3"
        );
    }

    // ── parse_smil (flat par in seq) ──────────────────────────────────────────

    #[test]
    fn parse_flat_par_sequence() {
        let xml = r#"<?xml version="1.0"?>
<smil xmlns="http://www.w3.org/ns/SMIL">
  <body>
    <seq epub:textref="ch01.xhtml" epub:type="chapter">
      <par id="par0">
        <text src="ch01.xhtml#word_0001"/>
        <audio src="audio/ch01.mp3" clipBegin="0s" clipEnd="0.840s"/>
      </par>
      <par id="par1">
        <text src="ch01.xhtml#word_0002"/>
        <audio src="audio/ch01.mp3" clipBegin="0.840s" clipEnd="1.920s"/>
      </par>
    </seq>
  </body>
</smil>"#;

        let objects = parse_smil(xml, "OEBPS").unwrap();
        assert_eq!(objects.len(), 1); // one top-level <seq>
        let seq = &objects[0];
        assert_eq!(seq.text_ref, "OEBPS/ch01.xhtml");
        assert_eq!(seq.role, vec!["chapter"]);
        assert_eq!(seq.children.len(), 2);

        let par0 = &seq.children[0];
        assert_eq!(par0.text_ref, "OEBPS/ch01.xhtml#word_0001");
        assert_eq!(
            par0.audio_ref.as_deref(),
            Some("OEBPS/audio/ch01.mp3#t=0.000,0.840")
        );

        let par1 = &seq.children[1];
        assert_eq!(par1.text_ref, "OEBPS/ch01.xhtml#word_0002");
        assert_eq!(
            par1.audio_ref.as_deref(),
            Some("OEBPS/audio/ch01.mp3#t=0.840,1.920")
        );
    }

    // ── parse_smil (nested seq) ───────────────────────────────────────────────

    #[test]
    fn parse_nested_seq() {
        let xml = r#"<?xml version="1.0"?>
<smil xmlns="http://www.w3.org/ns/SMIL">
  <body>
    <seq epub:textref="ch01.xhtml" epub:type="chapter">
      <seq epub:textref="ch01.xhtml#aside1" epub:type="sidebar">
        <par>
          <text src="ch01.xhtml#aside1_p1"/>
          <audio src="audio/ch01.mp3" clipBegin="5s" clipEnd="8s"/>
        </par>
      </seq>
    </seq>
  </body>
</smil>"#;

        let objects = parse_smil(xml, "OEBPS").unwrap();
        assert_eq!(objects.len(), 1);
        let chapter = &objects[0];
        assert_eq!(chapter.role, vec!["chapter"]);
        assert_eq!(chapter.children.len(), 1);

        let aside = &chapter.children[0];
        assert_eq!(aside.text_ref, "OEBPS/ch01.xhtml#aside1");
        assert_eq!(aside.role, vec!["sidebar"]);
        assert_eq!(aside.children.len(), 1);

        let par = &aside.children[0];
        assert_eq!(par.text_ref, "OEBPS/ch01.xhtml#aside1_p1");
        assert_eq!(
            par.audio_ref.as_deref(),
            Some("OEBPS/audio/ch01.mp3#t=5.000,8.000")
        );
    }

    // ── Tolerates missing epub:textref on <seq> ───────────────────────────────

    #[test]
    fn parse_seq_missing_textref() {
        let xml = r#"<?xml version="1.0"?>
<smil xmlns="http://www.w3.org/ns/SMIL">
  <body>
    <seq>
      <par>
        <text src="ch01.xhtml#p1"/>
        <audio src="audio.mp3" clipBegin="0s" clipEnd="1s"/>
      </par>
    </seq>
  </body>
</smil>"#;

        let objects = parse_smil(xml, "OEBPS").unwrap();
        assert_eq!(objects.len(), 1);
        assert!(objects[0].text_ref.is_empty());
        assert_eq!(objects[0].children.len(), 1);
    }
}
