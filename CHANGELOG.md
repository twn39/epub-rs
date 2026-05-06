# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-05-06

### Added

- **`PositionIndex`** — O(1) bidirectional CFI ↔ location-index conversion:
  - `PositionIndex::build(by_chapter)` — constructs the index from a
    `positions_by_reading_order` result using closed-form integer arithmetic
  - `location_from_cfi(cfi)` → `Option<usize>` — find the 0-based location index for any CFI
  - `cfi_from_location(idx)` → `Option<&EpubCfi>` — retrieve the CFI for a given index
  - `location_from_progression(pct)` → `Option<usize>` — map a 0–1 reading percentage to an index
  - `EpubArchive::generate_location_index(book, bpp)` — high-level entry point
- **FFI (C API) — new exports** (requires `--features ffi`):
  - `epub_generate_location_index` / `epub_location_from_cfi` / `epub_cfi_from_location`
  - `epub_generator_set_toc` / `epub_generator_set_metadata` / `epub_validate`
  - `epub_get_cover_image` / `epub_get_resource_by_href` / `epub_get_resource_by_id`
  - `epub_get_chapter_with_cfi` / `epub_search_chapter` / `epub_get_semantic_content`
  - `epub_generate_locations` / `epub_decrypt_font` / `epub_cfi_compare`
  - `epub_cfi_generate_range` / `epub_cfi_from_spine_index`
  - Auto-generated `include/epub_rs.h` updated with all new symbols
- **WASM** — symmetric new methods on `EpubParser`:
  `generate_location_index` / `location_from_cfi` / `cfi_from_location`
- **`TitleEntry` model** — structured multi-title support with `lang`, `title_type`,
  `sort_as`, `display_seq` fields; EPUB 3 `display-seq` refinements now respected
- **`NavigationDocument`** — unified single-pass `get_navigation()` reading all three
  navigation structures (TOC, page-list, landmarks) at once

### Changed

- **`ZipProvider::resolve_zip_name`** — lazy fallback cache: O(1) exact-match (unchanged)
  for well-formed EPUBs; O(n) scan for broken paths cached after first access so
  repeated broken-path lookups are O(1) with zero startup cost
- **`processor`** — split monolithic `processor.rs` into focused submodules:
  `processor/html.rs`, `processor/positions.rs`, `processor/semantic.rs`
- **`parser`** — split monolithic `parser.rs` into `parser/mod.rs`, `parser/opf.rs`,
  `parser/navigation.rs`, `parser/positions.rs`
- `epub_relative_path` in generator now uses correct path-component algorithm
  instead of slash-counting heuristic
- Removed lazy-parse boilerplate duplication via unified `ensure_parsed()` helper
- `Metadata::titles` is now populated with full `Vec<TitleEntry>` from all
  `<dc:title>` elements including EPUB 2 `opf:title-type` and EPUB 3 refinements

### Fixed

- E0499 double mutable borrow in three FFI location functions (CI vs local NLL difference)
- FFI `needless_borrow` (×11), `missing_const_for_thread_local` (×1),
  `cast_slice_from_raw_parts` (×1)
- Invalid JSDoc formatting in WASM bindings causing JS syntax errors
- Nested path and image extension resolution for WASM reader

### Infrastructure

- Added `rust-toolchain.toml` pinned to `1.95.0` (aligns local and CI environments)
- CI now uses `cargo clippy --all-targets --all-features -- -D warnings`
- Added benchmark suite (`benches/epub_benchmark.rs`) with Criterion:
  parse, generate, extract_text, semantic extraction, location index, search
- Added layered test architecture: roundtrip tests, property-based tests (proptest),
  and invariant tests (103 unit tests + 12 roundtrip + 8 property + 4 integration)

## [0.2.0] - 2025-??-??

Initial published release.
