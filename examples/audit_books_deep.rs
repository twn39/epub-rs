//! Deeper structural audit of real EPUBs (metadata, TOC, cover, start, sample chapter).
//!
//! ```text
//! cargo run --example audit_books_deep --release -- ../../ebooks
//! ```

use epub_rs::parser::EpubArchive;
use epub_rs::processor::PrepareChapterOptions;
use std::env;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

fn collect_epubs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(root) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("epub"))
                == Some(true)
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn short_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let roots: Vec<PathBuf> = if args.is_empty() {
        vec![PathBuf::from("../../ebooks")]
    } else {
        args.into_iter().map(PathBuf::from).collect()
    };

    let mut all = Vec::new();
    for root in &roots {
        if root.is_dir() {
            all.extend(collect_epubs(root));
        } else if root.is_file() {
            all.push(root.clone());
        }
    }
    all.sort_by_key(|p| short_name(p));
    all.dedup_by_key(|p| short_name(p));

    println!("# Deep EPUB structure audit\n");

    for path in &all {
        let name = short_name(path);
        let bytes = fs::read(path).expect("read");
        let mut archive = EpubArchive::new(Cursor::new(bytes)).expect("open");
        let book = archive.parse().expect("parse");
        let toc = archive.get_toc(&book).unwrap_or_default();

        let linear: Vec<_> = book.spine.iter().filter(|s| s.linear).collect();
        let non_linear = book.spine.len().saturating_sub(linear.len());

        // preferred start (always returns a value)
        let start = archive.preferred_reading_start(&book);

        // cover
        let cover = archive.get_cover_image(&book).ok();

        // sample first linear + mid + last prepare sizes
        let opts = PrepareChapterOptions {
            inject_cfi: true,
            inline_resources: true,
            max_inline_bytes: 4 * 1024 * 1024,
        };
        let mut sample_lens = Vec::new();
        let sample_idx: Vec<usize> = {
            let n = linear.len();
            if n == 0 {
                vec![]
            } else if n == 1 {
                vec![0]
            } else if n == 2 {
                vec![0, 1]
            } else {
                vec![0, n / 2, n - 1]
            }
        };
        for i in sample_idx {
            let id = &linear[i].idref;
            let href = book.manifest.get(id).map(|m| m.href.clone()).unwrap_or_default();
            match archive.prepare_chapter(&book, id, &opts) {
                Ok(html) => {
                    let has_data_uri = html.contains("data:");
                    let has_cfi = html.contains("data-cfi");
                    let img_rel = html.matches("src=\"").count();
                    sample_lens.push(format!(
                        "spine[{i}] id={id} href={href} len={} data_uri={has_data_uri} data_cfi={has_cfi} src_attrs≈{img_rel}",
                        html.len()
                    ));
                }
                Err(e) => sample_lens.push(format!("spine[{i}] FAIL {e}")),
            }
        }

        // media type breakdown
        let mut types: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for m in book.manifest.values() {
            *types.entry(m.media_type.clone()).or_default() += 1;
        }

        println!("## {name}");
        println!(
            "- title: {}",
            book.metadata.title.as_deref().unwrap_or("(none)")
        );
        println!(
            "- language: {}",
            book.metadata.language.as_deref().unwrap_or("(none)")
        );
        if let Some(a) = book.metadata.creators.first() {
            println!("- creator: {}", a.name);
        }
        println!(
            "- spine: {} linear={} non_linear={} | manifest={} | toc_top={}",
            book.spine.len(),
            linear.len(),
            non_linear,
            book.manifest.len(),
            toc.len()
        );
        println!(
            "- preferred_start: spine_index={} source={} href={}",
            start.spine_index,
            start.source,
            start.href.as_deref().unwrap_or("?")
        );
        match &cover {
            Some((bytes, media)) => {
                println!("- cover: {} bytes, media={media}", bytes.len())
            }
            None => println!("- cover: (none)"),
        }
        println!("- media_types: {types:?}");
        println!("- sample prepare:");
        for line in sample_lens {
            println!("  - {line}");
        }
        println!();
    }
}
