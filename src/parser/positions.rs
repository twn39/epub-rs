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

/// Strategy that uses the **original plaintext** byte length from `encryption.xml`.
///
/// Use this when the EPUB contains AES-CBC / LCP full-content encryption, where the
/// stored cipher-text is larger than the original content (due to AES padding and IV).
/// [`ArchiveEntryLength`] would over-count positions in that scenario.
///
/// The position count is read from the `OriginalLength` field of
/// [`crate::crypto::EncryptionInfo`], which is parsed from the
/// `<comp:Compression OriginalLength="N">` element in `META-INF/encryption.xml`.
/// The strategy is applied by [`super::EpubArchive::positions_by_reading_order`]
/// automatically when a spine item is found in the encryption map and has a known
/// original length.
///
/// For entries **without** a `<Compression OriginalLength>` annotation (e.g. IDPF/Adobe
/// font obfuscation), the strategy transparently falls back to the archive entry length,
/// because font obfuscation only XORs the header bytes and does not change the file size.
///
/// # Comparison with go-toolkit
///
/// go-toolkit's `OriginalLength` strategy exists in `positions_service.go` but ships
/// with a `math.Min` / `math.Max` bug that caps every result to 1. This implementation
/// is correct: `max(ceil(original_length / page_length), 1)`.
pub struct OriginalLength {
    /// Number of bytes per reading position. Typically 1024.
    pub page_length: usize,
}

impl ReflowableStrategy for OriginalLength {
    fn position_count(&self, entry_length: u64) -> usize {
        let page_len = (self.page_length.max(1)) as u64;
        entry_length.div_ceil(page_len).max(1) as usize
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

        let mut result: Vec<Vec<crate::model::Position>> = Vec::with_capacity(linear_items.len());

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
            // Reflowable: choose the effective byte length, then delegate to strategy.
            let position_count = if is_fixed {
                1usize
            } else {
                // Resolve the ZIP path the same way read_resource_by_href does, so that
                // the encryption map lookup uses the same key that was inserted at parse time.
                let zip_path = if book.opf_dir.is_empty() {
                    manifest_item.href.clone()
                } else {
                    super::EpubArchive::<P>::normalize_path(&book.opf_dir, &manifest_item.href)
                };

                // Archive entry length (uncompressed ZIP size) — the base measurement.
                let archive_len = self.provider.entry_length(&zip_path).unwrap_or(0);

                // Use the declared original plaintext length when available.
                // This corrects position counts for LCP / AES-CBC encrypted EPUBs where
                // the stored cipher-text is larger than the actual content.
                // For IDPF/Adobe font obfuscation (original_length = None), archive_len
                // is the correct value since those algorithms preserve the file size.
                let effective_len = book
                    .encryptions
                    .get(&zip_path)
                    .and_then(|enc| enc.original_length)
                    .unwrap_or(archive_len);

                strategy.position_count(effective_len)
            };

            let base_cfi = crate::cfi::EpubCfi::generate_spine_base_cfi(*spine_index, &item.idref);
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

    /// Generate positions **and** build a [`PositionIndex`] for bidirectional
    /// CFI ↔ location-index conversion in a single call.
    ///
    /// Equivalent to calling [`positions_by_reading_order`] followed by
    /// [`PositionIndex::build`], but avoids an intermediate flatten/collect.
    ///
    /// `bytes_per_position`: pass `0` to use the Readium/Adobe default of 1024 bytes.
    pub fn generate_location_index(
        &mut self,
        book: &EpubBook,
        bytes_per_position: usize,
    ) -> Result<PositionIndex, EpubError> {
        let strategy = ArchiveEntryLength {
            page_length: if bytes_per_position == 0 {
                BYTES_PER_POSITION
            } else {
                bytes_per_position
            },
        };
        let by_chapter = self.positions_by_reading_order(book, &strategy)?;
        Ok(PositionIndex::build(by_chapter))
    }
}

// ── PositionIndex — bidirectional CFI ↔ location-index conversion ─────────────

/// Bidirectional index for converting between CFI strings and 0-based position
/// indices at O(1) cost per query.
///
/// # Design
///
/// The CFIs generated by [`EpubArchive::generate_locations`] have a closed-form
/// mathematical structure:
///
/// ```text
/// spine_step  = (spine_index + 1) * 2       (/6/N where N is even)
/// local_step  = p * 2                        (/4/K where K = p*2, or /4 for p=0)
/// ```
///
/// This lets us compute the position index **directly** from any CFI by:
/// 1. Extracting `spine_step` → `spine_index` → reading-order index (HashMap, O(1))
/// 2. Extracting `first_content_step / 2` → position within chapter (integer division)
///
/// No binary search, no re-parsing of the position list, no pre-built sorted vector.
///
/// # Construction
///
/// Build with [`PositionIndex::build`] from the output of
/// [`EpubArchive::positions_by_reading_order`]. The build step is O(n) and
/// performs **zero CFI string parsing** — it only records chapter sizes.
///
/// Use [`EpubArchive::generate_location_index`] to generate positions and build
/// the index in a single call.
///
/// # Querying
///
/// | Method | Complexity |
/// |--------|-----------|
/// | [`location_from_cfi`] | O(\\|cfi_str\\|) ≈ O(1) |
/// | [`cfi_from_location`] | O(1) |
/// | [`position_at`]       | O(1) |
/// | [`location_from_progression`] | O(1) |
pub struct PositionIndex {
    /// Flat position list. O(1) access by index.
    positions: Vec<crate::model::Position>,

    /// `chapter_starts[k]` = index in `positions` where reading-order chapter `k` begins.
    /// `chapter_starts[num_chapters]` = `positions.len()` (sentinel).
    chapter_starts: Vec<usize>,

    /// Maps `full_spine_index` (0-based index in the **full** spine, including non-linear)
    /// to reading-order index (0-based index among linear items only).
    spine_to_order: HashMap<usize, usize>,
}

impl PositionIndex {
    /// Build the index from chapter-grouped positions.
    ///
    /// Consumes `by_chapter` (the output of [`positions_by_reading_order`]).
    /// Zero CFI parsing — O(n) time, O(k) extra memory where k = number of chapters.
    pub fn build(by_chapter: Vec<Vec<crate::model::Position>>) -> Self {
        let total: usize = by_chapter.iter().map(|c| c.len()).sum();
        let mut positions = Vec::with_capacity(total);
        let mut chapter_starts = Vec::with_capacity(by_chapter.len() + 1);
        let mut spine_to_order = HashMap::with_capacity(by_chapter.len());

        chapter_starts.push(0);
        for (order_idx, chapter) in by_chapter.into_iter().enumerate() {
            if let Some(first) = chapter.first() {
                spine_to_order.insert(first.spine_index, order_idx);
            }
            positions.extend(chapter);
            chapter_starts.push(positions.len());
        }

        Self {
            positions,
            chapter_starts,
            spine_to_order,
        }
    }

    /// Find which 0-based position index a CFI falls into.
    ///
    /// Works for **any** CFI — position CFIs (generated by `generate_locations`),
    /// character-level bookmark CFIs, and annotation range CFIs. For range CFIs
    /// the start endpoint is used.
    ///
    /// **Semantics:** returns the largest `i` such that `positions[i].cfi <= cfi`.
    /// Equivalent to epub.js `Locations.locationFromCfi()`.
    ///
    /// Returns `None` if the index is empty or the CFI's spine item is not a
    /// linear reading-order chapter (e.g. a non-linear footnote).
    ///
    /// # How it works
    ///
    /// ```text
    /// CFI: epubcfi(/6/4[ch1]!/4/6)
    ///              └─┬─┘  └┬┘ └┬┘
    ///           spine_step  │   first_content_step = 6
    ///           = 4         │   → p = 6 / 2 = 3
    ///           → spine_index = 4/2 - 1 = 1
    ///           → reading_order_index = spine_to_order[1]
    ///           → global_idx = chapter_starts[order] + p
    /// ```
    pub fn location_from_cfi(&self, cfi_str: &str) -> Option<usize> {
        use std::str::FromStr;

        if self.positions.is_empty() {
            return None;
        }

        let cfi = crate::cfi::EpubCfi::from_str(cfi_str).ok()?;

        // For Range CFIs use the parent path (which carries the spine step and
        // the shared local path prefix up to the common ancestor).
        let path = match &cfi {
            crate::cfi::EpubCfi::Point(p) => p,
            crate::cfi::EpubCfi::Range { parent, .. } => parent,
        };

        // path.steps = [/6, /N, ...]
        // steps[1].index = spine_step = (full_spine_index + 1) * 2
        let spine_step = path.steps.get(1)?.index as usize;
        let full_spine_index = spine_step / 2 - 1;
        let order_idx = *self.spine_to_order.get(&full_spine_index)?;

        // local_steps = [/4, /K, ...] where /4 is the <body> element.
        // local_steps[1].index = first_content_step = p * 2
        // For p=0 (chapter root), there is no local_steps[1].
        let local = path.local_steps.as_deref().unwrap_or(&[]);
        let first_content_step = local.get(1).map(|s| s.index as usize).unwrap_or(0);
        let p_within_chapter = first_content_step / 2;

        let chapter_start = self.chapter_starts[order_idx];
        let chapter_len = self.chapter_starts[order_idx + 1] - chapter_start;

        // Clamp to valid chapter range: a user bookmark at the very end of the
        // chapter may produce a step beyond the last position boundary.
        let p_clamped = p_within_chapter.min(chapter_len.saturating_sub(1));
        Some(chapter_start + p_clamped)
    }

    /// Return the CFI string for a given 0-based position index. O(1).
    ///
    /// Equivalent to epub.js `Locations.cfiFromLocation()`.
    pub fn cfi_from_location(&self, idx: usize) -> Option<&str> {
        self.positions.get(idx).map(|p| p.cfi.as_str())
    }

    /// Return the full [`Position`] for a given 0-based index. O(1).
    pub fn position_at(&self, idx: usize) -> Option<&crate::model::Position> {
        self.positions.get(idx)
    }

    /// Convert a total-progression value (0.0–1.0) to the nearest position index.
    ///
    /// Equivalent to epub.js `Locations.cfiFromPercentage()` (returns index, not CFI).
    /// Use [`cfi_from_location`] on the result to obtain the CFI string.
    /// O(1).
    pub fn location_from_progression(&self, progression: f32) -> Option<usize> {
        let n = self.positions.len();
        if n == 0 {
            return None;
        }
        let idx = (progression.clamp(0.0, 1.0) * n as f32) as usize;
        Some(idx.min(n - 1))
    }

    /// Total number of positions. Equivalent to epub.js `Locations.total + 1`.
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    /// Returns `true` if the index contains no positions.
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EpubBook, LayoutType, ManifestItem, Metadata, SpineItem, TocEntry};
    use std::collections::HashMap;

    // ── ArchiveEntryLength::position_count ────────────────────────────────────

    #[test]
    fn position_count_empty_file_returns_one() {
        let s = ArchiveEntryLength { page_length: 1024 };
        assert_eq!(
            s.position_count(0),
            1,
            "empty file must produce at least 1 position"
        );
    }

    #[test]
    fn position_count_exact_one_page() {
        let s = ArchiveEntryLength { page_length: 1024 };
        assert_eq!(s.position_count(1024), 1);
    }

    #[test]
    fn position_count_one_byte_over_page() {
        let s = ArchiveEntryLength { page_length: 1024 };
        assert_eq!(s.position_count(1025), 2, "1025 bytes → ceil(1025/1024)=2");
    }

    #[test]
    fn position_count_exactly_ten_pages() {
        let s = ArchiveEntryLength { page_length: 1024 };
        assert_eq!(s.position_count(10240), 10);
    }

    #[test]
    fn position_count_just_over_ten_pages() {
        let s = ArchiveEntryLength { page_length: 1024 };
        assert_eq!(s.position_count(10241), 11);
    }

    #[test]
    fn position_count_custom_page_length() {
        // 512 bytes / 2048 page_length → ceil=1 (less than one page)
        let s = ArchiveEntryLength { page_length: 2048 };
        assert_eq!(s.position_count(512), 1);
    }

    #[test]
    fn position_count_page_length_zero_treated_as_one() {
        // page_length of 0 is clamped to 1 via .max(1)
        let s = ArchiveEntryLength { page_length: 0 };
        // 100 bytes / max(0,1)=1 → 100 positions
        assert_eq!(s.position_count(100), 100);
    }

    // ── build_title_map ───────────────────────────────────────────────────────

    #[test]
    fn title_map_bare_href() {
        let toc = vec![TocEntry {
            title: "Chapter 1".to_string(),
            href: "text/ch1.xhtml".to_string(),
            children: vec![],
        }];
        let map = build_title_map(&toc);
        assert_eq!(
            map.get("text/ch1.xhtml").map(String::as_str),
            Some("Chapter 1")
        );
    }

    #[test]
    fn title_map_strips_fragment() {
        let toc = vec![TocEntry {
            title: "Section".to_string(),
            href: "text/ch1.xhtml#section-2".to_string(),
            children: vec![],
        }];
        let map = build_title_map(&toc);
        // The fragment-stripped key must exist; the raw key must NOT
        assert!(
            map.contains_key("text/ch1.xhtml"),
            "fragment should be stripped"
        );
        assert!(
            !map.contains_key("text/ch1.xhtml#section-2"),
            "raw key with fragment must not appear"
        );
    }

    #[test]
    fn title_map_first_title_wins_for_duplicate_href() {
        // Two TOC entries pointing to the same file; first title should win (or_insert_with semantics)
        let toc = vec![
            TocEntry {
                title: "First Title".to_string(),
                href: "ch.xhtml".to_string(),
                children: vec![],
            },
            TocEntry {
                title: "Second Title".to_string(),
                href: "ch.xhtml".to_string(),
                children: vec![],
            },
        ];
        let map = build_title_map(&toc);
        assert_eq!(map.get("ch.xhtml").map(String::as_str), Some("First Title"));
    }

    #[test]
    fn title_map_indexes_nested_children() {
        let toc = vec![TocEntry {
            title: "Chapter".to_string(),
            href: "ch.xhtml".to_string(),
            children: vec![TocEntry {
                title: "Section".to_string(),
                href: "ch.xhtml#sec1".to_string(),
                children: vec![],
            }],
        }];
        let map = build_title_map(&toc);
        assert!(map.contains_key("ch.xhtml"), "parent entry must be indexed");
        // child uses same base href (after strip) — first-wins means parent title holds
        assert_eq!(map.get("ch.xhtml").map(String::as_str), Some("Chapter"));
    }

    // ── Mathematical invariants (using in-memory EPUB via EpubBook) ───────────
    //
    // These tests construct an EpubBook directly (no ZIP) and call
    // positions_by_reading_order via a lightweight mock provider.
    //
    // The invariants below MUST hold for ANY valid EPUB input.

    /// Shared invariant checker — can be called from any test.
    fn assert_position_invariants(all: &[Vec<crate::model::Position>]) {
        let flat: Vec<_> = all.iter().flatten().collect();
        if flat.is_empty() {
            return;
        }

        // 1. global_position is 1-based and contiguous
        for (i, pos) in flat.iter().enumerate() {
            assert_eq!(
                pos.global_position,
                i + 1,
                "global_position must equal 1-based index but got {} at i={}",
                pos.global_position,
                i,
            );
        }

        // 2. total_progression ∈ [0.0, 1.0)  (last pos < 1.0)
        for pos in &flat {
            assert!(
                pos.total_progression >= 0.0 && pos.total_progression < 1.0 + f32::EPSILON,
                "total_progression out of range: {}",
                pos.total_progression,
            );
        }

        // 3. First position always has total_progression = 0.0
        assert_eq!(
            flat[0].total_progression, 0.0,
            "first position must have total_progression = 0.0",
        );

        // 4. chapter_progression ∈ [0.0, 1.0)
        for pos in &flat {
            assert!(
                pos.chapter_progression >= 0.0 && pos.chapter_progression < 1.0 + f32::EPSILON,
                "chapter_progression out of range: {}",
                pos.chapter_progression,
            );
        }

        // 5. Total position count consistent with last global_position
        assert_eq!(
            flat.last().unwrap().global_position,
            flat.len(),
            "last global_position must equal total count",
        );
    }

    /// Minimal in-memory provider that returns a fixed byte length for every href.
    struct FixedLengthProvider(u64);
    impl crate::provider::EpubProvider for FixedLengthProvider {
        fn read_file<'a>(
            &'a mut self,
            _path: &str,
        ) -> Result<Box<dyn std::io::Read + 'a>, crate::error::EpubError> {
            Ok(Box::new(std::io::Cursor::new(vec![0u8; self.0 as usize])))
        }
        fn entry_length(&mut self, _path: &str) -> Result<u64, crate::error::EpubError> {
            Ok(self.0)
        }
    }

    /// Build a minimal EpubBook with N reflowable linear chapters.
    fn make_book(n: usize, layout: LayoutType) -> EpubBook {
        let mut manifest = HashMap::new();
        let mut spine = Vec::new();
        for i in 0..n {
            let id = format!("ch{}", i);
            let href = format!("text/ch{}.xhtml", i);
            manifest.insert(
                id.clone(),
                ManifestItem {
                    id: id.clone(),
                    href,
                    media_type: "application/xhtml+xml".to_string(),
                    properties: vec![],
                    media_overlay: None,
                },
            );
            spine.push(SpineItem {
                idref: id,
                linear: true,
                layout_override: None,
                page_spread: None,
            });
        }
        EpubBook {
            metadata: Metadata {
                layout,
                ..Default::default()
            },
            manifest,
            spine,
            opf_dir: String::new(),
            toc_id: None,
            encryptions: HashMap::new(),
        }
    }

    #[test]
    fn positions_single_chapter_invariants() {
        let book = make_book(1, LayoutType::Reflowable);
        // 1 chapter, file size = 2048 bytes → 2 positions at 1024 bytes/pos
        let mut archive = super::super::EpubArchive::new_with_provider(FixedLengthProvider(2048));
        let strategy = ArchiveEntryLength { page_length: 1024 };
        let positions = archive
            .positions_by_reading_order(&book, &strategy)
            .unwrap();

        assert_eq!(positions.len(), 1, "one chapter → one group");
        assert_eq!(positions[0].len(), 2, "2048 bytes / 1024 = 2 positions");
        assert_position_invariants(&positions);
    }

    #[test]
    fn positions_multi_chapter_invariants() {
        let book = make_book(3, LayoutType::Reflowable);
        // 3 chapters, each 3072 bytes → 3 positions each → 9 total
        let mut archive = super::super::EpubArchive::new_with_provider(FixedLengthProvider(3072));
        let strategy = ArchiveEntryLength { page_length: 1024 };
        let positions = archive
            .positions_by_reading_order(&book, &strategy)
            .unwrap();

        assert_eq!(positions.len(), 3);
        assert_eq!(positions.iter().map(|c| c.len()).sum::<usize>(), 9);
        assert_position_invariants(&positions);

        // Chapter boundary: first pos of chapter 2 continues from chapter 1
        assert_eq!(
            positions[1][0].global_position,
            positions[0].last().unwrap().global_position + 1
        );
    }

    #[test]
    fn positions_fixed_layout_always_one_per_chapter() {
        let book = make_book(4, LayoutType::PrePaginated);
        // Even with huge files, fixed-layout = 1 position per chapter
        let mut archive =
            super::super::EpubArchive::new_with_provider(FixedLengthProvider(1_000_000));
        let strategy = ArchiveEntryLength { page_length: 1024 };
        let positions = archive
            .positions_by_reading_order(&book, &strategy)
            .unwrap();

        assert_eq!(positions.len(), 4);
        for chapter in &positions {
            assert_eq!(
                chapter.len(),
                1,
                "fixed-layout chapter must have exactly 1 position"
            );
        }
        assert_position_invariants(&positions);
    }

    #[test]
    fn positions_nonlinear_items_excluded() {
        // Build a book where spine item 2 is non-linear (e.g. pop-up footnote)
        let mut book = make_book(3, LayoutType::Reflowable);
        book.spine[1].linear = false;

        let mut archive = super::super::EpubArchive::new_with_provider(FixedLengthProvider(1024));
        let strategy = ArchiveEntryLength { page_length: 1024 };
        let positions = archive
            .positions_by_reading_order(&book, &strategy)
            .unwrap();

        // Only 2 linear chapters should appear
        assert_eq!(positions.len(), 2, "non-linear items must be excluded");
        assert_position_invariants(&positions);
    }

    #[test]
    fn positions_cfi_format_first_position() {
        let book = make_book(1, LayoutType::Reflowable);
        let mut archive = super::super::EpubArchive::new_with_provider(FixedLengthProvider(2048));
        let strategy = ArchiveEntryLength { page_length: 1024 };
        let positions = archive
            .positions_by_reading_order(&book, &strategy)
            .unwrap();

        // Position 0 (p=0): format must be epubcfi(.../6/N!/4)
        let cfi0 = &positions[0][0].cfi;
        assert!(cfi0.starts_with("epubcfi("), "CFI must start with epubcfi(");
        assert!(
            cfi0.ends_with("!/4)"),
            "first position CFI must end with !/4)"
        );

        // Position 1 (p=1): format must be epubcfi(.../6/N!/4/2)  (p*2=2)
        let cfi1 = &positions[0][1].cfi;
        assert!(
            cfi1.ends_with("!/4/2)"),
            "second position CFI must end with !/4/2)"
        );
    }

    // ── PositionIndex ─────────────────────────────────────────────────────────

    /// Build a PositionIndex from N chapters, each with the given number of positions.
    fn build_index(n_chapters: usize, positions_per_chapter: usize) -> PositionIndex {
        // Each chapter is `positions_per_chapter * 1024` bytes with 1024 bytes/position.
        let byte_len = (positions_per_chapter * 1024) as u64;
        let book = make_book(n_chapters, LayoutType::Reflowable);
        let mut archive =
            super::super::EpubArchive::new_with_provider(FixedLengthProvider(byte_len));
        let strategy = ArchiveEntryLength { page_length: 1024 };
        let by_chapter = archive
            .positions_by_reading_order(&book, &strategy)
            .unwrap();
        PositionIndex::build(by_chapter)
    }

    #[test]
    fn position_index_empty_returns_none() {
        let idx = PositionIndex::build(vec![]);
        assert!(idx.is_empty());
        assert_eq!(idx.len(), 0);
        assert!(idx.location_from_cfi("epubcfi(/6/2!/4)").is_none());
        assert!(idx.cfi_from_location(0).is_none());
        assert!(idx.location_from_progression(0.5).is_none());
    }

    #[test]
    fn position_index_single_chapter_first_position() {
        // 1 chapter, 3 positions → CFIs: /6/2!/4, /6/2!/4/2, /6/2!/4/4
        let idx = build_index(1, 3);
        assert_eq!(idx.len(), 3);

        // Chapter-root CFI (p=0): no local_steps[1]
        let loc = idx.location_from_cfi("epubcfi(/6/2[ch0]!/4)").unwrap();
        assert_eq!(loc, 0, "chapter root must map to position 0");
    }

    #[test]
    fn position_index_single_chapter_second_position() {
        let idx = build_index(1, 3);
        // p=1 → local_step = /4/2 → first_content_step = 2 → 2/2 = 1
        let loc = idx.location_from_cfi("epubcfi(/6/2[ch0]!/4/2)").unwrap();
        assert_eq!(loc, 1);
    }

    #[test]
    fn position_index_single_chapter_third_position() {
        let idx = build_index(1, 3);
        // p=2 → local_step = /4/4 → 4/2 = 2
        let loc = idx.location_from_cfi("epubcfi(/6/2[ch0]!/4/4)").unwrap();
        assert_eq!(loc, 2);
    }

    #[test]
    fn position_index_multi_chapter_boundaries() {
        // 3 chapters, 2 positions each → global indices 0-1, 2-3, 4-5
        let idx = build_index(3, 2);
        assert_eq!(idx.len(), 6);

        // Chapter 1 first pos (spine_index=0 → step=2)
        assert_eq!(idx.location_from_cfi("epubcfi(/6/2[ch0]!/4)").unwrap(), 0);
        // Chapter 1 second pos
        assert_eq!(idx.location_from_cfi("epubcfi(/6/2[ch0]!/4/2)").unwrap(), 1);
        // Chapter 2 first pos (spine_index=1 → step=4)
        assert_eq!(idx.location_from_cfi("epubcfi(/6/4[ch1]!/4)").unwrap(), 2);
        // Chapter 2 second pos
        assert_eq!(idx.location_from_cfi("epubcfi(/6/4[ch1]!/4/2)").unwrap(), 3);
        // Chapter 3 first pos (spine_index=2 → step=6)
        assert_eq!(idx.location_from_cfi("epubcfi(/6/6[ch2]!/4)").unwrap(), 4);
        // Chapter 3 second pos
        assert_eq!(idx.location_from_cfi("epubcfi(/6/6[ch2]!/4/2)").unwrap(), 5);
    }

    #[test]
    fn position_index_cfi_from_location_roundtrip() {
        // Build index, get CFI from each position, look up the index, verify identity.
        let idx = build_index(3, 4);
        for i in 0..idx.len() {
            let cfi = idx
                .cfi_from_location(i)
                .expect("cfi must exist for valid index");
            let back = idx.location_from_cfi(cfi).expect("location must resolve");
            assert_eq!(back, i, "roundtrip failed at index {i}");
        }
    }

    #[test]
    fn position_index_deep_user_bookmark_cfi() {
        // A user bookmark with character offset, deeper than position boundaries.
        // epubcfi(/6/2!/4/2[para01]/1:42) — first_content_step = /4/2 → p=1
        let idx = build_index(1, 5);
        // Without the assertion bracket and char offset, step /2 → p=1
        let loc = idx.location_from_cfi("epubcfi(/6/2!/4/2/1:42)").unwrap();
        assert_eq!(loc, 1, "deep bookmark under /4/2 must map to position 1");
    }

    #[test]
    fn position_index_out_of_chapter_step_clamped() {
        // A CFI whose local step exceeds the chapter's position count gets clamped
        // to the last position in that chapter.
        let idx = build_index(1, 3); // positions 0,1,2 → steps /4, /4/2, /4/4
        // Step /4/100 (p=50) exceeds chapter size of 3 → clamped to 2 (last)
        let loc = idx.location_from_cfi("epubcfi(/6/2!/4/100)").unwrap();
        assert_eq!(
            loc, 2,
            "out-of-range step must clamp to last chapter position"
        );
    }

    #[test]
    fn position_index_nonlinear_spine_item_returns_none() {
        // spine_index=1 is non-linear and not included in the index.
        let mut book = make_book(3, LayoutType::Reflowable);
        book.spine[1].linear = false;
        let mut archive = super::super::EpubArchive::new_with_provider(FixedLengthProvider(1024));
        let strategy = ArchiveEntryLength { page_length: 1024 };
        let by_chapter = archive
            .positions_by_reading_order(&book, &strategy)
            .unwrap();
        let idx = PositionIndex::build(by_chapter);

        // spine_index=1 → step=4 — not in spine_to_order → None
        let result = idx.location_from_cfi("epubcfi(/6/4[ch1]!/4)");
        assert!(result.is_none(), "non-linear spine item must return None");
    }

    #[test]
    fn position_index_location_from_progression() {
        let idx = build_index(1, 10); // 10 positions
        assert_eq!(idx.location_from_progression(0.0).unwrap(), 0);
        assert_eq!(idx.location_from_progression(1.0).unwrap(), 9); // clamped
        assert_eq!(idx.location_from_progression(0.5).unwrap(), 5);
    }

    #[test]
    fn position_index_oob_location_returns_none() {
        let idx = build_index(1, 3);
        assert!(
            idx.cfi_from_location(3).is_none(),
            "index 3 is out of bounds for 3-position list"
        );
    }
}
