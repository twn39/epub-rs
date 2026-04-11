use criterion::{Criterion, criterion_group, criterion_main};
use epub_rs::generator::EpubBuilder;
use epub_rs::model::{EpubVersion, Metadata};
use epub_rs::parser::EpubArchive;
use epub_rs::processor::{
    extract_positions, extract_semantic_content, extract_text, search_chapter,
};
use regex::Regex;
use std::hint::black_box;
use std::io::Cursor;

fn generate_dummy_epub(chapter_count: usize) -> Vec<u8> {
    let mut builder = EpubBuilder::new()
        .version(EpubVersion::V30)
        .metadata(Metadata::default());

    let chapter_html = r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Test Chapter</title></head>
<body>
    <h1>A Test Chapter</h1>
    <p>This is a paragraph of text used to benchmark the EPUB parser and generator.</p>
    <p>It contains multiple lines to increase the payload size.</p>
    <div id="content">
        <p>Some more text here.</p>
    </div>
</body>
</html>"#
        .as_bytes()
        .to_vec();

    for i in 0..chapter_count {
        builder = builder.add_chapter(
            format!("ch{}", i),
            format!("text/ch{}.xhtml", i),
            chapter_html.clone(),
        );
    }

    let mut buffer = Cursor::new(Vec::new());
    builder.generate(&mut buffer).unwrap();
    buffer.into_inner()
}

fn bench_parse_epub(c: &mut Criterion) {
    let epub_bytes = generate_dummy_epub(10); // 10 chapters

    c.bench_function("parse_epub (10 chapters)", |b| {
        b.iter(|| {
            let cursor = Cursor::new(black_box(&epub_bytes));
            let mut archive = EpubArchive::new(cursor).unwrap();
            let _book = archive.parse().unwrap();
        })
    });
}

fn bench_generate_epub(c: &mut Criterion) {
    let chapter_html = r#"<html><body><p>Test</p></body></html>"#.as_bytes().to_vec();

    c.bench_function("generate_epub (10 chapters)", |b| {
        b.iter(|| {
            let mut builder = EpubBuilder::new()
                .version(EpubVersion::V30)
                .metadata(Metadata::default());

            for i in 0..10 {
                builder = builder.add_chapter(
                    format!("ch{}", i),
                    format!("text/ch{}.xhtml", i),
                    chapter_html.clone(),
                );
            }

            let mut buffer = Cursor::new(Vec::new());
            builder.generate(&mut buffer).unwrap();
            black_box(buffer.into_inner());
        })
    });
}

fn bench_processor_extract_text(c: &mut Criterion) {
    let html = r#"<html><body>"#.to_string()
        + &"<p>Benchmarking extraction performance.</p>".repeat(50)
        + "</body></html>";
    let bytes = html.as_bytes();

    c.bench_function("extract_text (50 paragraphs)", |b| {
        b.iter(|| {
            let _text = extract_text(black_box(bytes)).unwrap();
        })
    });
}

fn bench_processor_extract_semantic(c: &mut Criterion) {
    let html = r#"<html><body>"#.to_string()
        + &"<h2>Heading</h2><p>Benchmarking semantic extraction performance.</p>".repeat(50)
        + "</body></html>";

    c.bench_function("extract_semantic_content (100 blocks)", |b| {
        b.iter(|| {
            let _elements = extract_semantic_content(black_box(&html), black_box("/6/4!"));
        })
    });
}

fn bench_processor_extract_positions(c: &mut Criterion) {
    let html = r#"<html><body>"#.to_string()
        + &"<p>Benchmarking position extraction performance.</p>".repeat(50)
        + "</body></html>";
    let ctx = epub_rs::processor::PositionContext {
        base_cfi: "/6/4!",
        chars_per_position: 1024,
        spine_index: 0,
        href: "ch1.xhtml",
    };

    c.bench_function("extract_positions (50 paragraphs)", |b| {
        b.iter(|| {
            let mut char_counter = 0;
            let mut global_pos = 0;
            let mut positions = Vec::new();

            extract_positions(
                black_box(&html),
                black_box(&ctx),
                &mut char_counter,
                &mut positions,
                &mut global_pos,
            );

            black_box(positions);
        })
    });
}

fn bench_processor_search_chapter(c: &mut Criterion) {
    let html = r#"<html><body>"#.to_string()
        + &"<p>Finding the needle in the haystack.</p>".repeat(50)
        + "</body></html>";
    let pattern = Regex::new(r"needle").unwrap();

    c.bench_function("search_chapter (50 needles)", |b| {
        b.iter(|| {
            let _results =
                search_chapter(black_box(&html), black_box("/6/4!"), black_box(&pattern)).unwrap();
        })
    });
}

criterion_group!(
    benches,
    bench_parse_epub,
    bench_generate_epub,
    bench_processor_extract_text,
    bench_processor_extract_semantic,
    bench_processor_extract_positions,
    bench_processor_search_chapter
);
criterion_main!(benches);
