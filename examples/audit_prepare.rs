//! Audit `prepare_chapter` on real EPUB files.
//!
//! ```text
//! cargo run --example audit_prepare -- /path/to/ebooks
//! ```

use epub_rs::parser::EpubArchive;
use epub_rs::processor::PrepareChapterOptions;
use std::env;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn collect_epubs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(root) else {
        return out;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("epub")) == Some(true)
        {
            out.push(p);
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

fn truncate(s: &str, max: usize) -> String {
    let t = s.replace('\n', " ");
    if t.chars().count() <= max {
        t
    } else {
        let mut out: String = t.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn audit_one(path: &Path) {
    let name = short_name(path);
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            println!("## {name}");
            println!("  OPEN FAIL: {e}");
            println!();
            return;
        }
    };

    let mut archive = match EpubArchive::new(Cursor::new(bytes)) {
        Ok(a) => a,
        Err(e) => {
            println!("## {name}");
            println!("  ZIP/OPEN FAIL: {e}");
            println!();
            return;
        }
    };

    let book = match archive.parse() {
        Ok(b) => b,
        Err(e) => {
            println!("## {name}");
            println!("  PARSE FAIL: {e}");
            println!();
            return;
        }
    };

    let linear: Vec<_> = book.spine.iter().filter(|s| s.linear).collect();
    let spine_n = book.spine.len();
    let linear_n = linear.len();
    let title = book
        .metadata
        .title
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "(no title)".into());
    let lang = book.metadata.language.clone().unwrap_or_default();

    println!("## {name}");
    println!("  title: {title}");
    if !lang.is_empty() {
        println!("  language: {lang}");
    }
    println!(
        "  size: {:.1} MB | spine: {spine_n} (linear {linear_n}) | manifest: {}",
        fs::metadata(path).map(|m| m.len() as f64 / 1_048_576.0).unwrap_or(0.0),
        book.manifest.len()
    );

    let opts_cfi = PrepareChapterOptions {
        inject_cfi: true,
        inline_resources: true,
        max_inline_bytes: 4 * 1024 * 1024,
    };
    let opts_no_cfi = PrepareChapterOptions {
        inject_cfi: false,
        inline_resources: true,
        max_inline_bytes: 4 * 1024 * 1024,
    };

    let mut engine_ok = 0usize;
    let mut engine_ok_no_cfi = 0usize;
    let mut engine_fail = 0usize;
    let mut empty = 0usize;
    let mut failures: Vec<(String, String, String)> = Vec::new();
    let t0 = Instant::now();

    for (i, item) in linear.iter().enumerate() {
        let id = &item.idref;
        let href = book
            .manifest
            .get(id)
            .map(|m| m.href.as_str())
            .unwrap_or("?");

        match archive.prepare_chapter(&book, id, &opts_cfi) {
            Ok(html) if !html.is_empty() => {
                engine_ok += 1;
                continue;
            }
            Ok(_) => {
                // empty with CFI — try without
            }
            Err(e_cfi) => {
                // retry without CFI (Latte policy)
                match archive.prepare_chapter(&book, id, &opts_no_cfi) {
                    Ok(html) if !html.is_empty() => {
                        engine_ok_no_cfi += 1;
                        if failures.len() < 12 {
                            failures.push((
                                format!("spine[{i}] {id}"),
                                href.to_string(),
                                format!("CFI fail → ok without CFI: {e_cfi}"),
                            ));
                        }
                        continue;
                    }
                    Ok(_) => {
                        empty += 1;
                        engine_fail += 1;
                        if failures.len() < 12 {
                            failures.push((
                                format!("spine[{i}] {id}"),
                                href.to_string(),
                                format!("empty HTML (CFI err: {e_cfi})"),
                            ));
                        }
                        continue;
                    }
                    Err(e2) => {
                        engine_fail += 1;
                        if failures.len() < 12 {
                            failures.push((
                                format!("spine[{i}] {id}"),
                                href.to_string(),
                                format!("CFI: {e_cfi} | no-CFI: {e2}"),
                            ));
                        }
                        continue;
                    }
                }
            }
        }

        // empty after CFI success path
        match archive.prepare_chapter(&book, id, &opts_no_cfi) {
            Ok(html) if !html.is_empty() => {
                engine_ok_no_cfi += 1;
                if failures.len() < 12 {
                    failures.push((
                        format!("spine[{i}] {id}"),
                        href.to_string(),
                        "empty with CFI → ok without CFI".into(),
                    ));
                }
            }
            Ok(_) => {
                empty += 1;
                engine_fail += 1;
                if failures.len() < 12 {
                    failures.push((
                        format!("spine[{i}] {id}"),
                        href.to_string(),
                        "empty HTML both with/without CFI".into(),
                    ));
                }
            }
            Err(e) => {
                engine_fail += 1;
                if failures.len() < 12 {
                    failures.push((
                        format!("spine[{i}] {id}"),
                        href.to_string(),
                        format!("empty+CFI then no-CFI err: {e}"),
                    ));
                }
            }
        }
    }

    let elapsed = t0.elapsed();
    let total = linear_n.max(1);
    let ok_any = engine_ok + engine_ok_no_cfi;
    let rate = (ok_any as f64 / total as f64) * 100.0;

    println!(
        "  prepare: engine_ok={engine_ok}  retry_no_cfi={engine_ok_no_cfi}  fail={engine_fail}  empty={empty}  ({rate:.1}% engine usable)"
    );
    println!("  elapsed: {:.2}s", elapsed.as_secs_f64());

    if !failures.is_empty() {
        println!("  sample issues (up to 12):");
        for (loc, href, reason) in &failures {
            println!(
                "    - {loc}  href={href}\n      {}",
                truncate(reason, 160)
            );
        }
    }
    println!();
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let roots: Vec<PathBuf> = if args.is_empty() {
        vec![
            PathBuf::from("ebooks"),
            PathBuf::from("../../ebooks"),
            PathBuf::from("../../../ebooks"),
        ]
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

    // de-dupe by file name
    all.sort_by_key(|p| short_name(p));
    all.dedup_by_key(|p| short_name(p));

    if all.is_empty() {
        eprintln!("No .epub files found. Pass a directory, e.g.:");
        eprintln!("  cargo run --example audit_prepare -- ../../ebooks");
        std::process::exit(1);
    }

    println!("# epub-rs prepare_chapter audit");
    println!("books: {}\n", all.len());

    let mut total_ok = 0usize;
    let mut total_retry = 0usize;
    let mut total_fail = 0usize;
    let mut total_ch = 0usize;

    for path in &all {
        // re-run counting via print only — lightweight second pass would need refactor;
        // for now per-book prints are enough.
        let _ = path;
        audit_one(path);
    }

    // Aggregate re-scan for summary table
    println!("# Summary");
    println!(
        "| Book | Linear | engine_ok | no_cfi_retry | fail | usable% |"
    );
    println!("|------|--------|-----------|--------------|------|---------|");

    for path in &all {
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let mut archive = match EpubArchive::new(Cursor::new(bytes)) {
            Ok(a) => a,
            Err(_) => continue,
        };
        let book = match archive.parse() {
            Ok(b) => b,
            Err(_) => continue,
        };
        let linear: Vec<_> = book.spine.iter().filter(|s| s.linear).collect();
        let opts_cfi = PrepareChapterOptions {
            inject_cfi: true,
            inline_resources: true,
            max_inline_bytes: 4 * 1024 * 1024,
        };
        let opts_no_cfi = PrepareChapterOptions {
            inject_cfi: false,
            inline_resources: true,
            max_inline_bytes: 4 * 1024 * 1024,
        };
        let mut ok = 0usize;
        let mut retry = 0usize;
        let mut fail = 0usize;
        for item in &linear {
            match archive.prepare_chapter(&book, &item.idref, &opts_cfi) {
                Ok(h) if !h.is_empty() => ok += 1,
                _ => match archive.prepare_chapter(&book, &item.idref, &opts_no_cfi) {
                    Ok(h) if !h.is_empty() => retry += 1,
                    _ => fail += 1,
                },
            }
        }
        let n = linear.len();
        total_ok += ok;
        total_retry += retry;
        total_fail += fail;
        total_ch += n;
        let usable = if n == 0 {
            0.0
        } else {
            ((ok + retry) as f64 / n as f64) * 100.0
        };
        let label = truncate(&short_name(path), 42);
        println!(
            "| {label} | {n} | {ok} | {retry} | {fail} | {usable:.1}% |"
        );
    }

    let usable_all = if total_ch == 0 {
        0.0
    } else {
        ((total_ok + total_retry) as f64 / total_ch as f64) * 100.0
    };
    println!();
    println!(
        "TOTAL chapters={total_ch}  engine_ok={total_ok}  no_cfi_retry={total_retry}  fail={total_fail}  usable={usable_all:.1}%"
    );
}
