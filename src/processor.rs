//! HTML content processor using `lol_html`.

use crate::error::EpubError;
use lol_html::{element, text, HtmlRewriter, Settings};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::rc::Rc;
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
    let mut element_index = 0; // Only elements are even numbers starting from 2
    
    for child in node.children() {
        if child.as_element().is_some() {
            element_index += 2; // Elements are 2, 4, 6...
            
            let id_assertion = child.as_element()
                .and_then(|el| el.attributes.borrow().get("id").map(|s| s.to_string()));
            
            let assertion_str = match id_assertion {
                Some(id) => format!("[{}]", id),
                None => String::new(),
            };
            
            let child_path = format!("{}/{}{}", current_path, element_index, assertion_str);
            
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
