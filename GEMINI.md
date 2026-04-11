# epub-rs

## Project Overview

`epub-rs` is a Rust library designed for parsing and generating EPUB (Electronic Publication) files.

The project architecture is split into two primary components:
- **`parser` (`src/parser.rs`)**: Handles reading EPUB files, unzipping contents, and extracting EPUB metadata (such as `container.xml`, `.opf` manifests) and internal HTML content.
- **`generator` (`src/generator.rs`)**: Manages the construction of EPUB archives, ensuring structural metadata and HTML content are appropriately zipped according to the official EPUB specifications.

### Key Technologies

- **Rust** (Edition 2024): The core programming language.
- **[zip](https://crates.io/crates/zip)** (`v8.5.1`): For manipulating the EPUB archive format, since EPUB files are fundamentally ZIP archives.
- **[quick-xml](https://crates.io/crates/quick-xml)** (`v0.39.2`): A high-performance XML parser/writer used for processing essential EPUB metadata like `META-INF/container.xml`, rootfiles, and navigation manifests.
- **[lol_html](https://crates.io/crates/lol_html)** (`v2.7.2`): Cloudflare's low-latency HTML rewriting/parsing engine, utilized for high-speed extraction or modification of XHTML/HTML chapter contents.

## Building and Running

As a standard Rust library managed by Cargo, the typical build commands apply:

- **Build the project:**
  ```bash
  cargo build
  ```
- **Run the test suite:**
  ```bash
  cargo test
  ```
- **Check for compilation errors (faster than building):**
  ```bash
  cargo check
  ```
- **Generate and view documentation:**
  ```bash
  cargo doc --open
  ```

## Development Conventions

- **Module Separation**: Maintain a strict separation of concerns. Parsing logic should reside in the `parser` module, while packaging and archive creation logic belongs in the `generator` module. Shared structures (like the `Epub` struct) reside in `src/lib.rs`.
- **Error Handling**: The initial setup uses generic `Result<T, Box<dyn std::error::Error>>`. As the project scales, it is recommended to implement specific error types using standard idiomatic Rust approaches (e.g., integrating the `thiserror` crate) for robust error matching.
- **Code Quality**: Adhere to standard Rust styling. Code should be formatted using `cargo fmt` and linted with `cargo clippy`.
- **EPUB Standards Compliance**: When working on the `generator`, ensure strict adherence to the EPUB structural requirements (e.g., the `mimetype` file must be the very first file added to the ZIP archive, and it must be stored *uncompressed*).
- **`third_party` Directory**: Components in the `third_party` directory serve as reference implementations for the current library's functionality. Do NOT commit this directory to the git repository, and do NOT add it to `.gitignore`.
- **`GEMINI.md` Management**: This file (`GEMINI.md`) provides local AI context. Do NOT commit it to the git repository, and do NOT add it to `.gitignore`.

---
*This file serves as contextual instruction for AI interactions within the `epub-rs` directory.*