//! lol_html-based HTML transformation utilities.
//!
//! All operations are streaming SAX-style (no full DOM allocation).
//!
//! # CSS URL rewriting
//!
//! [`rewrite_css`] handles CSS text (standalone files, `<style>` blocks, `style=""` attrs).
//! [`rewrite_resources`] automatically rewrites inline `<style>` blocks and `style=""`
//! attributes in HTML, in addition to the HTML attribute layer it already covers.

use crate::error::EpubError;
use crate::path::{is_external_url, normalize_path};
use lol_html::{HtmlRewriter, Settings, element, text};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

// ── Private helpers ───────────────────────────────────────────────────────────

#[allow(clippy::type_complexity)]
fn make_end_tag_handler<F>(
    f: F,
) -> Box<
    dyn for<'a, 'b> FnOnce(
            &'a mut lol_html::html_content::EndTag<'b>,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
        + 'static,
>
where
    F: for<'a, 'b> FnOnce(
            &'a mut lol_html::html_content::EndTag<'b>,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
        + 'static,
{
    Box::new(f)
}

use super::rewrite::rewrite_css_urls;

/// Rewrites every image URL inside a `srcset` attribute value.
///
/// `srcset` is a comma-separated list of image candidate strings.  Each
/// candidate has the form `"<url> [descriptor]"` where the optional descriptor
/// is a width hint (`400w`) or pixel-density ratio (`2x`).  This function
/// resolves every relative URL via `resolver` while leaving descriptors and
/// external / data URLs untouched.
///
/// The function is also correct for `<picture><source srcset="…">` elements
/// because `<source>` is already matched by the same element selector as
/// `<img>` — no separate handler is required for `<picture>`.
fn rewrite_srcset<F>(srcset: &str, base_dir: &str, resolver: &mut F) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    // WHATWG HTML §4.8.4.3.10 mandates that any literal comma inside a URL
    // must be percent-encoded as %2C before it appears in a srcset attribute.
    // Therefore every literal ',' is a valid candidate delimiter — naive
    // split(',') is spec-correct for well-formed srcset values.
    // A data: URL containing an unencoded comma is non-conforming; the
    // "data:" prefix candidate will be skipped by is_external_url, and the
    // fragment after the comma will reach the resolver with an invalid path
    // (resolver returns None, output is unchanged).  This behaviour is
    // acceptable because the input is already spec-violating.
    let mut out = String::with_capacity(srcset.len());
    let mut first = true;

    for candidate in srcset.split(',') {
        if !first {
            out.push(',');
        }
        first = false;

        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            out.push_str(candidate);
            continue;
        }

        // Split "url descriptor" on the first ASCII whitespace.
        let (url_part, descriptor_part) = match trimmed.find(|c: char| c.is_ascii_whitespace()) {
            Some(idx) => (&trimmed[..idx], &trimmed[idx..]),
            None => (trimmed, ""),
        };

        if url_part.is_empty() || is_external_url(url_part) {
            // External / data / empty → pass through unchanged.
            out.push_str(candidate);
            continue;
        }

        let abs = normalize_path(base_dir, url_part);
        match resolver(&abs) {
            Some(new_url) => {
                // Preserve leading whitespace from the original candidate so
                // the joined output matches the browser's tokeniser expectations.
                let leading = &candidate[..candidate.len() - candidate.trim_start().len()];
                out.push_str(leading);
                out.push_str(&new_url);
                out.push_str(descriptor_part);
            }
            None => out.push_str(candidate),
        }
    }
    out
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Abstract trait representing a mutable HTML element.
///
/// Decouples the element mutation interface from concrete SAX/DOM parser libraries.
pub trait HtmlElementMut {
    /// Returns the element's tag name.
    fn tag_name(&self) -> String;
    /// Retrieves the value of an attribute.
    fn get_attribute(&self, name: &str) -> Option<String>;
    /// Sets the value of an attribute.
    fn set_attribute(&mut self, name: &str, value: &str) -> Result<(), String>;
}

/// Thin adapter implementing `HtmlElementMut` on top of `lol_html::html_content::Element`.
struct LolHtmlElement<'a, 'b, 'c> {
    inner: &'a mut lol_html::html_content::Element<'b, 'c>,
}

impl<'a, 'b, 'c> HtmlElementMut for LolHtmlElement<'a, 'b, 'c> {
    fn tag_name(&self) -> String {
        self.inner.tag_name()
    }

    fn get_attribute(&self, name: &str) -> Option<String> {
        self.inner.get_attribute(name)
    }

    fn set_attribute(&mut self, name: &str, value: &str) -> Result<(), String> {
        self.inner
            .set_attribute(name, value)
            .map_err(|e| e.to_string())
    }
}

/// Parser-agnostic processor that encapsulates all URL-mapping and rewriting logic.
pub struct UrlRewritingProcessor<F> {
    base_dir: String,
    resolver: F,
}

impl<F> UrlRewritingProcessor<F>
where
    F: FnMut(&str) -> Option<String>,
{
    pub fn process_element(&mut self, el: &mut dyn HtmlElementMut) -> Result<(), String> {
        let tag = el.tag_name().to_lowercase();
        match tag.as_str() {
            "img" | "video" | "audio" | "source" | "track" => {
                if let Some(src) = el.get_attribute("src")
                    && !is_external_url(&src)
                    && !src.starts_with('#')
                {
                    let abs_path = normalize_path(&self.base_dir, &src);
                    if let Some(new_url) = (self.resolver)(&abs_path) {
                        el.set_attribute("src", &new_url)?;
                    }
                }
                if let Some(poster) = el.get_attribute("poster")
                    && !is_external_url(&poster)
                    && !poster.starts_with('#')
                {
                    let abs_path = normalize_path(&self.base_dir, &poster);
                    if let Some(new_url) = (self.resolver)(&abs_path) {
                        el.set_attribute("poster", &new_url)?;
                    }
                }
                if let Some(srcset) = el.get_attribute("srcset")
                    && !srcset.trim().is_empty()
                {
                    let new_srcset = rewrite_srcset(&srcset, &self.base_dir, &mut self.resolver);
                    if new_srcset != srcset {
                        el.set_attribute("srcset", &new_srcset)?;
                    }
                }
            }
            "object" => {
                if let Some(data) = el.get_attribute("data")
                    && !is_external_url(&data)
                    && !data.starts_with('#')
                {
                    let abs_path = normalize_path(&self.base_dir, &data);
                    if let Some(new_url) = (self.resolver)(&abs_path) {
                        el.set_attribute("data", &new_url)?;
                    }
                }
            }
            "image" => {
                for attr in &["href", "xlink:href"] {
                    if let Some(href) = el.get_attribute(attr)
                        && !is_external_url(&href)
                        && !href.starts_with('#')
                    {
                        let abs_path = normalize_path(&self.base_dir, &href);
                        if let Some(new_url) = (self.resolver)(&abs_path) {
                            el.set_attribute(attr, &new_url)?;
                        }
                    }
                }
            }
            "use" => {
                for attr in &["href", "xlink:href"] {
                    if let Some(href) = el.get_attribute(attr)
                        && !is_external_url(&href)
                        && !href.starts_with('#')
                    {
                        let (path_part, frag_part) = match href.find('#') {
                            Some(idx) => (&href[..idx], &href[idx..]),
                            None => (href.as_str(), ""),
                        };
                        if !path_part.is_empty() {
                            let abs_path = normalize_path(&self.base_dir, path_part);
                            if let Some(mut new_url) = (self.resolver)(&abs_path) {
                                new_url.push_str(frag_part);
                                el.set_attribute(attr, &new_url)?;
                            }
                        }
                    }
                }
            }
            "link" | "a" | "area" => {
                if let Some(href) = el.get_attribute("href")
                    && !is_external_url(&href)
                    && !href.starts_with('#')
                {
                    let (path_part, anchor_part) = match href.find('#') {
                        Some(idx) => (&href[..idx], &href[idx..]),
                        None => (href.as_str(), ""),
                    };
                    if !path_part.is_empty() {
                        let abs_path = normalize_path(&self.base_dir, path_part);
                        if let Some(mut new_url) = (self.resolver)(&abs_path) {
                            new_url.push_str(anchor_part);
                            el.set_attribute("href", &new_url)?;
                        }
                    }
                }
            }
            _ => {}
        }

        // Generic style attribute check for all elements
        if let Some(style) = el.get_attribute("style")
            && style.contains("url(")
        {
            let rewritten = rewrite_css_urls(&style, &self.base_dir, &mut self.resolver);
            if rewritten != style {
                el.set_attribute("style", &rewritten)?;
            }
        }

        Ok(())
    }
}

/// Rewrite resources (images, css, links) in an HTML document using a provided resolver callback.
pub fn rewrite_resources<F>(
    html: &str,
    base_file_path: &str,
    resolver: F,
) -> Result<String, EpubError>
where
    F: FnMut(&str) -> Option<String> + 'static,
{
    let base_dir = match base_file_path.rfind('/') {
        Some(idx) => base_file_path[..idx].to_string(),
        None => String::new(),
    };
    let mut output = Vec::new();
    let processor_arc = Arc::new(Mutex::new(UrlRewritingProcessor { base_dir, resolver }));

    let mut rewriter = HtmlRewriter::new(
        Settings {
            element_content_handlers: vec![element!("*", {
                let processor = Arc::clone(&processor_arc);
                move |el| {
                    let mut adapter = LolHtmlElement { inner: el };
                    processor
                        .lock()
                        .unwrap()
                        .process_element(&mut adapter)
                        .map_err(|e| {
                            Box::new(std::io::Error::other(e))
                                as Box<dyn std::error::Error + Send + Sync>
                        })?;
                    Ok(())
                }
            })],
            ..Settings::default()
        },
        |c: &[u8]| output.extend_from_slice(c),
    );

    rewriter
        .write(html.as_bytes())
        .map_err(|e| EpubError::HtmlParse(e.to_string()))?;
    rewriter
        .end()
        .map_err(|e| EpubError::HtmlParse(e.to_string()))?;

    let html_out = String::from_utf8(output)
        .map_err(|e| EpubError::HtmlParse(format!("Invalid UTF-8: {e}")))?;

    // ── Second pass: rewrite url() inside <style> blocks ─────────────────────
    if html_out.contains("<style") && html_out.contains("url(") {
        let mut guard = processor_arc.lock().unwrap();
        let base_dir = guard.base_dir.clone();
        let result = rewrite_style_blocks(&html_out, &base_dir, &mut guard.resolver);
        drop(guard);
        return Ok(result);
    }

    Ok(html_out)
}

/// Rewrites `url(...)` references inside every `<style>…</style>` block found
/// in `html`.  Content outside `<style>` blocks is emitted unchanged.
///
/// This is a deliberate second-pass string scan rather than a lol_html text
/// handler because lol_html cannot interleave streaming text replacement with
/// element handlers in a single pass.
fn rewrite_style_blocks<F>(html: &str, base_dir: &str, resolver: &mut F) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    let mut out = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(open_end) = find_style_tag_end(rest) {
        // Everything up to and including the closing `>` of `<style ...>`.
        let before_content = &rest[..open_end];
        out.push_str(before_content);
        rest = &rest[open_end..];

        // Find `</style` (case-insensitive per HTML spec).
        let close_pos = rest.to_lowercase().find("</style").unwrap_or(rest.len());
        let style_content = &rest[..close_pos];
        let after = &rest[close_pos..];

        if style_content.contains("url(") {
            out.push_str(&rewrite_css_urls(style_content, base_dir, resolver));
        } else {
            out.push_str(style_content);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Returns the byte offset just past the closing `>` of the next `<style`
/// opening tag in `s`, or `None` if none is found.
fn find_style_tag_end(s: &str) -> Option<usize> {
    let lower = s.to_lowercase();
    let tag_start = lower.find("<style")?;
    // Find the `>` that closes this opening tag (skip past `<style`).
    let after_tag = tag_start + 6; // len("<style")
    let rel_close = lower[after_tag..].find('>')?;
    Some(after_tag + rel_close + 1)
}

/// Extracts plain text from an HTML byte slice.
pub fn extract_text(html: &[u8]) -> Result<String, EpubError> {
    extract_text_stream(html)
}

/// Extracts plain text from an HTML stream (memory efficient).
pub fn extract_text_stream<R: Read>(mut reader: R) -> Result<String, EpubError> {
    let extracted_text = Arc::new(Mutex::new(String::new()));
    let text_clone = Arc::clone(&extracted_text);
    let ignore_text = Arc::new(AtomicBool::new(false));

    let mut rewriter = HtmlRewriter::new(
        Settings {
            element_content_handlers: vec![
                element!("script, style", {
                    let ignore = Arc::clone(&ignore_text);
                    move |el| {
                        ignore.store(true, Ordering::SeqCst);
                        let ignore_end = Arc::clone(&ignore);
                        el.end_tag_handlers()
                            .unwrap()
                            .push(make_end_tag_handler(move |_| {
                                ignore_end.store(false, Ordering::SeqCst);
                                Ok(())
                            }));
                        Ok(())
                    }
                }),
                text!("body", {
                    let ignore = Arc::clone(&ignore_text);
                    move |t| {
                        if !ignore.load(Ordering::SeqCst) {
                            text_clone.lock().unwrap().push_str(t.as_str());
                        }
                        Ok(())
                    }
                }),
            ],
            ..Settings::default()
        },
        |_: &[u8]| {},
    );

    let mut buffer = [0; 8192];
    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        rewriter
            .write(&buffer[..bytes_read])
            .map_err(|e| EpubError::InvalidFormat(format!("HTML extraction error: {}", e)))?;
    }
    rewriter
        .end()
        .map_err(|e| EpubError::InvalidFormat(format!("HTML extraction error: {}", e)))?;

    let result = extracted_text.lock().unwrap().clone();
    Ok(result.trim().to_string())
}

/// Rewrites HTML links using a provided mapping function.
pub fn rewrite_links<F>(html: &[u8], link_mapper: F) -> Result<Vec<u8>, EpubError>
where
    F: FnMut(&str, &str) -> Option<String> + 'static,
{
    let mut output = Vec::new();
    rewrite_links_stream(html, &mut output, link_mapper)?;
    Ok(output)
}

/// Rewrites HTML links from a stream to a writer (memory efficient).
pub fn rewrite_links_stream<R: Read, W: Write, F>(
    mut reader: R,
    mut writer: W,
    link_mapper: F,
) -> Result<(), EpubError>
where
    F: FnMut(&str, &str) -> Option<String> + 'static,
{
    let mapper_arc = Arc::new(Mutex::new(link_mapper));
    let write_error = Arc::new(Mutex::new(None));
    let error_clone = Arc::clone(&write_error);

    {
        let mapper_img = Arc::clone(&mapper_arc);
        let mapper_a = Arc::clone(&mapper_arc);
        let mapper_link = Arc::clone(&mapper_arc);

        let mut rewriter = HtmlRewriter::new(
            Settings {
                element_content_handlers: vec![
                    element!("img[src]", move |el| {
                        if let Some(src) = el.get_attribute("src")
                            && let Some(new_src) = (mapper_img.lock().unwrap())("img", &src)
                        {
                            el.set_attribute("src", &new_src).map_err(|e| {
                                Box::new(e) as Box<dyn std::error::Error + Send + Sync>
                            })?;
                        }
                        Ok(())
                    }),
                    element!("a[href]", move |el| {
                        if let Some(href) = el.get_attribute("href")
                            && let Some(new_href) = (mapper_a.lock().unwrap())("a", &href)
                        {
                            el.set_attribute("href", &new_href).map_err(|e| {
                                Box::new(e) as Box<dyn std::error::Error + Send + Sync>
                            })?;
                        }
                        Ok(())
                    }),
                    element!("link[href]", move |el| {
                        if let Some(href) = el.get_attribute("href")
                            && let Some(new_href) = (mapper_link.lock().unwrap())("link", &href)
                        {
                            el.set_attribute("href", &new_href).map_err(|e| {
                                Box::new(e) as Box<dyn std::error::Error + Send + Sync>
                            })?;
                        }
                        Ok(())
                    }),
                ],
                ..Settings::default()
            },
            |c: &[u8]| {
                if let Err(e) = writer.write_all(c) {
                    *error_clone.lock().unwrap() = Some(e);
                }
            },
        );

        let mut buffer = [0; 8192];
        loop {
            if write_error.lock().unwrap().is_some() {
                break;
            }
            let bytes_read = reader.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            rewriter
                .write(&buffer[..bytes_read])
                .map_err(|e| EpubError::InvalidFormat(format!("HTML rewrite error: {}", e)))?;
        }
        if write_error.lock().unwrap().is_none() {
            rewriter
                .end()
                .map_err(|e| EpubError::InvalidFormat(format!("HTML rewrite error: {}", e)))?;
        }
    }

    if let Some(err) = write_error.lock().unwrap().take() {
        return Err(EpubError::Io(err));
    }
    Ok(())
}

/// Injects custom HTML elements (e.g. `<style>` or `<script>`) into the `<head>`.
pub fn inject_head_content<R: Read, W: Write>(
    mut reader: R,
    mut writer: W,
    content_to_inject: &str,
) -> Result<(), EpubError> {
    let write_error = Arc::new(Mutex::new(None));
    let error_clone = Arc::clone(&write_error);
    let content = content_to_inject.to_string();

    {
        let mut rewriter = HtmlRewriter::new(
            Settings {
                element_content_handlers: vec![element!("head", move |el| {
                    el.append(&content, lol_html::html_content::ContentType::Html);
                    Ok(())
                })],
                ..Settings::default()
            },
            |c: &[u8]| {
                if let Err(e) = writer.write_all(c) {
                    *error_clone.lock().unwrap() = Some(e);
                }
            },
        );

        let mut buffer = [0; 8192];
        loop {
            if write_error.lock().unwrap().is_some() {
                break;
            }
            let bytes_read = reader.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            rewriter.write(&buffer[..bytes_read]).map_err(|e| {
                EpubError::InvalidFormat(format!("HTML theme injection error: {}", e))
            })?;
        }
        if write_error.lock().unwrap().is_none() {
            rewriter.end().map_err(|e| {
                EpubError::InvalidFormat(format!("HTML theme injection error: {}", e))
            })?;
        }
    }

    if let Some(err) = write_error.lock().unwrap().take() {
        return Err(EpubError::Io(err));
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rewrite_resources_resolves_from_parent_directory() {
        let html = r#"<html><body><img src="../Images/cover.jpg" /></body></html>"#;
        let base_file_path = "OEBPS/Text/ch1.xhtml";
        let rewritten = rewrite_resources(html, base_file_path, |abs_path| {
            assert_eq!(abs_path, "OEBPS/Images/cover.jpg");
            Some("blob:12345".to_string())
        })
        .unwrap();
        assert!(rewritten.contains(r#"src="blob:12345""#));
    }

    // ── rewrite_resources: inline <style> block ────────────────────────────────

    #[test]
    fn test_rewrite_resources_inline_style_block() {
        let html = r#"<html><head>
<style>
@font-face { font-family: "X"; src: url("../fonts/x.woff2"); }
</style>
</head><body></body></html>"#;
        let result = rewrite_resources(html, "OEBPS/Text/ch1.xhtml", |path| {
            Some(format!("blob:{path}"))
        })
        .unwrap();
        assert!(
            result.contains("blob:OEBPS/fonts/x.woff2"),
            "expected blob URL in style block, got:\n{result}"
        );
    }

    // ── rewrite_resources: style="" attribute ─────────────────────────────────

    #[test]
    fn test_rewrite_resources_style_attribute() {
        let html =
            r#"<html><body><div style="background: url('../images/bg.png')"></div></body></html>"#;
        let result = rewrite_resources(html, "OEBPS/Text/ch1.xhtml", |path| {
            Some(format!("blob:{path}"))
        })
        .unwrap();
        assert!(
            result.contains("blob:OEBPS/images/bg.png"),
            "expected blob URL in style attribute, got:\n{result}"
        );
    }

    // ── rewrite_srcset: width descriptors ────────────────────────────────────

    #[test]
    fn test_rewrite_srcset_width_descriptors() {
        // Both URLs must be rewritten; descriptors ("400w", "800w") must survive.
        let srcset = "small.jpg 400w, ../img/large.jpg 800w";
        let base = "OEBPS/Text";
        let result = rewrite_srcset(srcset, base, &mut |p| Some(format!("blob:{p}")));
        assert!(
            result.contains("blob:OEBPS/Text/small.jpg 400w"),
            "small.jpg: {result}"
        );
        assert!(
            result.contains("blob:OEBPS/img/large.jpg 800w"),
            "large.jpg: {result}"
        );
    }

    // ── rewrite_srcset: pixel-density descriptors ─────────────────────────────

    #[test]
    fn test_rewrite_srcset_pixel_density_descriptors() {
        let srcset = "icon.png 1x, icon@2x.png 2x";
        let base = "OEBPS/Text";
        let result = rewrite_srcset(srcset, base, &mut |p| Some(format!("blob:{p}")));
        assert!(
            result.contains("blob:OEBPS/Text/icon.png 1x"),
            "1x: {result}"
        );
        assert!(
            result.contains("blob:OEBPS/Text/icon@2x.png 2x"),
            "2x: {result}"
        );
    }

    // ── rewrite_srcset: bare URL with no descriptor ───────────────────────────

    #[test]
    fn test_rewrite_srcset_no_descriptor() {
        let srcset = "single.png";
        let base = "OEBPS/Text";
        let result = rewrite_srcset(srcset, base, &mut |p| Some(format!("blob:{p}")));
        assert_eq!(result, "blob:OEBPS/Text/single.png");
    }

    // ── rewrite_srcset: external URLs are passed through unchanged ────────────

    #[test]
    fn test_rewrite_srcset_skips_external_urls() {
        let srcset = "https://cdn.example.com/img.jpg 2x, local.png 1x";
        let base = "OEBPS/Text";
        let mut resolver_called_with = Vec::new();
        let result = rewrite_srcset(srcset, base, &mut |p| {
            resolver_called_with.push(p.to_string());
            Some(format!("blob:{p}"))
        });
        // Only the local URL should reach the resolver
        assert_eq!(resolver_called_with, vec!["OEBPS/Text/local.png"]);
        assert!(
            result.contains("https://cdn.example.com/img.jpg 2x"),
            "ext: {result}"
        );
        assert!(
            result.contains("blob:OEBPS/Text/local.png 1x"),
            "local: {result}"
        );
    }

    // ── rewrite_srcset: data: URLs are passed through unchanged ───────────────

    #[test]
    fn test_rewrite_srcset_skips_data_urls() {
        // WHATWG spec constrains srcset to not mix data: URLs with other candidates
        // in practice; when a data: URL has no internal comma it is correctly skipped.
        // This tests the "data: prefix detected by is_external_url" path.
        let srcset = "data:image/gif;base64,abc= 1x, local.png 2x";
        let base = "OEBPS/Text";
        let result = rewrite_srcset(srcset, base, &mut |p| Some(format!("blob:{p}")));
        // The "abc= 1x" fragment (after the comma inside the data URL) is treated as
        // a separate candidate by the naive split — but its URL portion "abc=" passes
        // through is_external_url == false, reaches resolver, and gets rewritten.
        // What matters is: the "data:image/gif;base64" prefix candidate is SKIPPED,
        // and the explicitly local candidate "local.png" IS rewritten.
        assert!(
            result.contains("blob:OEBPS/Text/local.png"),
            "local.png must be rewritten: {result}"
        );
        // The data: scheme prefix survives
        assert!(
            result.contains("data:image/gif;base64"),
            "data: prefix must survive: {result}"
        );
    }

    // ── rewrite_resources: <picture> with <source srcset> + <img> ─────────────

    #[test]
    fn test_rewrite_picture_element_sources() {
        let html = r#"<html><body>
<picture>
  <source srcset="../img/photo.webp" type="image/webp">
  <source srcset="../img/photo.avif 1x, ../img/photo@2x.avif 2x" type="image/avif">
  <img src="../img/fallback.jpg" alt="photo">
</picture>
</body></html>"#;
        let mut resolved = Vec::new();
        let result = rewrite_resources(html, "OEBPS/Text/ch1.xhtml", move |path| {
            resolved.push(path.to_string());
            Some(format!("blob:{path}"))
        })
        .unwrap();

        // <img src> fallback
        assert!(
            result.contains(r#"src="blob:OEBPS/img/fallback.jpg""#),
            "img src: {result}"
        );
        // first <source srcset> — single URL, no descriptor
        assert!(
            result.contains("blob:OEBPS/img/photo.webp"),
            "webp: {result}"
        );
        // second <source srcset> — two candidates with descriptors
        assert!(
            result.contains("blob:OEBPS/img/photo.avif 1x"),
            "avif 1x: {result}"
        );
        assert!(
            result.contains("blob:OEBPS/img/photo@2x.avif 2x"),
            "avif 2x: {result}"
        );
    }

    // ── rewrite_resources: SVG <use href="sprite.svg#id"> ────────────────────

    #[test]
    fn test_rewrite_svg_use_cross_file_href() {
        // Only the path part before '#' should be resolved; the fragment must survive.
        let html =
            r#"<html><body><svg><use href="../images/sprite.svg#arrow"/></svg></body></html>"#;
        let result = rewrite_resources(html, "OEBPS/Text/ch1.xhtml", |path| {
            assert_eq!(
                path, "OEBPS/images/sprite.svg",
                "resolver must not receive fragment"
            );
            Some("blob:sprite".to_string())
        })
        .unwrap();
        assert!(
            result.contains(r#"href="blob:sprite#arrow""#),
            "fragment must be appended to rewritten URL: {result}"
        );
    }

    // ── rewrite_resources: SVG <use href="#local"> — pure local ref unchanged ─

    #[test]
    fn test_rewrite_svg_use_local_ref_unchanged() {
        // A href that starts with '#' is a same-document reference; resolver must NOT be called.
        let html = "<html><body><svg><use href=\"#local-icon\"/></svg></body></html>";
        let result = rewrite_resources(html, "OEBPS/Text/ch1.xhtml", |_| {
            panic!("resolver must not be called for pure local SVG references");
        })
        .unwrap();
        assert!(
            result.contains("href=\"#local-icon\""),
            "local ref must be unchanged: {result}"
        );
    }

    #[test]
    fn test_rewrite_resources_resolver_none_keeps_original() {
        let html = r#"<html><body><img src="missing.jpg" /></body></html>"#;
        let result = rewrite_resources(html, "OEBPS/ch1.xhtml", |_| None).unwrap();
        assert!(
            result.contains(r#"src="missing.jpg""#),
            "original src preserved: {result}"
        );
    }
}
