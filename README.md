# epub-rs

**epub-rs** is an industrial-grade, highly performant EPUB 2/3 processing engine for Rust. 

It provides an end-to-end toolchain to **parse, process, deobfuscate, and generate** electronic books. Designed for heavy workloads and commercial reading apps, it avoids deep DOM tree memory overheads by utilizing blazing-fast stream processors ([`lol_html`](https://github.com/cloudflare/lol-html)).

## Features

### 🌐 WebAssembly (WASM) Support
* **Browser-Native EPUB Engine**: Compile the entire parsing and generation engine to `wasm32-unknown-unknown` to run directly in the browser or Node.js.
* **Zero-FS Architecture**: Parse binary `Uint8Array` EPUB buffers completely in memory without requiring a virtual file system.
* **JS-Interop FFI**: Full `wasm-bindgen` FFI bindings (`EpubParser`, `EpubGenerator`, `compare_cfi`, `decrypt_font`) with `serde` integration for passing complex metadata and multi-level TOC JSON seamlessly between JS and Rust.

### 📖 Robust Parsing
* **Multiple Renditions**: Support for fetching and parsing multiple `.opf` rootfiles from a single EPUB container.
* **Storage Agnostic**: `EpubProvider` trait allows extracting resources from traditional `.epub` ZIP files or exploded local directories without memory bloat.
* **Smart Cover API**: Heuristic 4-tier fallback extraction algorithm to securely find the book cover.
* **TOC & Navigation**: Reverse parses modern EPUB 3 `nav.xhtml` and legacy EPUB 2 `toc.ncx` into nested tree structures.
* **Font Deobfuscation**: Transparent stream-decryption of commercially obfuscated `.ttf`/`.otf` fonts (supports IDPF and Adobe algorithms via `META-INF/encryption.xml`).

### ⚙️ Content Processing & CFI
* **Stream-Based HTML Rewriting**: Inject custom CSS themes (e.g. Dark Mode) into `<head>` or rewrite asset links (`<img>`, `<a>`) with near-zero latency.
* **Canonical Fragment Identifier (CFI)**: Full specification support for Point and Range EPUB CFI (`epubcfi(/6/4!/4/2:5)`).
* **DOM CFI Injection**: Injects exact `data-cfi` paths into every DOM element to bridge frontend web-reader interactions (Highlighting & Bookmarks).
* **Full-text Search to CFI**: Search raw HTML using Regex and return the exact CFI ranges pointing to the match.
* **Synthetic Positions**: Generates virtual reading progress markers across the entire book for unified pagination.
* **Semantic Extractor**: Extracts TTS (Text-To-Speech) and A11Y friendly structural streams (`ContentElement`), preserving language and block boundaries.

### ✍️ Intelligent Generation (Builder)
* **Strict EPUB 2 / 3 Generation**: Conditional compilation isolating legacy `NCX` from modern `NAV`, generating compliant `content.opf`.
* **Streaming Large Files**: Stream massive assets (videos/images) directly into the EPUB ZIP pipe without loading them into memory (`add_resource_stream`).
* **Rich Metadata & Layouts**: Full Dublin Core property refinements (authors vs. translators), Pre-Paginated Fixed-Layout (FXL), and Page Spreads (Comics/Manga).
* **Automatic Property Inference**: Automatically detects `<script>`, `<svg>`, and `<math>` to inject EPUB 3 required properties.
* **Landmarks & Page-Lists**: Build comprehensive guide mappings for academic and textbook parity.

---

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
epub-rs = "0.1.0"
```

## Quick Start

### 1. Read an EPUB & Extract Text
```rust
use epub_rs::parser::EpubArchive;
use std::fs::File;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open("book.epub")?;
    let mut archive = EpubArchive::new(file)?;
    
    // Parse OPF and metadata
    let book = archive.parse()?;
    println!("Title: {:?}", book.metadata.title);
    
    // Read the first chapter from the spine
    let first_chapter_id = &book.spine[0].idref;
    let html_bytes = archive.get_resource_by_id(&book, first_chapter_id)?;
    
    // Extract plain text for search indexing
    let plain_text = epub_rs::processor::extract_text(&html_bytes)?;
    println!("Content: {}", plain_text);
    
    Ok(())
}
```

### 2. Generate a Compliant EPUB 3 Book
```rust
use epub_rs::generator::{EpubBuilder, TocEntry};
use epub_rs::model::{Creator, EpubVersion, Metadata};
use std::fs::File;

fn main() {
    let metadata = Metadata {
        title: Some("My Awesome Book".to_string()),
        creators: vec![Creator::new("Rustacean")],
        language: Some("en".to_string()),
        ..Default::default()
    };

    let builder = EpubBuilder::new()
        .version(EpubVersion::V30)
        .metadata(metadata)
        // Auto-inject built-in typography and dark mode CSS
        .theme(epub_rs::generator::Theme::Modern) 
        // Add nested table of contents
        .set_toc(vec![TocEntry::new("Chapter 1", "text/ch1.xhtml")])
        // Generate book and HTML
        .add_chapter("ch1", "text/ch1.xhtml", b"<h1>Hello</h1><p>World!</p>".to_vec());

    let mut file = File::create("output.epub").unwrap();
    builder.generate(&mut file).expect("Failed to generate EPUB");
}
```

### 3. Build a Web Reader (CFI Injection)
Pass HTML directly to the browser with exact book-location identifiers, removing the need for complex frontend calculation.

```rust
let chapter_html_with_cfi = archive.get_chapter_with_cfi(&book, "chapter_1_id")?;
// output: <p data-cfi="epubcfi(/6/4!/4/2)">...</p>
```

### 4. Semantic TTS Extraction
Ideal for Text-To-Speech, extracts language-tagged block structures instead of flat strings.

```rust
let elements = archive.get_semantic_content(&book, "chapter_1_id")?;
for el in elements {
    println!("Read {} (in {:?}): {}", el.tag_name, el.language, el.text);
    // e.g., Read p (in Some("fr")): Bonjour!
}
```

## Performance
Built on Cloudflare's `lol_html` and `zip-rs`, `epub-rs` processes DOMs in a single pass without allocating heavy AST trees.

* **~20 µs**: Open ZIP, parse OPF, setup Domain Models (10 chapters).
* **~140 µs**: Build, assemble, and compress a full EPUB to memory.
* **~30 µs**: Find 50 regex text matches and reverse-map them to exact CFI ranges.

*(Benchmarks executed on Apple Silicon M-series via `cargo bench`)*

## License
MIT License
s**: Build, assemble, and compress a full EPUB to memory.
* **~30 µs**: Find 50 regex text matches and reverse-map them to exact CFI ranges.

*(Benchmarks executed on Apple Silicon M-series via `cargo bench`)*

## License
MIT License
