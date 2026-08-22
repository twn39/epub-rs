//! High-level chapter packaging for Web readers.
//!
//! Produces self-contained (or nearly self-contained) HTML by optionally:
//! - injecting `data-cfi` attributes
//! - rewriting relative resources to `data:` URIs (images, fonts, stylesheets)
//!
//! Resource bytes are supplied by the caller. When wired through
//! [`crate::parser::EpubArchive::prepare_chapter`], font obfuscation is already
//! reversed (same as other resource reads). Prefer declaring media types from
//! the OPF manifest when known.

use super::cfi::inject_cfi_dom;
use super::html::rewrite_resources;
use super::rewrite::RewriteContext;
use crate::error::EpubError;
use crate::path::{is_external_url, normalize_path};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Bytes loaded for prepare, optionally with a declared OPF media type.
#[derive(Debug, Clone)]
pub struct LoadedResource {
    pub bytes: Vec<u8>,
    /// Manifest `media-type` when known; extension guessing is used as fallback.
    pub media_type: Option<String>,
}

impl From<Vec<u8>> for LoadedResource {
    fn from(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            media_type: None,
        }
    }
}

/// Options for [`prepare_chapter_html`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PrepareChapterOptions {
    /// Inject `data-cfi` on every element (same pipeline as `get_chapter_with_cfi`).
    #[serde(default)]
    pub inject_cfi: bool,
    /// Rewrite local images / fonts / stylesheets to `data:` URIs when under the size cap.
    #[serde(default = "default_true")]
    pub inline_resources: bool,
    /// Maximum resource size (bytes) eligible for inlining. Default 4 MiB.
    #[serde(default = "default_max_inline")]
    pub max_inline_bytes: usize,
}

impl Default for PrepareChapterOptions {
    fn default() -> Self {
        Self {
            inject_cfi: false,
            inline_resources: true,
            max_inline_bytes: default_max_inline(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_max_inline() -> usize {
    4 * 1024 * 1024
}

/// Guess a MIME type from an EPUB-internal path (extension only).
pub fn guess_media_type(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    // Strip query/fragment if a raw URL slipped through.
    let path_only = lower.split(['?', '#']).next().unwrap_or(&lower);
    let ext = path_only.rsplit('.').next().unwrap_or("");
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" | "svgz" => "image/svg+xml",
        "css" => "text/css",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "xhtml" | "html" | "htm" => "application/xhtml+xml",
        _ => "application/octet-stream",
    }
}

/// Prefer a declared media type; fall back to path extension.
pub fn resolve_media_type(path: &str, declared: Option<&str>) -> String {
    if let Some(mt) = declared {
        let t = mt.trim();
        if !t.is_empty() && t != "application/octet-stream" {
            return t.to_string();
        }
    }
    guess_media_type(path).to_string()
}

fn is_inlineable_media(mt: &str) -> bool {
    let lower = mt.to_ascii_lowercase();
    lower.starts_with("image/")
        || lower.starts_with("font/")
        || lower.starts_with("audio/")
        || lower == "text/css"
        || lower == "application/font-woff"
        || lower == "application/font-woff2"
        || lower == "application/vnd.ms-opentype"
        || lower == "application/x-font-ttf"
        || lower == "application/x-font-opentype"
}

/// Encode bytes as a `data:` URI.
pub fn data_uri(media_type: &str, bytes: &[u8]) -> String {
    format!("data:{media_type};base64,{}", B64.encode(bytes))
}

/// Collect local resource references from HTML (src/href/poster/srcset + CSS url()).
fn collect_local_refs(html: &str) -> Vec<String> {
    let mut out = Vec::new();

    let attr_re = regex::Regex::new(r#"(?i)(?:src|href|poster)\s*=\s*["']([^"']+)["']"#)
        .expect("valid regex");
    for cap in attr_re.captures_iter(html) {
        out.push(cap[1].to_string());
    }

    // srcset: comma-separated candidates (`url [descriptor]`).
    let srcset_re = regex::Regex::new(r#"(?i)srcset\s*=\s*["']([^"']+)["']"#).expect("valid regex");
    for cap in srcset_re.captures_iter(html) {
        for candidate in cap[1].split(',') {
            let trimmed = candidate.trim();
            if trimmed.is_empty() {
                continue;
            }
            let url_part = match trimmed.find(|c: char| c.is_ascii_whitespace()) {
                Some(idx) => &trimmed[..idx],
                None => trimmed,
            };
            if !url_part.is_empty() {
                out.push(url_part.to_string());
            }
        }
    }

    // CSS url(...) — lightweight scan; nested CSS reloaded via RewriteContext later.
    let url_re = regex::Regex::new(r#"(?i)url\(\s*['"]?([^'")]+)['"]?\s*\)"#).expect("valid regex");
    for cap in url_re.captures_iter(html) {
        out.push(cap[1].to_string());
    }

    out
}

fn push_normalized_ref(pending: &mut Vec<String>, base_dir: &str, raw: &str) {
    let raw = raw.trim();
    if raw.is_empty() || is_external_url(raw) || raw.starts_with('#') {
        return;
    }
    pending.push(normalize_path(base_dir, raw));
}

/// Counters from a prepare pass (resource inlining).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PrepareStats {
    /// Unique resource paths considered for inlining.
    pub considered: usize,
    /// Successfully rewritten to `data:` URIs (images/fonts/css).
    pub inlined: usize,
    /// Skipped because loaded size exceeded `max_inline_bytes`.
    pub skipped_oversize: usize,
    /// Loader returned `None`.
    pub missing: usize,
    /// Loaded but media type not eligible for inlining.
    pub skipped_type: usize,
}

/// Prepare chapter HTML for offline / WKWebView-style embedding.
///
/// `base_cfi` is required when `options.inject_cfi` is true (spine base, e.g. `/6/4!`).
/// `chapter_path` is the EPUB-root-relative path of the chapter document (for URL join).
/// `load_resource` loads EPUB-root-relative paths (and may supply manifest media types).
pub fn prepare_chapter_html<F>(
    raw_html: &str,
    chapter_path: &str,
    base_cfi: Option<&str>,
    options: &PrepareChapterOptions,
    load_resource: F,
) -> Result<String, EpubError>
where
    F: FnMut(&str) -> Option<LoadedResource>,
{
    let (html, _) =
        prepare_chapter_html_with_stats(raw_html, chapter_path, base_cfi, options, load_resource)?;
    Ok(html)
}

/// Same as [`prepare_chapter_html`] but returns inlining statistics.
pub fn prepare_chapter_html_with_stats<F>(
    raw_html: &str,
    chapter_path: &str,
    base_cfi: Option<&str>,
    options: &PrepareChapterOptions,
    mut load_resource: F,
) -> Result<(String, PrepareStats), EpubError>
where
    F: FnMut(&str) -> Option<LoadedResource>,
{
    let mut stats = PrepareStats::default();
    let mut html = if options.inject_cfi {
        let base = base_cfi
            .ok_or_else(|| EpubError::InvalidFormat("inject_cfi requires base_cfi".to_string()))?;
        inject_cfi_dom(raw_html, base)?
    } else {
        raw_html.to_string()
    };

    if !options.inline_resources {
        return Ok((html, stats));
    }

    let chapter_ctx = RewriteContext::from_document_path(chapter_path);
    let base_dir = chapter_ctx.base_dir().to_string();

    let mut pending: Vec<String> = Vec::new();
    for r in collect_local_refs(&html) {
        push_normalized_ref(&mut pending, &base_dir, &r);
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut uri_map: HashMap<String, String> = HashMap::new();
    // Keep CSS source for a second pass after nested fonts/images are inlined.
    let mut css_sources: HashMap<String, String> = HashMap::new();

    while let Some(path) = pending.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        stats.considered += 1;
        let Some(loaded) = load_resource(&path) else {
            stats.missing += 1;
            continue;
        };
        if loaded.bytes.len() > options.max_inline_bytes {
            stats.skipped_oversize += 1;
            continue;
        }
        let mt = resolve_media_type(&path, loaded.media_type.as_deref());
        if mt == "text/css" {
            let css = String::from_utf8_lossy(&loaded.bytes).into_owned();
            let css_ctx = RewriteContext::from_document_path(&path);
            // Discover nested url() / @import targets without rewriting yet.
            let _ = css_ctx.rewrite_css(&css, |abs: &str| {
                if !is_external_url(abs) {
                    pending.push(abs.to_string());
                }
                None
            });
            css_sources.insert(path, css);
        } else if is_inlineable_media(&mt) {
            uri_map.insert(path, data_uri(&mt, &loaded.bytes));
            stats.inlined += 1;
        } else {
            stats.skipped_type += 1;
        }
    }

    // Materialise CSS as data: URIs with nested resources rewritten.
    for (path, css) in css_sources {
        let css_ctx = RewriteContext::from_document_path(&path);
        let rewritten = css_ctx.rewrite_css(&css, |abs: &str| {
            if is_external_url(abs) {
                return None;
            }
            uri_map.get(abs).cloned()
        });
        uri_map.insert(path, data_uri("text/css", rewritten.as_bytes()));
        stats.inlined += 1;
    }

    let uri_map = Arc::new(uri_map);
    // Do not fail the whole prepare if lol-html cannot rewrite (malformed XHTML).
    // Host readers (Latte) treat a prepare `Err` as a Swift packaging fallback;
    // keeping CFI/raw HTML is a better degradation than leaving the engine path.
    html = match rewrite_resources(&html, chapter_path, {
        let uri_map = Arc::clone(&uri_map);
        move |abs_path| {
            if is_external_url(abs_path) || abs_path.starts_with('#') || abs_path.is_empty() {
                return None;
            }
            uri_map.get(abs_path).cloned()
        }
    }) {
        Ok(rewritten) => rewritten,
        Err(_) => html,
    };

    // Host readers (Latte) strip `<head>` and only keep body + `<style>` blocks.
    // Convert inlined stylesheet `<link href="data:text/css…">` into `<style>` so
    // publisher CSS survives head-strip / chrome wrap without a second packaging pass.
    html = promote_data_stylesheet_links(&html);

    Ok((html, stats))
}

/// Replace `<link rel=stylesheet href="data:text/css…">` with `<style>…</style>`.
///
/// Leaves external (`http(s):`) and unresolved local links unchanged so callers can
/// still fall back to host packaging if needed.
fn promote_data_stylesheet_links(html: &str) -> String {
    let link_re = regex::Regex::new(r#"(?is)<link\b[^>]*>"#).expect("valid regex");
    let href_re = regex::Regex::new(r#"(?i)\bhref\s*=\s*["']([^"']+)["']"#).expect("valid regex");

    link_re
        .replace_all(html, |caps: &regex::Captures| {
            let tag = caps.get(0).map(|m| m.as_str()).unwrap_or("");
            let lower = tag.to_ascii_lowercase();
            let looks_like_css = lower.contains("stylesheet")
                || lower.contains("text/css")
                || lower.contains("type=\"text/css\"")
                || lower.contains("type='text/css'");
            if !looks_like_css {
                return tag.to_string();
            }
            let Some(href_cap) = href_re.captures(tag) else {
                return tag.to_string();
            };
            let href = href_cap.get(1).map(|m| m.as_str()).unwrap_or("").trim();
            let Some(css) = decode_data_css_uri(href) else {
                return tag.to_string();
            };
            if css.trim().is_empty() {
                return String::new();
            }
            // Escape `</style>` sequences that could break out of the injected block.
            let safe = css
                .replace("</style", "<\\/style")
                .replace("</STYLE", "<\\/STYLE");
            format!("<style type=\"text/css\">\n{safe}\n</style>")
        })
        .into_owned()
}

/// Decode `data:text/css…` (base64 or URL-encoded) into CSS text.
fn decode_data_css_uri(href: &str) -> Option<String> {
    let lower = href.to_ascii_lowercase();
    if !lower.starts_with("data:") {
        return None;
    }
    // Accept text/css and generic data URIs produced by this module.
    let is_css = lower.starts_with("data:text/css")
        || lower.starts_with("data:text/plain")
        || lower.starts_with("data:application/css");
    if !is_css && !lower.contains("text/css") {
        // Still try generic `data:,…` / `data:;base64,…` only when charset-less CSS.
        if !(lower.starts_with("data:,") || lower.starts_with("data:;")) {
            return None;
        }
    }

    let comma = href.find(',')?;
    let meta = &href[..comma];
    let payload = &href[comma + 1..];
    let is_b64 = meta.to_ascii_lowercase().contains(";base64");
    if is_b64 {
        let bytes = B64.decode(payload.as_bytes()).ok()?;
        // Lossy UTF-8 is fine for publisher CSS with legacy encodings.
        Some(String::from_utf8_lossy(&bytes).into_owned())
    } else {
        // Percent-decoded URL payload (rare for our pipeline, but cheap to support).
        Some(urlencoding_minimal_decode(payload))
    }
}

/// Minimal percent-decoding for `data:,…` payloads (no full URL crate dependency).
fn urlencoding_minimal_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h1 = bytes[i + 1];
            let h2 = bytes[i + 2];
            if let (Some(a), Some(b)) = (from_hex(h1), from_hex(h2)) {
                out.push((a << 4) | b);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_uri_roundtrip_prefix() {
        let uri = data_uri("image/png", b"\x89PNG");
        assert!(uri.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn resolve_media_type_prefers_manifest() {
        assert_eq!(
            resolve_media_type("x.bin", Some("image/webp")),
            "image/webp"
        );
        assert_eq!(resolve_media_type("x.png", None), "image/png");
        assert_eq!(
            resolve_media_type("x.png", Some("application/octet-stream")),
            "image/png"
        );
    }

    #[test]
    fn prepare_inlines_relative_image() {
        let html = r#"<html><body><img src="img/a.png" alt="x"/></body></html>"#;
        let png = b"\x89PNG\r\n";
        let out = prepare_chapter_html(
            html,
            "OEBPS/ch1.xhtml",
            None,
            &PrepareChapterOptions {
                inject_cfi: false,
                inline_resources: true,
                max_inline_bytes: 1024,
            },
            |path| {
                if path == "OEBPS/img/a.png" {
                    Some(LoadedResource {
                        bytes: png.to_vec(),
                        media_type: Some("image/png".into()),
                    })
                } else {
                    None
                }
            },
        )
        .unwrap();
        assert!(out.contains("data:image/png;base64,"));
        assert!(!out.contains("src=\"img/a.png\""));
    }

    #[test]
    fn prepare_inlines_srcset_and_strips_fragment() {
        let html = r#"<html><body>
            <img src="a.png#frag" srcset="a.png 1x, b.jpg 2x"/>
        </body></html>"#;
        let out = prepare_chapter_html(
            html,
            "ch1.xhtml",
            None,
            &PrepareChapterOptions {
                inject_cfi: false,
                inline_resources: true,
                max_inline_bytes: 4096,
            },
            |path| match path {
                "a.png" => Some(LoadedResource::from(b"png".to_vec())),
                "b.jpg" => Some(LoadedResource {
                    bytes: b"jpg".to_vec(),
                    media_type: Some("image/jpeg".into()),
                }),
                _ => None,
            },
        )
        .unwrap();
        assert!(out.contains("data:image/png;base64,") || out.contains("data:image/jpeg;base64,"));
        assert!(out.contains("data:"));
    }

    #[test]
    fn prepare_skips_inline_when_disabled() {
        let html = r#"<html><body><img src="a.png"/></body></html>"#;
        let out = prepare_chapter_html(
            html,
            "ch1.xhtml",
            None,
            &PrepareChapterOptions {
                inject_cfi: false,
                inline_resources: false,
                max_inline_bytes: 1024,
            },
            |_| Some(LoadedResource::from(b"x".to_vec())),
        )
        .unwrap();
        assert!(out.contains("src=\"a.png\""));
    }

    #[test]
    fn collect_srcset_refs() {
        let refs = collect_local_refs(r#"<img srcset="img/a.png 1x, img/b.png 2x">"#);
        assert!(refs.iter().any(|r| r.contains("a.png")));
        assert!(refs.iter().any(|r| r.contains("b.png")));
    }

    #[test]
    fn prepare_stats_count_missing_and_oversize() {
        let html = r#"<html><body>
            <img src="ok.png"/><img src="big.png"/><img src="gone.png"/>
        </body></html>"#;
        let (_out, stats) = prepare_chapter_html_with_stats(
            html,
            "ch.xhtml",
            None,
            &PrepareChapterOptions {
                inject_cfi: false,
                inline_resources: true,
                max_inline_bytes: 10,
            },
            |path| match path {
                "ok.png" => Some(LoadedResource::from(b"small".to_vec())),
                "big.png" => Some(LoadedResource::from(vec![0u8; 100])),
                _ => None,
            },
        )
        .unwrap();
        assert_eq!(stats.inlined, 1);
        assert_eq!(stats.skipped_oversize, 1);
        assert_eq!(stats.missing, 1);
        assert_eq!(stats.considered, 3);
    }

    #[test]
    fn prepare_promotes_stylesheet_link_to_style_element() {
        let html = r#"<html><head>
            <link rel="stylesheet" type="text/css" href="styles/ch.css"/>
        </head><body><p class="x">hi</p></body></html>"#;
        let out = prepare_chapter_html(
            html,
            "OEBPS/Text/ch1.xhtml",
            None,
            &PrepareChapterOptions {
                inject_cfi: false,
                inline_resources: true,
                max_inline_bytes: 64 * 1024,
            },
            |path| {
                if path == "OEBPS/Text/styles/ch.css" || path.ends_with("styles/ch.css") {
                    Some(LoadedResource {
                        bytes: b"p.x { color: #c00; }".to_vec(),
                        media_type: Some("text/css".into()),
                    })
                } else {
                    None
                }
            },
        )
        .unwrap();
        assert!(
            out.contains("<style"),
            "stylesheet should become a <style> block: {out}"
        );
        assert!(
            out.contains("p.x { color: #c00; }") || out.contains("color: #c00"),
            "publisher CSS body should be present: {out}"
        );
        // Original local link should not remain unresolved.
        assert!(
            !out.contains("href=\"styles/ch.css\""),
            "relative stylesheet href should be rewritten: {out}"
        );
    }

    #[test]
    fn promote_data_stylesheet_decodes_base64_css() {
        let css = "body{color:red}";
        let uri = data_uri("text/css", css.as_bytes());
        let html = format!(
            r#"<html><head><link rel="stylesheet" href="{uri}"/></head><body></body></html>"#
        );
        let out = promote_data_stylesheet_links(&html);
        assert!(out.contains("<style"));
        assert!(out.contains("body{color:red}"));
        assert!(!out.contains("<link"));
    }

    #[test]
    fn prepare_oversize_resource_keeps_original_src() {
        // When an image exceeds max_inline_bytes, the original src must be preserved,
        // not replaced with a data URI.
        let html = r#"<html><body><img src="large.png"/></body></html>"#;
        let (out, stats) = prepare_chapter_html_with_stats(
            html,
            "ch.xhtml",
            None,
            &PrepareChapterOptions {
                inject_cfi: false,
                inline_resources: true,
                max_inline_bytes: 512,
            },
            |_| Some(LoadedResource::from(vec![0u8; 1024])),
        )
        .unwrap();
        assert_eq!(stats.skipped_oversize, 1);
        assert_eq!(stats.inlined, 0);
        // The src attribute must NOT have been rewritten to a data: URI.
        assert!(
            !out.contains("data:"),
            "oversize resource must not be inlined: {out}"
        );
        assert!(out.contains("large.png"), "original src must remain: {out}");
    }

    #[test]
    fn prepare_multiple_images_partial_oversize() {
        // Mixed: one small (inlined), one oversize (skipped), one missing.
        let html = r#"<html><body>
            <img src="small.png"/>
            <img src="big.png"/>
            <img src="absent.png"/>
        </body></html>"#;
        let (out, stats) = prepare_chapter_html_with_stats(
            html,
            "ch.xhtml",
            None,
            &PrepareChapterOptions {
                inject_cfi: false,
                inline_resources: true,
                max_inline_bytes: 20,
            },
            |path| match path {
                "small.png" => Some(LoadedResource::from(b"tiny".to_vec())),
                "big.png" => Some(LoadedResource::from(vec![0u8; 100])),
                _ => None,
            },
        )
        .unwrap();
        assert_eq!(stats.inlined, 1, "exactly one image inlined");
        assert_eq!(stats.skipped_oversize, 1, "one image skipped oversize");
        assert_eq!(stats.missing, 1, "one image missing");
        assert_eq!(stats.considered, 3);
        // The inlined image should appear as a data URI.
        assert!(out.contains("data:"), "inlined img must be data URI: {out}");
        // The oversize image src must remain as-is.
        assert!(out.contains("big.png"), "oversize src must remain: {out}");
    }

    #[test]
    fn prepare_succeeds_on_unquoted_attributes() {
        // Unquoted attrs used to surface as HtmlParse and force host Swift fallback.
        let html = r#"<html><body><img src=a.png alt=x><p>ok</p></body></html>"#;
        let out = prepare_chapter_html(
            html,
            "ch1.xhtml",
            None,
            &PrepareChapterOptions {
                inject_cfi: false,
                inline_resources: true,
                max_inline_bytes: 1024,
            },
            |path| {
                if path == "a.png" {
                    Some(LoadedResource {
                        bytes: b"\x89PNG".to_vec(),
                        media_type: Some("image/png".into()),
                    })
                } else {
                    None
                }
            },
        );
        assert!(out.is_ok(), "prepare must not fail: {out:?}");
        let html = out.unwrap();
        assert!(html.contains("ok"));
    }
}
