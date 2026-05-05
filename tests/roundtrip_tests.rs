//! Roundtrip integration tests: Generator → Parser semantic fidelity.
//!
//! Each test generates an in-memory EPUB, parses it back, and asserts that
//! the semantic meaning is preserved end-to-end. This layer catches bugs that
//! neither pure generator tests nor pure parser tests can detect.

use epub_rs::generator::EpubBuilder;
use epub_rs::model::{Creator, EpubVersion, LayoutType, Metadata, TocEntry};
use epub_rs::parser::EpubArchive;
use std::io::Cursor;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn generate(builder: EpubBuilder) -> Cursor<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    builder.generate(&mut buf).expect("generation failed");
    buf.set_position(0);
    buf
}

/// Minimal valid Metadata (title + language + identifier all required).
fn minimal_meta(title: &str, lang: &str) -> Metadata {
    Metadata {
        title: Some(title.to_string()),
        language: Some(lang.to_string()),
        identifier: Some(format!("urn:uuid:test-{}", title.replace(' ', "-").to_lowercase())),
        ..Default::default()
    }
}

// ── Basic metadata roundtrip ──────────────────────────────────────────────────

#[test]
fn roundtrip_title_language_identifier() {
    let metadata = Metadata {
        title: Some("Roundtrip Book".to_string()),
        language: Some("zh-TW".to_string()),
        identifier: Some("urn:uuid:rt-0001".to_string()),
        ..Default::default()
    };
    let buf = generate(
        EpubBuilder::new()
            .version(EpubVersion::V30)
            .metadata(metadata)
            .add_chapter("c1", "text/c1.xhtml", b"<html><body><p>Hello</p></body></html>".to_vec()),
    );

    let mut archive = EpubArchive::new(buf).unwrap();
    let book = archive.parse().unwrap();

    assert_eq!(book.metadata.title.as_deref(), Some("Roundtrip Book"));
    assert_eq!(book.metadata.language.as_deref(), Some("zh-TW"));
    assert_eq!(book.metadata.identifier.as_deref(), Some("urn:uuid:rt-0001"));
}

#[test]
fn roundtrip_multiple_creators_with_roles() {
    let metadata = Metadata {
        title: Some("Multi-Creator Book".to_string()),
        language: Some("en".to_string()),
        identifier: Some("urn:uuid:test-multi-creator".to_string()),
        creators: vec![
            Creator { name: "Alice Author".to_string(), role: Some("aut".to_string()), file_as: Some("Author, Alice".to_string()) },
            Creator { name: "Bob Translator".to_string(), role: Some("trl".to_string()), file_as: None },
        ],
        ..Default::default()
    };
    let buf = generate(
        EpubBuilder::new()
            .version(EpubVersion::V30)
            .metadata(metadata)
            .add_chapter("c1", "text/c1.xhtml", b"<html><body><p>x</p></body></html>".to_vec()),
    );

    let mut archive = EpubArchive::new(buf).unwrap();
    let book = archive.parse().unwrap();

    assert_eq!(book.metadata.creators.len(), 2);
    assert_eq!(book.metadata.creators[0].name, "Alice Author");
    assert_eq!(book.metadata.creators[0].role.as_deref(), Some("aut"));
    assert_eq!(book.metadata.creators[0].file_as.as_deref(), Some("Author, Alice"));
    assert_eq!(book.metadata.creators[1].name, "Bob Translator");
    assert_eq!(book.metadata.creators[1].role.as_deref(), Some("trl"));
}

// ── Spine / manifest roundtrip ────────────────────────────────────────────────

#[test]
fn roundtrip_spine_order_is_preserved() {
    let chapters = vec![
        ("ch1", "text/ch1.xhtml"),
        ("ch2", "text/ch2.xhtml"),
        ("ch3", "text/ch3.xhtml"),
    ];
    let mut builder = EpubBuilder::new()
        .version(EpubVersion::V30)
        .metadata(minimal_meta("Spine Test", "en"));
    for (id, href) in &chapters {
        builder = builder.add_chapter(*id, *href, b"<html><body><p>x</p></body></html>".to_vec());
    }
    let buf = generate(builder);

    let mut archive = EpubArchive::new(buf).unwrap();
    let book = archive.parse().unwrap();

    assert_eq!(book.spine.len(), 3);
    assert_eq!(book.spine[0].idref, "ch1");
    assert_eq!(book.spine[1].idref, "ch2");
    assert_eq!(book.spine[2].idref, "ch3");

    // All spine items should be linear by default
    for item in &book.spine {
        assert!(item.linear, "default spine items should be linear");
    }
}

#[test]
fn roundtrip_resource_content_byte_for_byte() {
    let css_bytes = b"body { font-family: serif; color: #333; }";
    let html_bytes = b"<html><body><p>Content</p></body></html>";

    let buf = generate(
        EpubBuilder::new()
            .version(EpubVersion::V30)
            .metadata(minimal_meta("Resource Test", "en"))
            .add_resource("style", "css/style.css", "text/css", css_bytes.to_vec())
            .add_chapter("ch1", "text/ch1.xhtml", html_bytes.to_vec()),
    );

    let mut archive = EpubArchive::new(buf).unwrap();
    let book = archive.parse().unwrap();

    // CSS bytes must survive the ZIP round-trip exactly
    let extracted_css = archive.get_resource_by_id(&book, "style").unwrap();
    assert_eq!(extracted_css, css_bytes.as_slice());

    // HTML bytes must also be byte-for-byte identical
    let extracted_html = archive.get_resource_by_id(&book, "ch1").unwrap();
    assert_eq!(extracted_html, html_bytes.as_slice());
}

// ── TOC / navigation roundtrip ────────────────────────────────────────────────

#[test]
fn roundtrip_toc_titles_and_hrefs() {
    let toc = vec![
        TocEntry::new("第一章", "text/ch1.xhtml")
            .add_child(TocEntry::new("第一节", "text/ch1.xhtml#s1")),
        TocEntry::new("第二章", "text/ch2.xhtml"),
    ];

    let buf = generate(
        EpubBuilder::new()
            .version(EpubVersion::V30)
            .metadata(Metadata { title: Some("TOC Test".to_string()), language: Some("zh".to_string()), identifier: Some("urn:uuid:test-toc".to_string()), ..Default::default() })
            .add_chapter("ch1", "text/ch1.xhtml", "<html><body><p>\u{4e00}</p></body></html>".as_bytes().to_vec())
            .add_chapter("ch2", "text/ch2.xhtml", "<html><body><p>\u{4e8c}</p></body></html>".as_bytes().to_vec())
            .set_toc(toc),
    );

    let mut archive = EpubArchive::new(buf).unwrap();
    let book = archive.parse().unwrap();
    let nav = archive.get_navigation(&book).unwrap();

    assert_eq!(nav.toc.len(), 2, "top-level TOC should have 2 entries");
    assert_eq!(nav.toc[0].title, "第一章");
    assert_eq!(nav.toc[0].href, "text/ch1.xhtml");
    assert_eq!(nav.toc[0].children.len(), 1, "first chapter should have 1 child");
    assert_eq!(nav.toc[0].children[0].title, "第一节");
    assert_eq!(nav.toc[1].title, "第二章");
}

// ── EPUB version roundtrip ────────────────────────────────────────────────────

#[test]
fn roundtrip_epub2_basic_metadata() {
    let buf = generate(
        EpubBuilder::new()
            .version(EpubVersion::V20)
            .metadata(Metadata {
                title: Some("EPUB2 Classic".to_string()),
                language: Some("ja".to_string()),
                identifier: Some("urn:isbn:978-0-000-00000-0".to_string()),
                ..Default::default()
            })
            .add_chapter("ch1", "text/ch1.xhtml", "<html><body><p>\u{65e5}\u{672c}\u{8a9e}</p></body></html>".as_bytes().to_vec()),
    );

    let mut archive = EpubArchive::new(buf).unwrap();
    let book = archive.parse().unwrap();

    assert_eq!(book.metadata.title.as_deref(), Some("EPUB2 Classic"));
    assert_eq!(book.metadata.language.as_deref(), Some("ja"));
    assert_eq!(book.metadata.identifier.as_deref(), Some("urn:isbn:978-0-000-00000-0"));
}

// ── Fixed-layout roundtrip ────────────────────────────────────────────────────

#[test]
fn roundtrip_fixed_layout_global_setting() {
    let buf = generate(
        EpubBuilder::new()
            .version(EpubVersion::V30)
            .metadata(Metadata {
                title: Some("Comic".to_string()),
                language: Some("en".to_string()),
                identifier: Some("urn:uuid:test-comic".to_string()),
                layout: LayoutType::PrePaginated,
                ..Default::default()
            })
            .add_chapter("p1", "text/p1.xhtml", b"<html><body><p>Page 1</p></body></html>".to_vec())
            .add_chapter("p2", "text/p2.xhtml", b"<html><body><p>Page 2</p></body></html>".to_vec()),
    );

    let mut archive = EpubArchive::new(buf).unwrap();
    let book = archive.parse().unwrap();

    assert_eq!(book.metadata.layout, LayoutType::PrePaginated,
        "fixed-layout global setting must survive roundtrip");
    assert_eq!(book.spine.len(), 2);
}

// ── Positions roundtrip (end-to-end pipeline) ─────────────────────────────────

#[test]
fn roundtrip_positions_pipeline_invariants() {
    // Build a 3-chapter reflowable EPUB in memory, then compute positions.
    // This exercises the full pipeline: Generator → ZIP → Parser → Strategy → Positions.
    let html = b"<html><body>".iter()
        .chain(b"<p>Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.</p>".iter().cycle().take(2000))
        .chain(b"</body></html>".iter())
        .cloned()
        .collect::<Vec<u8>>();

    let mut builder = EpubBuilder::new()
        .version(EpubVersion::V30)
        .metadata(Metadata { title: Some("Positions Test".to_string()), language: Some("en".to_string()), identifier: Some("urn:uuid:test-positions".to_string()), ..Default::default() });
    for i in 0..3usize {
        builder = builder.add_chapter(
            format!("ch{i}"),
            format!("text/ch{i}.xhtml"),
            html.clone(),
        );
    }

    let buf = generate(builder);
    let mut archive = EpubArchive::new(buf).unwrap();
    let book = archive.parse().unwrap();

    let strategy = epub_rs::parser::positions::ArchiveEntryLength { page_length: 1024 };
    let positions = archive.positions_by_reading_order(&book, &strategy).unwrap();

    // 3 chapters → 3 groups
    assert_eq!(positions.len(), 3, "should have one group per linear chapter");

    // All groups must be non-empty
    for (i, chapter) in positions.iter().enumerate() {
        assert!(!chapter.is_empty(), "chapter {i} must have at least 1 position");
    }

    // Global monotonicity invariant
    let flat: Vec<_> = positions.iter().flatten().collect();
    for (i, pos) in flat.iter().enumerate() {
        assert_eq!(pos.global_position, i + 1,
            "global_position must be 1-based monotonic");
    }

    // total_progression of first position must be 0.0
    assert_eq!(flat[0].total_progression, 0.0);

    // All progressions in range
    for pos in &flat {
        assert!((0.0f32..=1.0).contains(&pos.total_progression));
        assert!((0.0f32..=1.0).contains(&pos.chapter_progression));
    }
}
