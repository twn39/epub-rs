//! HTML content processor using `lol_html` and `kuchikiki`.
//!
//! Organized into four focused submodules:
//!
//! | Submodule | Contents |
//! |-----------|----------|
//! | [`html`]  | lol_html-based streaming HTML rewriting and `<head>` injection |
//! | [`cfi`]   | CFI `data-` attribute injection and regex search with CFI ranges |
//! | [`positions`] | Internal: in-chapter DOM CFI markers only — package progression is `parser::positions` / `generate_locations` |
//! | [`semantic`]  | Semantic block extraction for TTS and accessibility |
//!
//! Path joining / `normalize_path` live in [`crate::path`] so HTML/CSS rewrite
//! share one policy with navigation and the generator.
//!
//! CSS URL scanning lives in [`rewrite`] — both [`html`] and [`css`] depend on
//! it (no mutual HTML↔CSS ownership).
//!
//! All public items are re-exported at this level so every existing caller of
//! `crate::processor::foo()` continues to work without any changes.

mod cfi;
mod css;
mod html;
mod positions;
mod prepare;
mod rewrite;
mod semantic;

// ── Shared DOM traversal helpers ──────────────────────────────────────────────
//
// These two pure functions are used by `cfi`, `positions`, and `semantic`.
// They are private to the `processor` module (accessible to child modules via
// `super::`) but not exposed to the rest of the crate, keeping the public API
// minimal.
//
// Centralising the CFI path formula here means a single fix propagates to all
// four DOM-walking operations that depend on it.

/// Extract the `id` attribute of a kuchikiki node and format it as a CFI assertion
/// string, e.g. `[chap01]`.  Returns an empty string when no `id` is present.
fn cfi_assertion(node: &kuchikiki::NodeRef) -> String {
    node.as_element()
        .and_then(|el| el.attributes.borrow().get("id").map(|s| s.to_string()))
        .map(|id| format!("[{id}]"))
        .unwrap_or_default()
}

/// Build a CFI child-step path segment:  `{parent}/{index}{assertion}`.
///
/// In EPUB CFI, element nodes occupy even indices (2, 4, 6 …).
/// The caller increments `index` by 2 for each element child encountered.
fn cfi_child_path(parent: &str, index: usize, assertion: &str) -> String {
    format!("{parent}/{index}{assertion}")
}

// ── Re-exports ────────────────────────────────────────────────────────────────

pub use crate::path::normalize_path;
pub use css::rewrite_css;
pub use html::{
    extract_text, extract_text_stream, inject_head_content, rewrite_links, rewrite_links_stream,
    rewrite_resources,
};

pub use cfi::{SearchResult, inject_cfi_dom, search_chapter};

// positions exports intentionally omitted — PositionContext and extract_positions
// are pub(crate) internal utilities; external callers use EpubArchive::generate_locations().

pub use prepare::{PrepareChapterOptions, data_uri, guess_media_type, prepare_chapter_html};
pub use semantic::extract_semantic_content;
