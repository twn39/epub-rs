//! Hand-written CSS URL/import parsing and rewriting implementation.
//!
//! Separated from `html.rs` to keep CSS-specific parsing logic independent of HTML streaming rewriting.

use super::html::{is_external_url, normalize_path};

/// Rewrites `url(...)` and `@import` references inside a CSS string.
///
/// For each relative URL found, `resolver(epub_root_relative_path)` is called:
/// - `Some(new_url)` → the `url(...)` token is replaced with `url("new_url")`
/// - `None`          → the token is left unchanged
///
/// `css_file_path` is the **EPUB-root-relative path of the CSS file itself**
/// (e.g. `"OEBPS/css/style.css"`), used to resolve relative paths before
/// calling the resolver.  Pass the path of the containing file for inline
/// `<style>` blocks.
///
/// Handles all three CSS `url()` syntaxes (`url("…")`, `url('…')`, `url(…)`)
/// and `@import "…"` / `@import '…'` rules.  External URLs (`http:`, `https:`,
/// `data:`, `blob:`) are always passed through unchanged.
///
/// Uses a zero-dependency hand-written two-pass scanner — no `regex` crate —
/// so this compiles to minimal WASM binary size.
pub fn rewrite_css<F>(css: &str, css_file_path: &str, mut resolver: F) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    let css_dir = match css_file_path.rfind('/') {
        Some(idx) => &css_file_path[..idx],
        None => "",
    };
    rewrite_css_impl(css, css_dir, &mut resolver)
}

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

/// Hand-written two-pass CSS url() / @import scanner.
///
/// Pass 1: collect all spans.
/// Pass 2: build the output string by splicing in resolved URLs.
pub(crate) fn rewrite_css_impl<F>(css: &str, css_dir: &str, resolver: &mut F) -> String
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

        // Append everything before this token unchanged.
        out.push_str(&css[last..span.start]);

        if inner.is_empty() || is_external_url(inner) {
            // Keep the original token as-is.
            out.push_str(&css[span.start..span.end]);
        } else {
            let abs = normalize_path(css_dir, inner);
            match resolver(&abs) {
                Some(new_url) => {
                    // Preserve the token type: @import → @import, url() → url()
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
                    // resolver declined — keep original.
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
///
/// Correctly handles:
/// - `url("path")`, `url('path')`, `url(path)` (with optional surrounding whitespace)
/// - `@import "path"`, `@import 'path'`
/// - Data URLs, blob URLs and http(s) URLs (returned as spans so the caller can skip them)
/// - CSS comments (`/* … */`) are skipped so comment-embedded `url()` isn't matched
fn scan_css_url_spans(css: &str) -> Vec<CssUrlSpan> {
    let bytes = css.as_bytes();
    let len = bytes.len();
    let mut spans: Vec<CssUrlSpan> = Vec::new();
    let mut i = 0usize;

    while i < len {
        // Skip CSS block comments /* … */
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        }

        // Match url(
        if bytes[i] == b'u'
            && i + 3 < len
            && bytes[i + 1] == b'r'
            && bytes[i + 2] == b'l'
            && bytes[i + 3] == b'('
        {
            let token_start = i;
            i += 4;
            // Skip whitespace inside the parens
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
                    i += 1; // closing quote
                }
                // skip whitespace then expect ')'
                while i < len && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
                    i += 1;
                }
                if i < len && bytes[i] == b')' {
                    i += 1;
                    (s, e, i)
                } else {
                    // Malformed; skip this token
                    continue;
                }
            } else {
                // Unquoted: read to closing ')'
                let s = i;
                while i < len && bytes[i] != b')' {
                    i += 1;
                }
                let raw = css[s..i].trim_end();
                let e = s + raw.len();
                if i < len {
                    i += 1; // closing ')'
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

        // Match @import "..." or @import '...'
        if bytes[i] == b'@' && css[i..].starts_with("@import") {
            let token_start = i;
            i += 7; // skip "@import"
            // Skip whitespace
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
                    i += 1; // closing quote
                }
                // Emit as a synthetic url() span so the caller's logic is uniform.
                // We'll emit `url("new")` for @import too — caller sees it as url().
                // Instead, mark it differently by overriding: we store the span of
                // `@import "..."` so the replacer knows to write `@import "new_url"`.
                // Use a sentinel: inner_start > inner_end means it's an @import.
                spans.push(CssUrlSpan {
                    start: token_start,
                    end: i,
                    inner_start: s,
                    inner_end: e,
                });
                // Note: we repurpose the struct — the replacer checks start/end/inner.
                // @import spans are identified by css[token_start..token_start+7] == "@import".
            } else {
                // @import url(...) form — the url() branch above will handle it
                // on the next iteration.  Don't advance i; just step past the '@'.
                i = token_start + 1;
            }
            continue;
        }

        i += 1;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rewrite_css_at_font_face() {
        let css = r#"
@font-face {
    font-family: "Book";
    src: url("../fonts/book.woff2") format("woff2"),
         url('../fonts/book.ttf') format("truetype");
}
"#;
        let result = rewrite_css(css, "OEBPS/css/style.css", |path| {
            Some(format!("blob:{path}"))
        });
        assert!(result.contains("url(\"blob:OEBPS/fonts/book.woff2\")"));
        assert!(result.contains("url(\"blob:OEBPS/fonts/book.ttf\")"));
        // format() hints must survive unchanged
        assert!(result.contains("format(\"woff2\")"));
        assert!(result.contains("format(\"truetype\")"));
    }

    #[test]
    fn test_rewrite_css_background_image() {
        let css = r#"body { background-image: url("../images/bg.png"); }"#;
        let result = rewrite_css(css, "OEBPS/css/style.css", |path| {
            Some(format!("blob:{path}"))
        });
        assert!(result.contains("url(\"blob:OEBPS/images/bg.png\")"));
    }

    #[test]
    fn test_rewrite_css_unquoted_url() {
        let css = "div { background: url(../images/icon.svg); }";
        let result = rewrite_css(css, "OEBPS/css/style.css", |path| {
            Some(format!("blob:{path}"))
        });
        assert!(result.contains("url(\"blob:OEBPS/images/icon.svg\")"));
    }

    #[test]
    fn test_rewrite_css_skips_data_url() {
        let css = r#"div { background: url("data:image/png;base64,abc"); }"#;
        let result = rewrite_css(css, "OEBPS/css/style.css", |_| {
            panic!("resolver should not be called for data: URLs");
        });
        // data: URL must pass through unchanged
        assert!(result.contains("data:image/png;base64,abc"));
    }

    #[test]
    fn test_rewrite_css_skips_https_url() {
        let css = r#"div { background: url("https://example.com/img.png"); }"#;
        let result = rewrite_css(css, "OEBPS/css/style.css", |_| {
            panic!("resolver should not be called for https: URLs");
        });
        assert!(result.contains("https://example.com/img.png"));
    }

    #[test]
    fn test_rewrite_css_at_import_double_quote() {
        let css = r#"@import "base.css";"#;
        let result = rewrite_css(css, "OEBPS/css/style.css", |path| {
            assert_eq!(path, "OEBPS/css/base.css");
            Some("blob:base".to_string())
        });
        assert!(result.contains("@import \"blob:base\""));
    }

    #[test]
    fn test_rewrite_css_at_import_single_quote() {
        let css = "@import 'reset.css';";
        let result = rewrite_css(css, "OEBPS/css/style.css", |path| {
            assert_eq!(path, "OEBPS/css/reset.css");
            Some("blob:reset".to_string())
        });
        assert!(result.contains("@import \"blob:reset\""));
    }

    #[test]
    fn test_rewrite_css_skips_comment_embedded_url() {
        let css = "/* url(should-not-be-rewritten.ttf) */ body { color: red; }";
        let result = rewrite_css(css, "OEBPS/css/style.css", |_| {
            panic!("resolver should not be called for comment-embedded URLs");
        });
        assert!(result.contains("should-not-be-rewritten.ttf"));
    }

    #[test]
    fn test_rewrite_css_resolver_none_preserves_original() {
        let css = r#"body { background: url("img.png"); }"#;
        let result = rewrite_css(css, "OEBPS/css/style.css", |_| None);
        // resolver returned None → original must be kept
        assert!(result.contains("url(\"img.png\")"));
    }
}
