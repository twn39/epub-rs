//! Public CSS rewrite API for standalone stylesheets.
//!
//! Scanning and URL splicing live in [`super::rewrite`] so HTML rewriting can
//! share the same engine without depending on this module (or vice versa).

use super::rewrite::RewriteContext;

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
/// Delegates to [`RewriteContext`] (shared with HTML inline style rewriting).
pub fn rewrite_css<F>(css: &str, css_file_path: &str, resolver: F) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    RewriteContext::from_document_path(css_file_path).rewrite_css(css, resolver)
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
        assert!(result.contains("url(\"img.png\")"));
    }
}
