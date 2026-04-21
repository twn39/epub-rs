//! HTML content processor using `lol_html`.

use crate::error::EpubError;
use crate::model::ContentElement;
use crate::model::Position;
use kuchikiki::NodeRef;
use kuchikiki::traits::*;
use lol_html::{HtmlRewriter, Settings, element, text};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use std::sync::atomic::{AtomicBool, Ordering};

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

/// Normalizes a relative URL path against a base directory within the EPUB archive.
/// Automatically handles URL-decoding (e.g. `%20` -> ` `) and stripping of URL query strings or hashes.
/// Example: `OEBPS/Text` + `../Images/cover%20image.jpg?v=1` -> `OEBPS/Images/cover image.jpg`
pub fn normalize_path(base_dir: &str, rel_path: &str) -> String {
    // 1. Strip query string or hash suffix from the URL
    let mut path_only = rel_path;
    if let Some(idx) = path_only.find('?') {
        path_only = &path_only[..idx];
    }
    if let Some(idx) = path_only.find('#') {
        path_only = &path_only[..idx];
    }

    // 2. URL decode
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
            // RootDir or CurDir ('.') are mostly ignored or handled inherently
            _ => {}
        }
    }

    // Convert back to string, replacing Windows backslashes if they somehow appear (EPUB uses forward slashes)
    path.to_string_lossy().replace('\\', "/")
}

/// Helper to check if a URL is external (http/https/data/mailto etc.)
fn is_external_url(url: &str) -> bool {
    url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("data:")
        || url.starts_with("mailto:")
        || url.starts_with("ftp:")
        || url.starts_with("//") // Protocol-relative URLs
}

/// Rewrite resources (images, css, links) in an HTML document using a provided resolver callback.
/// The resolver receives the absolute path within the EPUB (e.g. `OEBPS/Images/pic.jpg`)
/// and should return `Some(new_url)` if it wants to rewrite it, or `None` to leave it unchanged.
pub fn rewrite_resources<F>(
    html: &str,
    base_file_path: &str,
    resolver: F,
) -> Result<String, EpubError>
where
    F: FnMut(&str) -> Option<String> + 'static,
{
    let mut output = Vec::new();
    let resolver_arc = Arc::new(Mutex::new(resolver));

    let mut rewriter = HtmlRewriter::new(
        Settings {
            element_content_handlers: vec![
                // 1. Rewrite media sources
                element!("img, video, audio, source, track", {
                    let resolver = Arc::clone(&resolver_arc);
                    move |el| {
                        if let Some(src) = el.get_attribute("src")
                            && !is_external_url(&src)
                            && !src.starts_with('#')
                        {
                            let abs_path = normalize_path(base_file_path, &src);
                            if let Some(new_url) = (resolver.lock().unwrap())(&abs_path) {
                                el.set_attribute("src", &new_url).unwrap();
                            }
                        }
                        if let Some(poster) = el.get_attribute("poster")
                            && !is_external_url(&poster)
                            && !poster.starts_with('#')
                        {
                            let abs_path = normalize_path(base_file_path, &poster);
                            if let Some(new_url) = (resolver.lock().unwrap())(&abs_path) {
                                el.set_attribute("poster", &new_url).unwrap();
                            }
                        }
                        Ok(())
                    }
                }),
                // 2. Rewrite object data
                element!("object", {
                    let resolver = Arc::clone(&resolver_arc);
                    move |el| {
                        if let Some(data) = el.get_attribute("data")
                            && !is_external_url(&data)
                            && !data.starts_with('#')
                        {
                            let abs_path = normalize_path(base_file_path, &data);
                            if let Some(new_url) = (resolver.lock().unwrap())(&abs_path) {
                                el.set_attribute("data", &new_url).unwrap();
                            }
                        }
                        Ok(())
                    }
                }),
                // 3. Rewrite SVG images
                element!("image", {
                    let resolver = Arc::clone(&resolver_arc);
                    move |el| {
                        for attr in &["href", "xlink:href"] {
                            if let Some(href) = el.get_attribute(attr)
                                && !is_external_url(&href)
                                && !href.starts_with('#')
                            {
                                let abs_path = normalize_path(base_file_path, &href);
                                if let Some(new_url) = (resolver.lock().unwrap())(&abs_path) {
                                    el.set_attribute(attr, &new_url).unwrap();
                                }
                            }
                        }
                        Ok(())
                    }
                }),
                // 4. Rewrite stylesheets and hyperlinks
                element!("link, a, area", {
                    let resolver = Arc::clone(&resolver_arc);
                    move |el| {
                        if let Some(href) = el.get_attribute("href") {
                            // Skip external links and pure local anchors (e.g. href="#chapter1")
                            if !is_external_url(&href) && !href.starts_with('#') {
                                // Extract path and anchor (e.g. `ch2.xhtml#section1` -> `ch2.xhtml`, `#section1`)
                                let (path_part, anchor_part) = match href.find('#') {
                                    Some(idx) => (&href[..idx], &href[idx..]),
                                    None => (href.as_str(), ""),
                                };

                                // We only resolve the file path part
                                if !path_part.is_empty() {
                                    let abs_path = normalize_path(base_file_path, path_part);
                                    if let Some(mut new_url) = (resolver.lock().unwrap())(&abs_path)
                                    {
                                        // Append the anchor back if it existed
                                        new_url.push_str(anchor_part);
                                        el.set_attribute("href", &new_url).unwrap();
                                    }
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

/// Extracts plain text from an HTML stream. Memory efficient.
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
        |_: &[u8]| {}, // We discard the output HTML since we only want text
    );

    let mut buffer = [0; 8192]; // 8KB buffer
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

/// Rewrites HTML links (e.g., `<img src="...">`, `<a href="...">`) using a provided mapping function.
/// This is useful when serving EPUB content on the web to rewrite local ZIP paths to CDN or server URLs.
pub fn rewrite_links<F>(html: &[u8], link_mapper: F) -> Result<Vec<u8>, EpubError>
where
    F: FnMut(&str, &str) -> Option<String> + 'static,
{
    let mut output = Vec::new();
    rewrite_links_stream(html, &mut output, link_mapper)?;
    Ok(output)
}

/// Rewrites HTML links from a stream to a writer. Memory efficient.
pub fn rewrite_links_stream<R: Read, W: Write, F>(
    mut reader: R,
    mut writer: W,
    link_mapper: F,
) -> Result<(), EpubError>
where
    F: FnMut(&str, &str) -> Option<String> + 'static,
{
    let mapper_arc = Arc::new(Mutex::new(link_mapper));

    // To handle io::Error during write inside the closure
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

    let final_error = write_error.lock().unwrap().take();
    if let Some(err) = final_error {
        return Err(EpubError::Io(err));
    }

    Ok(())
}

/// Injects custom HTML elements (such as `<style>` or `<script>`) into the `<head>` of the document.
/// Very useful for dynamically applying themes (e.g. Dark Mode) or custom typography in web readers.
pub fn inject_head_content<R: Read, W: Write>(
    mut reader: R,
    mut writer: W,
    content_to_inject: &str,
) -> Result<(), EpubError> {
    let write_error = Arc::new(Mutex::new(None));
    let error_clone = Arc::clone(&write_error);

    // We clone the string so it can be moved into the closure
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

    let final_error = write_error.lock().unwrap().take();
    if let Some(err) = final_error {
        return Err(EpubError::Io(err));
    }

    Ok(())
}

/// Injects Canonical Fragment Identifier (CFI) paths as `data-cfi` attributes into all HTML elements.
///
/// This uses `kuchikiki` to build a DOM tree, calculate the strict CFI path for every node, and serialize it.
/// It accepts a `base_cfi` (e.g., `/6/4[chap01ref]!`) to prepend to the local path.
pub fn inject_cfi_dom(html: &str, base_cfi: &str) -> Result<String, EpubError> {
    let document = kuchikiki::parse_html().one(html);

    // We start from the html element (or body if preferred).
    // In EPUB CFI, the <html> element is usually `/2` under the document root.
    if let Ok(html_node) = document.select_first("html") {
        traverse_and_inject(html_node.as_node(), base_cfi, "");
    }

    let mut out = Vec::new();
    document
        .serialize(&mut out)
        .map_err(|e| EpubError::InvalidFormat(format!("DOM serialization failed: {}", e)))?;
    Ok(String::from_utf8_lossy(&out).to_string())
}

fn traverse_and_inject(node: &NodeRef, base_cfi: &str, current_path: &str) {
    let mut child_index = 0; // 0 is before the first element

    for child in node.children() {
        if child.as_element().is_some() {
            child_index += 2; // Elements are 2, 4, 6...

            let id_assertion = child
                .as_element()
                .and_then(|el| el.attributes.borrow().get("id").map(|s| s.to_string()));

            let assertion_str = match id_assertion {
                Some(id) => format!("[{}]", id),
                None => String::new(),
            };

            let child_path = format!("{}/{}{}", current_path, child_index, assertion_str);

            // Inject attribute
            if let Some(el) = child.as_element() {
                let full_cfi = format!("epubcfi({}{})", base_cfi, child_path);
                el.attributes.borrow_mut().insert("data-cfi", full_cfi);
            }

            // Recurse into children
            traverse_and_inject(&child, base_cfi, &child_path);
        }
    }
}

/// A search result mapped to its exact Canonical Fragment Identifier (CFI) range.
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
pub struct SearchResult {
    /// The excerpt of text where the match was found.
    pub excerpt: String,
    /// The exact CFI range string (e.g. `epubcfi(/6/4!/4/2,/1:5,/1:10)`) pointing to the match.
    pub cfi: String,
}

/// Searches an HTML string for a regular expression pattern and returns a list of results
/// mapped to their exact CFI ranges.
///
/// This is a killer feature for building web readers: the backend searches the text
/// and returns the CFIs. The frontend can just use these CFIs to highlight the results
/// without needing to parse or search the DOM itself.
///
/// * `html` - The raw HTML content of the chapter.
/// * `base_cfi` - The OPF context path (e.g. `/6/4!`).
/// * `pattern` - A compiled regular expression to search for.
pub fn search_chapter(
    html: &str,
    base_cfi: &str,
    pattern: &regex::Regex,
) -> Result<Vec<SearchResult>, EpubError> {
    let document = kuchikiki::parse_html().one(html);

    let mut results = Vec::new();

    // We start from the html element
    if let Ok(html_node) = document.select_first("html") {
        search_node(html_node.as_node(), base_cfi, "", pattern, &mut results);
    }

    Ok(results)
}

fn search_node(
    node: &NodeRef,
    base_cfi: &str,
    current_path: &str,
    pattern: &regex::Regex,
    results: &mut Vec<SearchResult>,
) {
    let mut child_index = 0; // Starts at 0 (before any element)

    for child in node.children() {
        if child.as_element().is_some() {
            child_index += 2; // Next element index

            let id_assertion = child
                .as_element()
                .and_then(|el| el.attributes.borrow().get("id").map(|s| s.to_string()));

            let assertion_str = match id_assertion {
                Some(id) => format!("[{}]", id),
                None => String::new(),
            };

            let child_path = format!("{}/{}{}", current_path, child_index, assertion_str);

            // Recurse into children
            search_node(&child, base_cfi, &child_path, pattern, results);
        } else if let Some(text_node) = child.as_text() {
            let text = text_node.borrow();

            // Text nodes are odd numbers in CFI: 1, 3, 5...
            let cfi_text_idx = child_index + 1;

            for mat in pattern.find_iter(&text) {
                let start = mat.start();
                let end = mat.end();

                // Build the range CFI
                // e.g. epubcfi(/6/4!/4/2,/1:start,/1:end)
                let range_cfi = format!(
                    "epubcfi({}{},/{}:{},/{}:{})",
                    base_cfi, current_path, cfi_text_idx, start, cfi_text_idx, end
                );

                // Extract a small context excerpt (~20 chars) around the match.
                // The regex returns byte offsets; we must snap to char boundaries to
                // avoid panicking on multi-byte characters (e.g., CJK text).
                let context_start = {
                    let mut idx = start.saturating_sub(20);
                    while idx > 0 && !text.is_char_boundary(idx) {
                        idx -= 1;
                    }
                    idx
                };
                let context_end = {
                    let mut idx = (end + 20).min(text.len());
                    while idx < text.len() && !text.is_char_boundary(idx) {
                        idx += 1;
                    }
                    idx
                };
                let excerpt = text[context_start..context_end].to_string();

                results.push(SearchResult {
                    excerpt,
                    cfi: range_cfi,
                });
            }
        }
    }
}

pub struct PositionContext<'a> {
    pub base_cfi: &'a str,
    pub chars_per_position: usize,
    pub spine_index: usize,
    pub href: &'a str,
}

pub fn extract_positions(
    html: &str,
    ctx: &PositionContext,
    char_counter: &mut usize,
    positions: &mut Vec<Position>,
    global_pos: &mut usize,
) {
    let document = kuchikiki::parse_html().one(html);
    if let Ok(html_node) = document.select_first("html") {
        traverse_for_positions(
            html_node.as_node(),
            ctx,
            "",
            char_counter,
            positions,
            global_pos,
        );
    }
}

fn traverse_for_positions(
    node: &NodeRef,
    ctx: &PositionContext,
    current_path: &str,
    char_counter: &mut usize,
    positions: &mut Vec<Position>,
    global_pos: &mut usize,
) {
    let mut child_index = 0;

    for child in node.children() {
        if child.as_element().is_some() {
            child_index += 2;
            let id_assertion = child
                .as_element()
                .and_then(|el| el.attributes.borrow().get("id").map(|s| s.to_string()));
            let assertion_str = match id_assertion {
                Some(id) => format!("[{}]", id),
                None => String::new(),
            };
            let child_path = format!("{}/{}{}", current_path, child_index, assertion_str);

            traverse_for_positions(
                &child,
                ctx,
                &child_path,
                char_counter,
                positions,
                global_pos,
            );
        } else if let Some(text_node) = child.as_text() {
            let text = text_node.borrow();
            let text_len = text.chars().count();
            let cfi_text_idx = child_index + 1;

            let mut offset = 0;
            while *char_counter + (text_len - offset) >= ctx.chars_per_position {
                let chars_needed = ctx.chars_per_position - *char_counter;
                offset += chars_needed;

                *global_pos += 1;
                let mut stripped_base = ctx.base_cfi.to_string();
                if stripped_base.ends_with('!') {
                    stripped_base.pop();
                }

                // Add the specific CFI range
                let cfi = format!(
                    "epubcfi({}!{}/{}:{})",
                    stripped_base, current_path, cfi_text_idx, offset
                );

                positions.push(Position {
                    spine_index: ctx.spine_index,
                    href: ctx.href.to_string(),
                    cfi,
                    global_position: *global_pos,
                    chapter_progression: 0.0,
                    total_progression: 0.0,
                });

                *char_counter = 0;
            }
            *char_counter += text_len - offset;
        }
    }
}

/// Extracts a structured, semantic list of content elements (headings, paragraphs, blockquotes)
/// from the HTML chapter. Excellent for feeding a Text-To-Speech (TTS) engine, as it preserves
/// paragraph boundaries, language, and exact CFI highlighting coordinates.
pub fn extract_semantic_content(html: &str, base_cfi: &str) -> Vec<ContentElement> {
    let document = kuchikiki::parse_html().one(html);
    let mut elements = Vec::new();

    // Find language from html tag or body if defined
    let doc_lang = document
        .select_first("html")
        .ok()
        .and_then(|n| {
            n.as_node()
                .as_element()
                .unwrap()
                .attributes
                .borrow()
                .get("lang")
                .map(|s| s.to_string())
        })
        .or_else(|| {
            document.select_first("html").ok().and_then(|n| {
                n.as_node()
                    .as_element()
                    .unwrap()
                    .attributes
                    .borrow()
                    .get("xml:lang")
                    .map(|s| s.to_string())
            })
        });

    let html_node_path = ""; // HTML itself is usually implicit in the `!` boundary

    if let Ok(html_node) = document.select_first("html") {
        // Look for the body tag inside HTML
        let mut child_index = 0;
        let mut body_path = format!("{}/4", html_node_path); // Default is /4
        let mut body_node = None;

        for child in html_node.as_node().children() {
            if let Some(el) = child.as_element() {
                child_index += 2;
                if el.name.local.to_string() == "body" {
                    body_node = Some(child.clone());
                    body_path = format!("{}/{}", html_node_path, child_index);
                    break;
                }
            }
        }

        if let Some(body) = body_node {
            let mut stripped_base = base_cfi.to_string();
            if stripped_base.ends_with('!') {
                stripped_base.pop();
            }

            traverse_semantic_nodes(&body, &stripped_base, &body_path, &doc_lang, &mut elements);
        }
    }

    elements
}

fn traverse_semantic_nodes(
    node: &NodeRef,
    base_cfi: &str,
    current_path: &str,
    inherited_lang: &Option<String>,
    elements: &mut Vec<ContentElement>,
) {
    let mut child_index = 0;

    for child in node.children() {
        if let Some(el) = child.as_element() {
            child_index += 2;
            let tag_name = el.name.local.to_string();

            let id_assertion = el.attributes.borrow().get("id").map(|s| s.to_string());
            let assertion_str = match id_assertion {
                Some(id) => format!("[{}]", id),
                None => String::new(),
            };

            let child_path = format!("{}/{}{}", current_path, child_index, assertion_str);

            let mut current_lang = inherited_lang.clone();
            if let Some(lang) = el.attributes.borrow().get("lang") {
                current_lang = Some(lang.to_string());
            } else if let Some(lang) = el.attributes.borrow().get("xml:lang") {
                current_lang = Some(lang.to_string());
            }

            // Is this a block-level semantic container?
            let is_block = matches!(
                tag_name.as_str(),
                "p" | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "blockquote"
                    | "li"
                    | "dt"
                    | "dd"
                    | "figcaption"
            );

            if is_block {
                let text_content = child.text_contents().trim().to_string();
                if !text_content.is_empty() {
                    let cfi_range = format!("epubcfi({}!{})", base_cfi, child_path);

                    elements.push(ContentElement {
                        text: text_content,
                        cfi_range,
                        tag_name: tag_name.clone(),
                        language: current_lang.clone(),
                    });
                }
            } else {
                // Not a block (e.g. <div>, <section>, <span>), traverse deeper
                traverse_semantic_nodes(&child, base_cfi, &child_path, &current_lang, elements);
            }
        }
    }
}
