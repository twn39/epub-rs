//! Neutral CSS URL rewrite engine shared by HTML streaming and standalone CSS.
//!
//! Both [`super::html`] and [`super::css`] depend on this module so neither owns
//! the other — path joining still goes through [`crate::path`].

use crate::path::{is_external_url, normalize_path};

/// Span of a single rewriteable URL token inside a CSS string.
#[derive(Debug)]
struct CssUrlSpan {
    /// Byte offset of the first character of the full token (the `u` of `url(` or the `@` of `@import`).
    start: usize,
    /// Byte offset one past the last character of the token (the closing `)` or closing quote).
    end: usize,
    /// Byte offsets within `css` of the URL value itself (without quotes / parens).
    inner_start: usize,
    inner_end: usize,
}

/// Rewrites `url(...)` and `@import` references using an already-resolved base directory.
///
/// `base_dir` is the EPUB-root-relative directory of the referencing file (not the file path).
/// Used by HTML for inline `<style>` / `style=""` and by [`super::css::rewrite_css`] after
/// stripping the file name from a CSS path.
pub(crate) fn rewrite_css_urls<F>(css: &str, base_dir: &str, resolver: &mut F) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    let spans = scan_css_url_spans(css);
    if spans.is_empty() {
        return css.to_string();
    }

    let mut out = String::with_capacity(css.len() + spans.len() * 32);
    let mut last = 0usize;

    for span in &spans {
        let inner = css[span.inner_start..span.inner_end].trim();

        out.push_str(&css[last..span.start]);

        if inner.is_empty() || is_external_url(inner) {
            out.push_str(&css[span.start..span.end]);
        } else {
            let abs = normalize_path(base_dir, inner);
            match resolver(&abs) {
                Some(new_url) => {
                    if css[span.start..].starts_with("@import") {
                        out.push_str("@import \"");
                        out.push_str(&new_url);
                        out.push('"');
                    } else {
                        out.push_str("url(\"");
                        out.push_str(&new_url);
                        out.push_str("\")");
                    }
                }
                None => {
                    out.push_str(&css[span.start..span.end]);
                }
            }
        }

        last = span.end;
    }

    out.push_str(&css[last..]);
    out
}

/// Scans `css` and returns the byte spans of every `url(...)` and `@import` token.
fn scan_css_url_spans(css: &str) -> Vec<CssUrlSpan> {
    let bytes = css.as_bytes();
    let len = bytes.len();
    let mut spans: Vec<CssUrlSpan> = Vec::new();
    let mut i = 0usize;

    while i < len {
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        }

        if bytes[i] == b'u'
            && i + 3 < len
            && bytes[i + 1] == b'r'
            && bytes[i + 2] == b'l'
            && bytes[i + 3] == b'('
        {
            let token_start = i;
            i += 4;
            while i < len && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
                i += 1;
            }
            if i >= len {
                break;
            }
            let (inner_s, inner_e, token_end) = if bytes[i] == b'"' || bytes[i] == b'\'' {
                let q = bytes[i];
                i += 1;
                let s = i;
                while i < len && bytes[i] != q {
                    i += 1;
                }
                let e = i;
                if i < len {
                    i += 1;
                }
                while i < len && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
                    i += 1;
                }
                if i < len && bytes[i] == b')' {
                    i += 1;
                    (s, e, i)
                } else {
                    continue;
                }
            } else {
                let s = i;
                while i < len && bytes[i] != b')' {
                    i += 1;
                }
                let raw = css[s..i].trim_end();
                let e = s + raw.len();
                if i < len {
                    i += 1;
                }
                (s, e, i)
            };
            spans.push(CssUrlSpan {
                start: token_start,
                end: token_end,
                inner_start: inner_s,
                inner_end: inner_e,
            });
            continue;
        }

        if bytes[i] == b'@' && css[i..].starts_with("@import") {
            let token_start = i;
            i += 7;
            while i < len && matches!(bytes[i], b' ' | b'\t') {
                i += 1;
            }
            if i < len && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let q = bytes[i];
                i += 1;
                let s = i;
                while i < len && bytes[i] != q {
                    i += 1;
                }
                let e = i;
                if i < len {
                    i += 1;
                }
                spans.push(CssUrlSpan {
                    start: token_start,
                    end: i,
                    inner_start: s,
                    inner_end: e,
                });
            } else {
                i = token_start + 1;
            }
            continue;
        }

        i += 1;
    }
    spans
}
