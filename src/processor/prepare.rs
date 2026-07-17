//! High-level chapter packaging for Web readers.
//!
//! Produces self-contained (or nearly self-contained) HTML by optionally:
//! - injecting `data-cfi` attributes
//! - rewriting relative resources to `data:` URIs (images, fonts, stylesheets)

use crate::error::EpubError;
use crate::path::{is_external_url, normalize_path};
use crate::processor::{inject_cfi_dom, rewrite_css, rewrite_resources};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

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

/// Guess a MIME type from an EPUB-internal path.
pub fn guess_media_type(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
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

/// Encode bytes as a `data:` URI.
pub fn data_uri(media_type: &str, bytes: &[u8]) -> String {
    format!("data:{media_type};base64,{}", B64.encode(bytes))
}

fn collect_local_refs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    // href / src attributes
    let attr_re =
        regex::Regex::new(r#"(?i)(?:src|href)\s*=\s*["']([^"']+)["']"#).expect("valid regex");
    for cap in attr_re.captures_iter(html) {
        out.push(cap[1].to_string());
    }
    // CSS url(...)
    let url_re = regex::Regex::new(r#"(?i)url\(\s*['"]?([^'")]+)['"]?\s*\)"#).expect("valid regex");
    for cap in url_re.captures_iter(html) {
        out.push(cap[1].to_string());
    }
    out
}

/// Prepare chapter HTML for offline / WKWebView-style embedding.
///
/// `base_cfi` is required when `options.inject_cfi` is true (spine base, e.g. `/6/4!`).
/// `chapter_path` is the EPUB-root-relative path of the chapter document (for URL join).
/// `load_resource` loads EPUB-root-relative paths and returns raw bytes.
pub fn prepare_chapter_html<F>(
    raw_html: &str,
    chapter_path: &str,
    base_cfi: Option<&str>,
    options: &PrepareChapterOptions,
    mut load_resource: F,
) -> Result<String, EpubError>
where
    F: FnMut(&str) -> Option<Vec<u8>>,
{
    let mut html = if options.inject_cfi {
        let base = base_cfi.ok_or_else(|| {
            EpubError::InvalidFormat("inject_cfi requires base_cfi".to_string())
        })?;
        inject_cfi_dom(raw_html, base)?
    } else {
        raw_html.to_string()
    };

    if !options.inline_resources {
        return Ok(html);
    }

    let base_dir = match chapter_path.rfind('/') {
        Some(i) => chapter_path[..i].to_string(),
        None => String::new(),
    };

    // Resolve and load every local ref we can find (including CSS-nested fonts/images).
    let mut pending: Vec<String> = collect_local_refs(&html)
        .into_iter()
        .filter(|r| !is_external_url(r) && !r.starts_with('#') && !r.is_empty())
        .map(|r| normalize_path(&base_dir, &r))
        .collect();

    let mut seen: HashSet<String> = HashSet::new();
    let mut uri_map: HashMap<String, String> = HashMap::new();

    while let Some(path) = pending.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let Some(bytes) = load_resource(&path) else {
            continue;
        };
        if bytes.len() > options.max_inline_bytes {
            continue;
        }
        let mt = guess_media_type(&path);
        if mt == "text/css" {
            let css = String::from_utf8_lossy(&bytes);
            let css_dir = match path.rfind('/') {
                Some(i) => &path[..i],
                None => "",
            };
            // Queue nested url() targets for loading.
            for rel in collect_local_refs(&format!("url({css})")) {
                if is_external_url(&rel) {
                    continue;
                }
                pending.push(normalize_path(css_dir, &rel));
            }
            let rewritten = rewrite_css(&css, &path, |inner| {
                if is_external_url(inner) {
                    return None;
                }
                let joined = normalize_path(css_dir, inner);
                uri_map.get(&joined).cloned()
            });
            // Nested deps may not be in map yet — second rewrite after loop is heavy;
            // rebuild CSS after all loads by re-running rewrite with full map.
            uri_map.insert(path, rewritten); // temporary plain CSS; fixed below
        } else if mt.starts_with("image/") || mt.starts_with("font/") || mt.starts_with("audio/") {
            uri_map.insert(path, data_uri(mt, &bytes));
        }
    }

    // Rebuild CSS entries now that fonts/images are materialised.
    let css_paths: Vec<String> = uri_map
        .keys()
        .filter(|p| guess_media_type(p) == "text/css")
        .cloned()
        .collect();
    for path in css_paths {
        let Some(bytes) = load_resource(&path) else {
            continue;
        };
        if bytes.len() > options.max_inline_bytes {
            continue;
        }
        let css = String::from_utf8_lossy(&bytes);
        let rewritten = rewrite_css(&css, &path, |inner| {
            if is_external_url(inner) {
                return None;
            }
            let css_dir = match path.rfind('/') {
                Some(i) => &path[..i],
                None => "",
            };
            let joined = normalize_path(css_dir, inner);
            uri_map.get(&joined).cloned()
        });
        uri_map.insert(path, data_uri("text/css", rewritten.as_bytes()));
    }

    let uri_map = Arc::new(uri_map);
    // `rewrite_resources` already joins relative refs against the chapter path;
    // the resolver receives EPUB-root-relative absolute paths.
    html = rewrite_resources(&html, chapter_path, {
        let uri_map = Arc::clone(&uri_map);
        move |abs_path| {
            if is_external_url(abs_path) || abs_path.starts_with('#') || abs_path.is_empty() {
                return None;
            }
            uri_map.get(abs_path).cloned()
        }
    })?;

    Ok(html)
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
                    Some(png.to_vec())
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
            |_| Some(b"x".to_vec()),
        )
        .unwrap();
        assert!(out.contains("src=\"a.png\""));
    }
}
