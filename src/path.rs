//! EPUB-internal path and href utilities.
//!
//! EPUB containers use forward-slash, root-relative paths (no leading `/`).
//! Call sites historically reimplemented RFC 3986-style joining in parser,
//! processor, and generator; a single policy here keeps fragment handling,
//! percent-decoding, and `..` traversal consistent across nav, rewrite, and
//! packaging.

/// Returns true when `url` is not an EPUB-internal relative path and must not
/// be joined against a base directory (schemes, protocol-relative, data URIs).
pub fn is_external_url(url: &str) -> bool {
    url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("data:")
        || url.starts_with("mailto:")
        || url.starts_with("ftp:")
        || url.starts_with("blob:")
        || url.starts_with("//")
}

/// Joins `base_dir` and a relative path, resolving `.` and `..` segments.
///
/// Both inputs are treated as EPUB-internal path strings (forward slashes).
/// Does **not** percent-decode and does **not** strip `?` / `#` — use
/// [`normalize_path`] when the input is a raw URL reference from HTML/CSS.
pub fn join_epub_path(base_dir: &str, rel_path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in base_dir.split('/').chain(rel_path.split('/')) {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            parts.pop();
        } else {
            parts.push(seg);
        }
    }
    parts.join("/")
}

/// Strip query string and fragment from a URL-like path reference.
fn strip_query_and_fragment(rel_path: &str) -> &str {
    let mut path_only = rel_path;
    if let Some(idx) = path_only.find('?') {
        path_only = &path_only[..idx];
    }
    if let Some(idx) = path_only.find('#') {
        path_only = &path_only[..idx];
    }
    path_only
}

/// Split `href` into `(path, fragment_including_hash)` where `fragment` is
/// either empty or starts with `#`.
fn split_fragment(href: &str) -> (&str, &str) {
    match href.find('#') {
        Some(i) => (&href[..i], &href[i..]),
        None => (href, ""),
    }
}

/// Normalizes a relative URL path against a base directory within the EPUB archive.
///
/// Percent-decodes the path (e.g. `%20` → space) and strips query strings /
/// fragments before joining.  The `base_dir` must be the **directory** of the
/// referencing file, not the file path itself.
///
/// This is the public API used by HTML/CSS resource rewriting and property tests.
pub fn normalize_path(base_dir: &str, rel_path: &str) -> String {
    let path_only = strip_query_and_fragment(rel_path);
    let decoded = percent_encoding::percent_decode_str(path_only).decode_utf8_lossy();
    join_epub_path(base_dir, decoded.as_ref())
}

/// Resolves `rel_href` against `base_dir` (EPUB-root-relative directory of the
/// referencing document), per RFC 3986 §5.2.
///
/// Preserves any `#fragment` suffix.  External URLs, fragment-only refs, empty
/// hrefs, and empty `base_dir` are returned unchanged so callers need not
/// pre-filter them (empty base means "at package root / unresolved context").
pub fn resolve_href(base_dir: &str, rel_href: &str) -> String {
    if rel_href.starts_with('#')
        || is_external_url(rel_href)
        || rel_href.is_empty()
        || base_dir.is_empty()
    {
        return rel_href.to_string();
    }

    // Fragment must stay verbatim after path resolution; folding it into the
    // path join would corrupt IDs that contain characters special to paths.
    let (path_part, fragment) = split_fragment(rel_href);
    format!("{}{fragment}", join_epub_path(base_dir, path_part))
}

/// Parent directory of an EPUB-internal file path, or `""` if the file is at root.
fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

/// File name component of an EPUB-internal path.
fn file_name(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// Relative path from one EPUB-internal file to another.
///
/// Both arguments are package-root-relative (no leading slash).  The result is
/// suitable for an HTML `href` / `src` (or CSS `url()`) written into `from_file`.
pub fn epub_relative_path(from_file: &str, to_file: &str) -> String {
    let from_dir = parent_dir(from_file);
    let to_dir = parent_dir(to_file);
    let to_name = file_name(to_file);

    let from_parts: Vec<&str> = from_dir.split('/').filter(|s| !s.is_empty()).collect();
    let to_parts: Vec<&str> = to_dir.split('/').filter(|s| !s.is_empty()).collect();

    let common_len = from_parts
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let up_steps = from_parts.len() - common_len;
    let down_parts = &to_parts[common_len..];

    let mut parts: Vec<&str> = Vec::with_capacity(up_steps + down_parts.len() + 1);
    parts.extend(std::iter::repeat_n("..", up_steps));
    parts.extend_from_slice(down_parts);
    parts.push(to_name);

    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── join_epub_path / normalize_path ───────────────────────────────────────

    #[test]
    fn join_parent_traversal() {
        assert_eq!(
            join_epub_path("OEBPS/Text", "../Images/cover.jpg"),
            "OEBPS/Images/cover.jpg"
        );
    }

    #[test]
    fn join_same_dir() {
        assert_eq!(
            join_epub_path("OEBPS/Text", "chapter2.xhtml"),
            "OEBPS/Text/chapter2.xhtml"
        );
    }

    #[test]
    fn join_empty_base() {
        assert_eq!(join_epub_path("", "images/cover.jpg"), "images/cover.jpg");
    }

    #[test]
    fn normalize_percent_decodes() {
        assert_eq!(
            normalize_path("OEBPS/Text", "../Images/my%20cover.jpg"),
            "OEBPS/Images/my cover.jpg"
        );
    }

    #[test]
    fn normalize_strips_query_and_fragment() {
        assert_eq!(
            normalize_path("OEBPS", "css/style.css?v=2.0#section"),
            "OEBPS/css/style.css"
        );
    }

    // ── resolve_href (nav / document-relative) ────────────────────────────────

    #[test]
    fn resolve_href_parent_traversal() {
        assert_eq!(
            resolve_href("OEBPS/nav", "../text/ch1.xhtml"),
            "OEBPS/text/ch1.xhtml"
        );
    }

    #[test]
    fn resolve_href_same_dir() {
        assert_eq!(
            resolve_href("OEBPS", "text/ch1.xhtml"),
            "OEBPS/text/ch1.xhtml"
        );
    }

    #[test]
    fn resolve_href_fragment_preserved() {
        assert_eq!(
            resolve_href("OEBPS/nav", "../text/ch2.xhtml#s2"),
            "OEBPS/text/ch2.xhtml#s2"
        );
    }

    #[test]
    fn resolve_href_absolute_url_passthrough() {
        assert_eq!(
            resolve_href("OEBPS", "https://example.com/page"),
            "https://example.com/page"
        );
    }

    #[test]
    fn resolve_href_empty_base_passthrough() {
        // Empty base means unresolved package context — keep href as authored.
        assert_eq!(resolve_href("", "text/ch1.xhtml"), "text/ch1.xhtml");
    }

    #[test]
    fn resolve_href_fragment_only() {
        assert_eq!(resolve_href("OEBPS", "#intro"), "#intro");
    }

    // ── epub_relative_path (generator) ────────────────────────────────────────

    #[test]
    fn relative_same_directory() {
        assert_eq!(
            epub_relative_path("text/ch1.xhtml", "text/ch2.xhtml"),
            "ch2.xhtml"
        );
    }

    #[test]
    fn relative_one_level_up() {
        assert_eq!(
            epub_relative_path("text/ch1.xhtml", "styles/main.css"),
            "../styles/main.css"
        );
    }

    #[test]
    fn relative_source_at_root() {
        assert_eq!(
            epub_relative_path("chapter.xhtml", "styles/main.css"),
            "styles/main.css"
        );
    }

    #[test]
    fn relative_target_at_root() {
        assert_eq!(
            epub_relative_path("text/ch1.xhtml", "cover.jpg"),
            "../cover.jpg"
        );
    }

    #[test]
    fn relative_both_at_root() {
        assert_eq!(
            epub_relative_path("chapter.xhtml", "cover.jpg"),
            "cover.jpg"
        );
    }

    #[test]
    fn relative_deeply_nested_source() {
        assert_eq!(
            epub_relative_path("a/b/ch1.xhtml", "styles/main.css"),
            "../../styles/main.css"
        );
    }

    #[test]
    fn relative_deeply_nested_both() {
        assert_eq!(
            epub_relative_path("text/part1/ch1.xhtml", "text/part2/ch2.xhtml"),
            "../part2/ch2.xhtml"
        );
    }

    #[test]
    fn relative_same_file() {
        assert_eq!(
            epub_relative_path("text/ch1.xhtml", "text/ch1.xhtml"),
            "ch1.xhtml"
        );
    }

    #[test]
    fn relative_theme_css_typical_cases() {
        let css = "styles/theme.css";
        assert_eq!(epub_relative_path("chapter.xhtml", css), "styles/theme.css");
        assert_eq!(
            epub_relative_path("text/ch1.xhtml", css),
            "../styles/theme.css"
        );
        assert_eq!(
            epub_relative_path("content/text/ch1.xhtml", css),
            "../../styles/theme.css"
        );
    }

    // ── Cross-module consistency (shared table) ───────────────────────────────

    #[test]
    fn join_and_resolve_agree_on_path_only_refs() {
        // When there is no fragment and base is non-empty, resolve_href path
        // component must match join_epub_path (nav vs resource lookup).
        let cases = [
            ("OEBPS/Text", "../Images/cover.jpg"),
            ("OEBPS/nav", "../text/ch1.xhtml"),
            ("OEBPS", "css/style.css"),
            ("a/b/c", "../../d/e.xhtml"),
            ("Text", "./ch1.xhtml"),
        ];
        for (base, rel) in cases {
            assert_eq!(
                join_epub_path(base, rel),
                resolve_href(base, rel),
                "join vs resolve for ({base}, {rel})"
            );
        }
    }

    /// Table: fragment handling differs by API contract.
    #[test]
    fn fragment_policy_table() {
        let cases = [
            // (base, input, normalize → strip, resolve → keep)
            (
                "OEBPS",
                "ch.xhtml#s1",
                "OEBPS/ch.xhtml",
                "OEBPS/ch.xhtml#s1",
            ),
            (
                "OEBPS/nav",
                "../text/ch2.xhtml#s2",
                "OEBPS/text/ch2.xhtml",
                "OEBPS/text/ch2.xhtml#s2",
            ),
            ("OEBPS", "#only", "OEBPS", "#only"),
            ("", "a.xhtml#x", "a.xhtml", "a.xhtml#x"),
        ];
        for (base, input, want_norm, want_resolve) in cases {
            assert_eq!(
                normalize_path(base, input),
                want_norm,
                "normalize({base}, {input})"
            );
            assert_eq!(
                resolve_href(base, input),
                want_resolve,
                "resolve({base}, {input})"
            );
        }
    }

    /// Table: query strings only affect normalize_path (stripped before join).
    #[test]
    fn query_and_encoding_table() {
        assert_eq!(
            normalize_path("OEBPS", "css/a.css?v=1#h"),
            "OEBPS/css/a.css"
        );
        assert_eq!(
            normalize_path("OEBPS/Text", "../Images/my%20cover.jpg"),
            "OEBPS/Images/my cover.jpg"
        );
        // join does not percent-decode
        assert_eq!(
            join_epub_path("OEBPS", "my%20file.xhtml"),
            "OEBPS/my%20file.xhtml"
        );
        // resolve does not percent-decode path segments
        assert_eq!(
            resolve_href("OEBPS", "my%20file.xhtml#id"),
            "OEBPS/my%20file.xhtml#id"
        );
    }

    #[test]
    fn external_and_special_schemes_table() {
        let externals = [
            "https://example.com/a",
            "http://example.com/a",
            "data:image/png;base64,xx",
            "mailto:a@b.c",
            "ftp://h/f",
            "blob:https://x/y",
            "//cdn.example/z",
        ];
        for url in externals {
            assert!(is_external_url(url), "{url}");
            assert_eq!(resolve_href("OEBPS", url), url);
        }
    }

    #[test]
    fn parent_traversal_and_dot_segments() {
        assert_eq!(join_epub_path("a/b/c", "../../x/y"), "a/x/y");
        assert_eq!(join_epub_path("a/b", "././c/./d"), "a/b/c/d");
        // Excess .. is popped until empty (no escape above package root)
        assert_eq!(join_epub_path("a", "../../x"), "x");
        assert_eq!(join_epub_path("", "../x"), "x");
    }

    #[test]
    fn empty_base_and_empty_href_contracts() {
        assert_eq!(resolve_href("", "text/ch1.xhtml"), "text/ch1.xhtml");
        assert_eq!(resolve_href("OEBPS", ""), "");
        assert_eq!(normalize_path("", "images/a.jpg"), "images/a.jpg");
        assert_eq!(normalize_path("OEBPS", ""), "OEBPS");
    }
}
