//! Semantic content extraction for TTS, accessibility, and structured output.
//!
//! Walks the chapter DOM collecting block-level elements (headings, paragraphs,
//! blockquotes, list items…) with their language tags and CFI coordinates.

use crate::model::ContentElement;
use kuchikiki::NodeRef;
use kuchikiki::traits::*;

// ── Public API ────────────────────────────────────────────────────────────────

/// Extracts a structured list of semantic content elements from an HTML chapter.
///
/// Each returned [`ContentElement`] carries the text content, its CFI coordinate
/// for precise highlighting, the HTML tag name, and an inherited language tag.
pub fn extract_semantic_content(html: &str, base_cfi: &str) -> Vec<ContentElement> {
    let document = kuchikiki::parse_html().one(html);
    let mut elements = Vec::new();

    // Resolve the document language from html[lang] / html[xml:lang]
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

    let html_node_path = "";

    if let Ok(html_node) = document.select_first("html") {
        let mut child_index = 0;
        let mut body_path = format!("{}/4", html_node_path);
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

// ── Private helpers ───────────────────────────────────────────────────────────

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

            let child_path = super::cfi_child_path(
                current_path, child_index, &super::cfi_assertion(&child),
            );

            let mut current_lang = inherited_lang.clone();
            if let Some(lang) = el.attributes.borrow().get("lang") {
                current_lang = Some(lang.to_string());
            } else if let Some(lang) = el.attributes.borrow().get("xml:lang") {
                current_lang = Some(lang.to_string());
            }

            let is_block = matches!(
                tag_name.as_str(),
                "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                    | "blockquote" | "li" | "dt" | "dd" | "figcaption"
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
                traverse_semantic_nodes(&child, base_cfi, &child_path, &current_lang, elements);
            }
        }
    }
}
