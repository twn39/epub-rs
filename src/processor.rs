//! HTML content processor using `lol_html`.

use crate::error::EpubError;
use lol_html::{element, text, HtmlRewriter, Settings};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::rc::Rc;
use crate::model::Position;
use kuchikiki::traits::*;
use kuchikiki::NodeRef;

/// Extracts plain text from an HTML byte slice.
pub fn extract_text(html: &[u8]) -> Result<String, EpubError> {
    extract_text_stream(html)
}

/// Extracts plain text from an HTML stream. Memory efficient.
pub fn extract_text_stream<R: Read>(mut reader: R) -> Result<String, EpubError> {
    let extracted_text = Rc::new(RefCell::new(String::new()));
    let text_clone = Rc::clone(&extracted_text);

    let mut rewriter = HtmlRewriter::new(
        Settings {
            element_content_handlers: vec![
                element!("script, style", |_el| {
                    Ok(())
                }),
                text!("body", |t| {
                    text_clone.borrow_mut().push_str(t.as_str());
                    Ok(())
                }),
            ],
            ..Settings::default()
        },
        |_: &[u8]| {} // We discard the output HTML since we only want text
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

    let result = extracted_text.borrow().clone();
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
    let mapper_rc = Rc::new(RefCell::new(link_mapper));

    // To handle io::Error during write inside the closure
    let write_error = Rc::new(RefCell::new(None));
    let error_clone = Rc::clone(&write_error);

    {
        let mapper_img = Rc::clone(&mapper_rc);
        let mapper_a = Rc::clone(&mapper_rc);
        let mapper_link = Rc::clone(&mapper_rc);

        let mut rewriter = HtmlRewriter::new(
            Settings {
                element_content_handlers: vec![
                    element!("img[src]", move |el| {
                        if let Some(src) = el.get_attribute("src")
                            && let Some(new_src) = (mapper_img.borrow_mut())("img", &src) {
                                el.set_attribute("src", &new_src).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                            }
                        Ok(())
                    }),
                    element!("a[href]", move |el| {
                        if let Some(href) = el.get_attribute("href")
                            && let Some(new_href) = (mapper_a.borrow_mut())("a", &href) {
                                el.set_attribute("href", &new_href).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                            }
                        Ok(())
                    }),
                    element!("link[href]", move |el| {
                        if let Some(href) = el.get_attribute("href")
                            && let Some(new_href) = (mapper_link.borrow_mut())("link", &href) {
                                el.set_attribute("href", &new_href).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                            }
                        Ok(())
                    }),
                ],
                ..Settings::default()
            },
            |c: &[u8]| {
                if let Err(e) = writer.write_all(c) {
                    *error_clone.borrow_mut() = Some(e);
                }
            },
        );

        let mut buffer = [0; 8192];
        loop {
            if write_error.borrow().is_some() {
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

        if write_error.borrow().is_none() {
            rewriter
                .end()
                .map_err(|e| EpubError::InvalidFormat(format!("HTML rewrite error: {}", e)))?;
        }
    }

    let final_error = write_error.borrow_mut().take();
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
    let write_error = Rc::new(RefCell::new(None));
    let error_clone = Rc::clone(&write_error);
    
    // We clone the string so it can be moved into the closure
    let content = content_to_inject.to_string();

    {
        let mut rewriter = HtmlRewriter::new(
            Settings {
                element_content_handlers: vec![
                    element!("head", move |el| {
                        el.append(&content, lol_html::html_content::ContentType::Html);
                        Ok(())
                    }),
                ],
                ..Settings::default()
            },
            |c: &[u8]| {
                if let Err(e) = writer.write_all(c) {
                    *error_clone.borrow_mut() = Some(e);
                }
            },
        );

        let mut buffer = [0; 8192];
        loop {
            if write_error.borrow().is_some() {
                break;
            }
            let bytes_read = reader.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            rewriter
                .write(&buffer[..bytes_read])
                .map_err(|e| EpubError::InvalidFormat(format!("HTML theme injection error: {}", e)))?;
        }

        if write_error.borrow().is_none() {
            rewriter
                .end()
                .map_err(|e| EpubError::InvalidFormat(format!("HTML theme injection error: {}", e)))?;
        }
    }

    let final_error = write_error.borrow_mut().take();
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
    document.serialize(&mut out).map_err(|e| EpubError::InvalidFormat(format!("DOM serialization failed: {}", e)))?;
    Ok(String::from_utf8_lossy(&out).to_string())
}

fn traverse_and_inject(node: &NodeRef, base_cfi: &str, current_path: &str) {
    let mut child_index = 0; // 0 is before the first element
    
    for child in node.children() {
        if child.as_element().is_some() {
            child_index += 2; // Elements are 2, 4, 6...
            
            let id_assertion = child.as_element()
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
#[derive(Debug, Clone, PartialEq)]
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
pub fn search_chapter(html: &str, base_cfi: &str, pattern: &regex::Regex) -> Result<Vec<SearchResult>, EpubError> {
    let document = kuchikiki::parse_html().one(html);
    
    let mut results = Vec::new();
    
    // We start from the html element
    if let Ok(html_node) = document.select_first("html") {
        search_node(html_node.as_node(), base_cfi, "", pattern, &mut results);
    }
    
    Ok(results)
}

fn search_node(node: &NodeRef, base_cfi: &str, current_path: &str, pattern: &regex::Regex, results: &mut Vec<SearchResult>) {
    let mut child_index = 0; // Starts at 0 (before any element)
    
    for child in node.children() {
        if child.as_element().is_some() {
            child_index += 2; // Next element index
            
            let id_assertion = child.as_element()
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
                    base_cfi, current_path,
                    cfi_text_idx, start,
                    cfi_text_idx, end
                );
                
                // Extract a small context excerpt (up to 20 chars around the match)
                let context_start = start.saturating_sub(20);
                let context_end = std::cmp::min(text.len(), end + 20);
                let excerpt = text[context_start..context_end].to_string();
                
                results.push(SearchResult {
                    excerpt,
                    cfi: range_cfi,
                });
            }
        }
    }
}

pub fn extract_positions(
    html: &str,
    base_cfi: &str,
    chars_per_position: usize,
    char_counter: &mut usize,
    positions: &mut Vec<Position>,
    global_pos: &mut usize,
    spine_index: usize,
    href: &str,
) {
    let document = kuchikiki::parse_html().one(html);
    if let Ok(html_node) = document.select_first("html") {
        traverse_for_positions(html_node.as_node(), base_cfi, "", chars_per_position, char_counter, positions, global_pos, spine_index, href);
    }
}

fn traverse_for_positions(
    node: &NodeRef,
    base_cfi: &str,
    current_path: &str,
    chars_per_position: usize,
    char_counter: &mut usize,
    positions: &mut Vec<Position>,
    global_pos: &mut usize,
    spine_index: usize,
    href: &str,
) {
    let mut child_index = 0;
    
    for child in node.children() {
        if child.as_element().is_some() {
            child_index += 2;
            let id_assertion = child.as_element()
                .and_then(|el| el.attributes.borrow().get("id").map(|s| s.to_string()));
            let assertion_str = match id_assertion {
                Some(id) => format!("[{}]", id),
                None => String::new(),
            };
            let child_path = format!("{}/{}{}", current_path, child_index, assertion_str);
            
            traverse_for_positions(&child, base_cfi, &child_path, chars_per_position, char_counter, positions, global_pos, spine_index, href);
        } else if let Some(text_node) = child.as_text() {
            let text = text_node.borrow();
            let text_len = text.chars().count();
            let cfi_text_idx = child_index + 1;
            
            let mut offset = 0;
            while *char_counter + (text_len - offset) >= chars_per_position {
                let chars_needed = chars_per_position - *char_counter;
                offset += chars_needed;
                
                *global_pos += 1;
                let mut stripped_base = base_cfi.to_string();
                if stripped_base.ends_with('!') {
                    stripped_base.pop();
                }
                
                // Add the specific CFI range
                let cfi = format!("epubcfi({}!{}/{}:{})", stripped_base, current_path, cfi_text_idx, offset);
                
                positions.push(Position {
                    spine_index,
                    href: href.to_string(),
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
