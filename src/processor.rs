//! HTML content processor using `lol_html`.

use crate::error::EpubError;
use lol_html::{element, text, HtmlRewriter, Settings};
use std::cell::RefCell;
use std::rc::Rc;

/// Extracts plain text from an HTML byte slice.
pub fn extract_text(html: &[u8]) -> Result<String, EpubError> {
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

    rewriter
        .write(html)
        .map_err(|e| EpubError::InvalidFormat(format!("HTML extraction error: {}", e)))?;
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
    let mapper_rc = Rc::new(RefCell::new(link_mapper));

    {
        let mapper_img = Rc::clone(&mapper_rc);
        let mapper_a = Rc::clone(&mapper_rc);
        let mapper_link = Rc::clone(&mapper_rc);

        let mut rewriter = HtmlRewriter::new(
            Settings {
                element_content_handlers: vec![
                    element!("img[src]", move |el| {
                        if let Some(src) = el.get_attribute("src") {
                            if let Some(new_src) = (mapper_img.borrow_mut())("img", &src) {
                                el.set_attribute("src", &new_src).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                            }
                        }
                        Ok(())
                    }),
                    element!("a[href]", move |el| {
                        if let Some(href) = el.get_attribute("href") {
                            if let Some(new_href) = (mapper_a.borrow_mut())("a", &href) {
                                el.set_attribute("href", &new_href).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                            }
                        }
                        Ok(())
                    }),
                    element!("link[href]", move |el| {
                        if let Some(href) = el.get_attribute("href") {
                            if let Some(new_href) = (mapper_link.borrow_mut())("link", &href) {
                                el.set_attribute("href", &new_href).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                            }
                        }
                        Ok(())
                    }),
                ],
                ..Settings::default()
            },
            |c: &[u8]| output.extend_from_slice(c),
        );

        rewriter
            .write(html)
            .map_err(|e| EpubError::InvalidFormat(format!("HTML rewrite error: {}", e)))?;
        rewriter
            .end()
            .map_err(|e| EpubError::InvalidFormat(format!("HTML rewrite error: {}", e)))?;
    }

    Ok(output)
}
