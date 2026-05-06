//! lol_html-based HTML transformation utilities.
//!
//! All operations are streaming SAX-style (no full DOM allocation).

use crate::error::EpubError;
use lol_html::{HtmlRewriter, Settings, element, text};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
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

fn is_external_url(url: &str) -> bool {
    url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("data:")
        || url.starts_with("mailto:")
        || url.starts_with("ftp:")
        || url.starts_with("//")
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Normalizes a relative URL path against a base directory within the EPUB archive.
/// Automatically handles URL-decoding (e.g. `%20` -> ` `) and stripping of URL query strings or hashes.
/// The `base_dir` must be the **directory** containing the referencing file, NOT the file path itself.
pub fn normalize_path(base_dir: &str, rel_path: &str) -> String {
    let mut path_only = rel_path;
    if let Some(idx) = path_only.find('?') {
        path_only = &path_only[..idx];
    }
    if let Some(idx) = path_only.find('#') {
        path_only = &path_only[..idx];
    }

    let decoded = percent_encoding::percent_decode_str(path_only).decode_utf8_lossy();

    let mut path = PathBuf::from(base_dir);
    for component in Path::new(decoded.as_ref()).components() {
        match component {
            Component::ParentDir => {
                path.pop();
            }
            Component::Normal(c) => {
                path.push(c);
            }
            _ => {}
        }
    }

    path.to_string_lossy().replace('\\', "/")
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
    let resolver_arc = Arc::new(Mutex::new(resolver));

    let mut rewriter = HtmlRewriter::new(
        Settings {
            element_content_handlers: vec![
                element!("img, video, audio, source, track", {
                    let resolver = Arc::clone(&resolver_arc);
                    let base_dir = base_dir.clone();
                    move |el| {
                        if let Some(src) = el.get_attribute("src")
                            && !is_external_url(&src)
                            && !src.starts_with('#')
                        {
                            let abs_path = normalize_path(&base_dir, &src);
                            if let Some(new_url) = (resolver.lock().unwrap())(&abs_path) {
                                el.set_attribute("src", &new_url).unwrap();
                            }
                        }
                        if let Some(poster) = el.get_attribute("poster")
                            && !is_external_url(&poster)
                            && !poster.starts_with('#')
                        {
                            let abs_path = normalize_path(&base_dir, &poster);
                            if let Some(new_url) = (resolver.lock().unwrap())(&abs_path) {
                                el.set_attribute("poster", &new_url).unwrap();
                            }
                        }
                        Ok(())
                    }
                }),
                element!("object", {
                    let resolver = Arc::clone(&resolver_arc);
                    let base_dir = base_dir.clone();
                    move |el| {
                        if let Some(data) = el.get_attribute("data")
                            && !is_external_url(&data)
                            && !data.starts_with('#')
                        {
                            let abs_path = normalize_path(&base_dir, &data);
                            if let Some(new_url) = (resolver.lock().unwrap())(&abs_path) {
                                el.set_attribute("data", &new_url).unwrap();
                            }
                        }
                        Ok(())
                    }
                }),
                element!("image", {
                    let resolver = Arc::clone(&resolver_arc);
                    let base_dir = base_dir.clone();
                    move |el| {
                        for attr in &["href", "xlink:href"] {
                            if let Some(href) = el.get_attribute(attr)
                                && !is_external_url(&href)
                                && !href.starts_with('#')
                            {
                                let abs_path = normalize_path(&base_dir, &href);
                                if let Some(new_url) = (resolver.lock().unwrap())(&abs_path) {
                                    el.set_attribute(attr, &new_url).unwrap();
                                }
                            }
                        }
                        Ok(())
                    }
                }),
                element!("link, a, area", {
                    let resolver = Arc::clone(&resolver_arc);
                    let base_dir = base_dir.clone();
                    move |el| {
                        if let Some(href) = el.get_attribute("href")
                            && !is_external_url(&href)
                            && !href.starts_with('#')
                        {
                            let (path_part, anchor_part) = match href.find('#') {
                                Some(idx) => (&href[..idx], &href[idx..]),
                                None => (href.as_str(), ""),
                            };
                            if !path_part.is_empty() {
                                let abs_path = normalize_path(&base_dir, path_part);
                                if let Some(mut new_url) = (resolver.lock().unwrap())(&abs_path) {
                                    new_url.push_str(anchor_part);
                                    el.set_attribute("href", &new_url).unwrap();
                                }
                            }
                        }
                        Ok(())
                    }
                }),
            ],
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
    String::from_utf8(output).map_err(|e| EpubError::HtmlParse(format!("Invalid UTF-8: {}", e)))
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
    fn test_normalize_path() {
        assert_eq!(
            normalize_path("OEBPS/Text", "../Images/cover.jpg"),
            "OEBPS/Images/cover.jpg"
        );
        assert_eq!(
            normalize_path("OEBPS/Text", "chapter2.xhtml"),
            "OEBPS/Text/chapter2.xhtml"
        );
        assert_eq!(normalize_path("", "images/cover.jpg"), "images/cover.jpg");
        assert_eq!(
            normalize_path("OEBPS/Text", "../Images/my%20cover.jpg"),
            "OEBPS/Images/my cover.jpg"
        );
        assert_eq!(
            normalize_path("OEBPS", "css/style.css?v=2.0#section"),
            "OEBPS/css/style.css"
        );
    }

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
}
