//! CFI-aware DOM processing: attribute injection and full-text search.
//!
//! Uses `kuchikiki` to build a full DOM tree and compute CFI paths,
//! enabling both `data-cfi` annotation and regex-based search with CFI ranges.

use crate::error::EpubError;
use kuchikiki::NodeRef;
use kuchikiki::traits::*;

// ── Public types ──────────────────────────────────────────────────────────────

/// A search result mapped to its exact Canonical Fragment Identifier (CFI) range.
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
pub struct SearchResult {
    /// The excerpt of text where the match was found.
    pub excerpt: String,
    /// The exact CFI range string (e.g. `epubcfi(/6/4!/4/2,/1:5,/1:10)`).
    pub cfi: String,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Injects `data-cfi` attributes into every element of an HTML document.
///
/// Accepts a `base_cfi` (e.g. `/6/4[chap01ref]!`) to prepend to local paths.
pub fn inject_cfi_dom(html: &str, base_cfi: &str) -> Result<String, EpubError> {
    let document = kuchikiki::parse_html().one(html);
    if let Ok(html_node) = document.select_first("html") {
        traverse_and_inject(html_node.as_node(), base_cfi, "");
    }
    let mut out = Vec::new();
    document
        .serialize(&mut out)
        .map_err(|e| EpubError::InvalidFormat(format!("DOM serialization failed: {}", e)))?;
    Ok(String::from_utf8_lossy(&out).to_string())
}

/// Searches an HTML string for a regex pattern, returning CFI range results.
pub fn search_chapter(
    html: &str,
    base_cfi: &str,
    pattern: &regex::Regex,
) -> Result<Vec<SearchResult>, EpubError> {
    let document = kuchikiki::parse_html().one(html);
    let mut results = Vec::new();
    if let Ok(html_node) = document.select_first("html") {
        search_node(html_node.as_node(), base_cfi, "", pattern, &mut results);
    }
    Ok(results)
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn traverse_and_inject(node: &NodeRef, base_cfi: &str, current_path: &str) {
    let mut child_index = 0;
    for child in node.children() {
        if child.as_element().is_some() {
            child_index += 2;
            let child_path = super::cfi_child_path(
                current_path, child_index, &super::cfi_assertion(&child),
            );
            if let Some(el) = child.as_element() {
                let full_cfi = format!("epubcfi({}{})", base_cfi, child_path);
                el.attributes.borrow_mut().insert("data-cfi", full_cfi);
            }
            traverse_and_inject(&child, base_cfi, &child_path);
        }
    }
}

fn search_node(
    node: &NodeRef,
    base_cfi: &str,
    current_path: &str,
    pattern: &regex::Regex,
    results: &mut Vec<SearchResult>,
) {
    let mut child_index = 0;
    for child in node.children() {
        if child.as_element().is_some() {
            child_index += 2;
            let child_path = super::cfi_child_path(
                current_path, child_index, &super::cfi_assertion(&child),
            );
            search_node(&child, base_cfi, &child_path, pattern, results);
        } else if let Some(text_node) = child.as_text() {
            let text = text_node.borrow();
            let cfi_text_idx = child_index + 1;

            for mat in pattern.find_iter(&text) {
                let start = mat.start();
                let end   = mat.end();

                let range_cfi = format!(
                    "epubcfi({}{},/{}:{},/{}:{})",
                    base_cfi, current_path, cfi_text_idx, start, cfi_text_idx, end,
                );

                let context_start = {
                    let mut idx = start.saturating_sub(20);
                    while idx > 0 && !text.is_char_boundary(idx) { idx -= 1; }
                    idx
                };
                let context_end = {
                    let mut idx = (end + 20).min(text.len());
                    while idx < text.len() && !text.is_char_boundary(idx) { idx += 1; }
                    idx
                };

                results.push(SearchResult {
                    excerpt: text[context_start..context_end].to_string(),
                    cfi: range_cfi,
                });
            }
        }
    }
}
