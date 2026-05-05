//! Position computation strategy and reading-order position generation.
//!
//! Mirrors go-toolkit's `positions_service.go`.

use crate::error::EpubError;
use crate::model::EpubBook;
use crate::model::TocEntry;
use std::collections::HashMap;

// ── Strategy types (pub — re-exported from mod.rs) ───────────────────────────

/// Default bytes per reading position, matching the Adobe RMSDK and Readium standard.
///
/// See: <https://github.com/readium/architecture/issues/123>
pub const BYTES_PER_POSITION: usize = 1024;

/// Strategy for computing the number of positions in a reflowable spine item.
///
/// Mirrors go-toolkit's `ReflowableStrategy` interface (`positions_service.go`).
/// A fixed-layout spine item always produces exactly 1 position, regardless of strategy.
pub trait ReflowableStrategy: Send + Sync {
    /// Returns the number of positions (≥ 1) for a spine item with the given byte length.
    fn position_count(&self, entry_length: u64) -> usize;
}

/// Strategy that uses the uncompressed ZIP entry length divided by `page_length`.
///
/// This is the **recommended** strategy, matching Adobe RMSDK and Readium defaults.
///
/// Equivalent to go-toolkit's `ArchiveEntryLength` (not `OriginalLength`, which has
/// a bug where it uses `math.Min` instead of `math.Max`, capping results at 1).
pub struct ArchiveEntryLength {
    /// Number of bytes per reading position. Typically 1024.
    pub page_length: usize,
}

impl ReflowableStrategy for ArchiveEntryLength {
    fn position_count(&self, entry_length: u64) -> usize {
        // max(ceil(entry_length / page_length), 1)
        // Ensures at least 1 position even for empty or very small files.
        let page_len = (self.page_length.max(1)) as u64;
        let count = entry_length.div_ceil(page_len) as usize;
        count.max(1)
    }
}

/// Returns the recommended reflowable strategy: `ArchiveEntryLength { page_length: 1024 }`.
pub fn recommended_reflowable_strategy() -> ArchiveEntryLength {
    ArchiveEntryLength {
        page_length: BYTES_PER_POSITION,
    }
}

// ── EpubArchive impl — position computation ───────────────────────────────────

use super::EpubArchive;
use crate::provider::EpubProvider;

impl<P: EpubProvider> EpubArchive<P> {
    /// Generate virtual pages (locations) for the entire EPUB based on a character limit.
    /// Returns positions grouped by spine item (reading order).
    ///
    /// The outer `Vec` index corresponds to the **reading order index** (i.e. only linear
    /// spine items are included; non-linear items such as pop-up footnotes are skipped).
    ///
    /// Mirrors go-toolkit's `PositionsByReadingOrder()` / `computePositions()`.
    ///
    /// The `strategy` parameter controls how position counts are computed for reflowable
    /// resources. Use [`recommended_reflowable_strategy()`] for the Adobe/Readium standard.
    pub fn positions_by_reading_order(
        &mut self,
        book: &EpubBook,
        strategy: &dyn ReflowableStrategy,
    ) -> Result<Vec<Vec<crate::model::Position>>, EpubError> {
        // Build href -> title map from the TOC for position title enrichment
        let toc = self.get_toc(book).unwrap_or_default();
        let title_map = build_title_map(&toc);

        // Collect only linear spine items — non-linear items don't count toward reading progress
        // (mirrors go-toolkit which skips `linear=false` items in reading-order traversal)
        let linear_items: Vec<(usize, &crate::model::SpineItem)> = book
            .spine
            .iter()
            .enumerate()
            .filter(|(_, item)| item.linear)
            .collect();

        let mut result: Vec<Vec<crate::model::Position>> =
            Vec::with_capacity(linear_items.len());

        // `last_position` carries the last global_position from the previous chapter,
        // exactly like go-toolkit's `lastPositionOfPreviousResource`.
        let mut last_position: usize = 0;

        // ── Pass 1: compute per-chapter positions with local progressions ─────
        for (spine_index, item) in &linear_items {
            let manifest_item = book.manifest.get(&item.idref).ok_or_else(|| {
                EpubError::InvalidFormat(format!("Missing manifest item: {}", item.idref))
            })?;

            // Determine the effective layout for this spine item:
            // the item-level override takes precedence over the publication-level default.
            let is_fixed = matches!(
                item.layout_override.unwrap_or(book.metadata.layout),
                crate::model::LayoutType::PrePaginated
            );

            // Fixed layout: always 1 position.
            // Reflowable: delegate to the strategy (ArchiveEntryLength by default).
            let position_count = if is_fixed {
                1usize
            } else {
                let byte_len = self
                    .provider
                    .entry_length(&manifest_item.href)
                    .unwrap_or(0); // graceful degradation for missing/unreadable files
                strategy.position_count(byte_len)
            };

            let base_cfi =
                crate::cfi::EpubCfi::generate_spine_base_cfi(*spine_index, &item.idref);
            let spine_path = base_cfi.trim_end_matches('!');

            // Look up the chapter title from the TOC (strip fragment from href for matching)
            let href_key = manifest_item
                .href
                .split('#')
                .next()
                .unwrap_or(&manifest_item.href);
            let title = title_map.get(href_key).cloned();

            let chapter_positions: Vec<crate::model::Position> = (0..position_count)
                .map(|p| {
                    // global_position is 1-based and continues monotonically across chapters.
                    // Formula: startPosition + p + 1  (identical to go-toolkit's createReflowable)
                    let global_position = last_position + p + 1;

                    // chapter_progression = p / position_count
                    // (0.0 for the first position in the chapter, approaching 1.0)
                    // Formula mirrors go-toolkit: `float64(p) / float64(positionCount)`
                    let chapter_progression = if position_count <= 1 {
                        0.0f32
                    } else {
                        p as f32 / position_count as f32
                    };

                    // CFI generation (without DOM parsing):
                    //   position 0 or fixed-layout → document root element (/4)
                    //   position N > 0             → /4/N*2 (even step = element, per CFI spec)
                    let cfi = if p == 0 || is_fixed {
                        format!("epubcfi({}!/4)", spine_path)
                    } else {
                        format!("epubcfi({}!/4/{})", spine_path, p * 2)
                    };

                    crate::model::Position {
                        spine_index: *spine_index,
                        href: manifest_item.href.clone(),
                        cfi,
                        global_position,
                        chapter_progression,
                        total_progression: 0.0, // filled in Pass 2
                        title: title.clone(),
                    }
                })
                .collect();

            last_position += position_count;
            result.push(chapter_positions);
        }

        // ── Pass 2: compute totalProgression ──────────────────────────────────
        // total_page_count = last global_position reached across all chapters.
        // Formula: (position - 1) / total_page_count  (identical to go-toolkit's computePositions)
        let total_page_count = last_position;
        if total_page_count > 0 {
            for chapter in &mut result {
                for loc in chapter.iter_mut() {
                    loc.total_progression =
                        (loc.global_position - 1) as f32 / total_page_count as f32;
                }
            }
        }

        Ok(result)
    }

    /// Returns a flat list of all reading positions across the entire EPUB.
    ///
    /// This is a convenience wrapper around [`positions_by_reading_order`] that flattens the
    /// per-chapter grouping. Mirrors go-toolkit's `Positions()`.
    ///
    /// `bytes_per_position` sets the granularity of reflowable positions.
    /// Pass `0` (or [`BYTES_PER_POSITION`]) to use the Readium/Adobe default of 1024 bytes.
    pub fn generate_locations(
        &mut self,
        book: &EpubBook,
        bytes_per_position: usize,
    ) -> Result<Vec<crate::model::Position>, EpubError> {
        let strategy = ArchiveEntryLength {
            page_length: if bytes_per_position == 0 {
                BYTES_PER_POSITION
            } else {
                bytes_per_position
            },
        };
        let by_chapter = self.positions_by_reading_order(book, &strategy)?;
        Ok(by_chapter.into_iter().flatten().collect())
    }
}

/// Builds a flat `href → title` lookup map from a TOC entry tree.
///
/// Used by [`EpubArchive::positions_by_reading_order`] to enrich each `Position`
/// with the chapter title from the table of contents.
///
/// The href key has any fragment suffix (`#anchor`) stripped so it matches the
/// bare file path stored in the manifest.
pub(super) fn build_title_map(toc: &[TocEntry]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    fn recurse(entries: &[TocEntry], map: &mut HashMap<String, String>) {
        for entry in entries {
            // Strip fragment so `chapter.xhtml#section1` matches `chapter.xhtml`
            let key = entry
                .href
                .split('#')
                .next()
                .unwrap_or(&entry.href)
                .to_string();
            // Only set the first title seen for a given href (most specific wins for TOC order)
            map.entry(key).or_insert_with(|| entry.title.clone());
            recurse(&entry.children, map);
        }
    }
    recurse(toc, &mut map);
    map
}
