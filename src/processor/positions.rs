//! Character-offset to CFI position computation.
//!
//! Walks a parsed HTML DOM accumulating text character counts and emits
//! [`Position`] entries at every `chars_per_position` boundary — matching
//! the Adobe RMSDK / Readium standard (default: 1024 bytes/chars per position).

use crate::model::Position;
use kuchikiki::NodeRef;
use kuchikiki::traits::*;

// ── Public types ──────────────────────────────────────────────────────────────

/// Context passed into [`extract_positions`] for a single spine item.
pub struct PositionContext<'a> {
    pub base_cfi: &'a str,
    pub chars_per_position: usize,
    pub spine_index: usize,
    pub href: &'a str,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Walks the DOM of `html`, emitting [`Position`] entries into `positions`
/// every `ctx.chars_per_position` characters.
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

// ── Private helpers ───────────────────────────────────────────────────────────

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
            let child_path = super::cfi_child_path(
                current_path, child_index, &super::cfi_assertion(&child),
            );
            traverse_for_positions(&child, ctx, &child_path, char_counter, positions, global_pos);
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

                let cfi = format!(
                    "epubcfi({}!{}/{}:{})",
                    stripped_base, current_path, cfi_text_idx, offset,
                );

                positions.push(Position {
                    spine_index: ctx.spine_index,
                    href: ctx.href.to_string(),
                    cfi,
                    global_position: *global_pos,
                    chapter_progression: 0.0,
                    total_progression: 0.0,
                    title: None,
                });

                *char_counter = 0;
            }
            *char_counter += text_len - offset;
        }
    }
}
