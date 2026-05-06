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
///
/// This is an internal type used by the crate's DOM-based character-offset position
/// extraction. External callers should use [`EpubArchive::generate_locations`] or
/// [`EpubArchive::positions_by_reading_order`] instead.
#[allow(dead_code)]
pub(crate) struct PositionContext<'a> {
    pub(crate) base_cfi: &'a str,
    pub(crate) chars_per_position: usize,
    pub(crate) spine_index: usize,
    pub(crate) href: &'a str,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Walks the DOM of `html`, emitting [`Position`] entries into `positions`
/// every `ctx.chars_per_position` characters.
///
/// Internal utility: provides DOM-level character-precise CFI positions.
/// External callers should use [`EpubArchive::generate_locations`] instead.
#[allow(dead_code)]
pub(crate) fn extract_positions(
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

#[allow(dead_code)]
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
            let child_path =
                super::cfi_child_path(current_path, child_index, &super::cfi_assertion(&child));
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

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// These tests were originally in tests/processor_tests.rs but were relocated here
// when PositionContext and extract_positions became pub(crate). Inline tests have
// full access to crate-private items; integration tests do not.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_positions_character_boundary_splitting() {
        let html = r#"<html><body><div id="content"><p>12345</p><p>67890</p><p>abcde</p></div></body></html>"#;

        let mut positions = Vec::new();
        let mut char_counter = 0;
        let mut global_pos = 0;

        // chars_per_position = 4.
        // "12345" (5 chars): emit after 4 chars → 1 position. Leftover: 1.
        // "67890" (5 chars): leftover(1)+5=6, emit after 3 chars → 1 position. Leftover: 2.
        // "abcde" (5 chars): leftover(2)+5=7, emit after 2 chars → 1 position. Leftover: 3.
        let ctx = PositionContext {
            base_cfi: "/6/4!",
            chars_per_position: 4,
            spine_index: 0,
            href: "test.xhtml",
        };

        extract_positions(
            html,
            &ctx,
            &mut char_counter,
            &mut positions,
            &mut global_pos,
        );

        assert_eq!(positions.len(), 3);

        // Match 1: in "12345", at offset 4  — body/4 → div/2[content] → p/2 → text/1
        assert_eq!(positions[0].cfi, "epubcfi(/6/4!/4/2[content]/2/1:4)");
        assert_eq!(positions[0].global_position, 1);

        // Match 2: in "67890", at offset 3  — body/4 → div/2[content] → p/4 → text/1
        assert_eq!(positions[1].cfi, "epubcfi(/6/4!/4/2[content]/4/1:3)");
        assert_eq!(positions[1].global_position, 2);

        // Match 3: in "abcde", at offset 2  — body/4 → div/2[content] → p/6 → text/1
        assert_eq!(positions[2].cfi, "epubcfi(/6/4!/4/2[content]/6/1:2)");
        assert_eq!(positions[2].global_position, 3);

        // Remaining chars that didn't reach the next boundary
        assert_eq!(char_counter, 3);
    }
}
