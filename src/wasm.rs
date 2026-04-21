use crate::generator::{EpubBuilder, Theme};
use crate::model::{Creator, EpubBook};
use crate::parser::EpubArchive;
use crate::processor;
use crate::provider::{EpubProvider, ZipProvider};
use std::io::{Cursor, Read};
use wasm_bindgen::prelude::*;

// -----------------------------------------------------------------------------
// EpubParser Wrapper
// -----------------------------------------------------------------------------

/// A WebAssembly wrapper for parsing EPUB archives directly from memory.
#[wasm_bindgen]
pub struct EpubParser {
    // Maintain the Zip archive connection to avoid re-scanning the central directory.
    archive: EpubArchive<ZipProvider<Cursor<Vec<u8>>>>,
    // Cache the parsed EPUB metadata model to avoid re-parsing the XML.
    book: Option<EpubBook>,
}

#[wasm_bindgen]
impl EpubParser {
    /// Create a new `EpubParser` from a Uint8Array containing the EPUB ZIP data.
    #[wasm_bindgen(constructor)]
    pub fn new(data: &[u8]) -> Result<EpubParser, JsValue> {
        let cursor = Cursor::new(data.to_vec());
        let archive = EpubArchive::new(cursor).map_err(|e| e.to_string())?;

        Ok(Self {
            archive,
            book: None,
        })
    }

    /// Parse the entire EPUB metadata, manifest, and spine into a JSON object.
    /// This returns a JavaScript object representing the `EpubBook` model.
    #[wasm_bindgen]
    pub fn parse(&mut self) -> Result<JsValue, JsValue> {
        if self.book.is_none() {
            let book = self.archive.parse().map_err(|e| e.to_string())?;
            self.book = Some(book);
        }

        serde_wasm_bindgen::to_value(self.book.as_ref().unwrap()).map_err(|e| e.to_string().into())
    }

    /// Retrieve the raw byte contents of a specific file inside the EPUB.
    /// E.g., `OEBPS/images/cover.jpg`. Returns a Uint8Array.
    #[wasm_bindgen]
    pub fn get_file_bytes(&mut self, path: &str) -> Result<Vec<u8>, JsValue> {
        let mut file = self
            .archive
            .provider
            .read_file(path)
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;

        Ok(bytes)
    }

    /// Retrieve the contents of a specific file inside the EPUB as a UTF-8 string.
    /// E.g., `OEBPS/Text/chapter1.xhtml`.
    #[wasm_bindgen]
    pub fn get_file_string(&mut self, path: &str) -> Result<String, JsValue> {
        let bytes = self.get_file_bytes(path)?;
        String::from_utf8(bytes).map_err(|e| format!("Failed to parse UTF-8: {}", e).into())
    }

    /// Retrieve the Table of Contents (TOC) of the EPUB.
    #[wasm_bindgen]
    pub fn get_toc(&mut self) -> Result<JsValue, JsValue> {
        if self.book.is_none() {
            let book = self.archive.parse().map_err(|e| e.to_string())?;
            self.book = Some(book);
        }

        let toc = self
            .archive
            .get_toc(self.book.as_ref().unwrap())
            .map_err(|e| e.to_string())?;
        serde_wasm_bindgen::to_value(&toc).map_err(|e| e.to_string().into())
    }

    /// Extract an HTML chapter and rewrite all internal assets (images, css, links) using a JS callback.
    /// The resolver function receives the absolute internal EPUB path (e.g. `OEBPS/Images/cover.jpg`)
    /// and should return a new URL (like a `blob://` URI or a base64 string) to replace it.
    /// Return `null` or `undefined` from JS to leave the link unchanged.
    #[wasm_bindgen]
    pub fn get_chapter_with_rewritten_assets(
        &mut self,
        html_path: &str,
        resolver: js_sys::Function,
    ) -> Result<String, JsValue> {
        let html_string = self.get_file_string(html_path)?;

        let new_html = processor::rewrite_resources(&html_string, html_path, move |abs_path| {
            let js_abs_path = JsValue::from_str(abs_path);
            match resolver.call1(&JsValue::NULL, &js_abs_path) {
                Ok(val) => val.as_string(),
                Err(_) => None,
            }
        })
        .map_err(|e| e.to_string())?;

        Ok(new_html)
    }

    /// Calculate and generate an array of virtual page locations (CFI snapshots)
    /// for the entire EPUB. Returns a JSON array of `Position` objects.
    /// `chars_per_location` defines how many characters constitute a "page" (default recommended: 1000).
    #[wasm_bindgen]
    pub fn generate_locations(&mut self, chars_per_location: usize) -> Result<JsValue, JsValue> {
        if self.book.is_none() {
            let book = self.archive.parse().map_err(|e| e.to_string())?;
            self.book = Some(book);
        }

        let locations = self
            .archive
            .generate_locations(self.book.as_ref().unwrap(), chars_per_location)
            .map_err(|e| e.to_string())?;

        serde_wasm_bindgen::to_value(&locations).map_err(|e| e.to_string().into())
    }
}

// -----------------------------------------------------------------------------
// EpubGenerator Wrapper
// -----------------------------------------------------------------------------

/// A WebAssembly wrapper for constructing new EPUB 3 archives purely in memory.
#[wasm_bindgen]
pub struct EpubGenerator {
    builder: EpubBuilder,
}

#[wasm_bindgen]
impl EpubGenerator {
    /// Create a new, empty EPUB generator.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            builder: EpubBuilder::new(),
        }
    }

    /// Set the title of the EPUB.
    #[wasm_bindgen]
    pub fn set_title(&mut self, title: &str) {
        let mut builder = std::mem::replace(&mut self.builder, EpubBuilder::new());
        builder.metadata.title = Some(title.to_string());
        self.builder = builder;
    }

    /// Set the language of the EPUB (e.g., `en`, `zh-CN`).
    #[wasm_bindgen]
    pub fn set_language(&mut self, lang: &str) {
        let mut builder = std::mem::replace(&mut self.builder, EpubBuilder::new());
        builder.metadata.language = Some(lang.to_string());
        self.builder = builder;
    }

    /// Add an author/creator to the EPUB metadata.
    #[wasm_bindgen]
    pub fn add_author(&mut self, name: &str, role: Option<String>) {
        let mut builder = std::mem::replace(&mut self.builder, EpubBuilder::new());
        let mut creator = Creator::new(name);
        if let Some(r) = role {
            creator.role = Some(r);
        }
        builder.metadata.creators.push(creator);
        self.builder = builder;
    }

    /// Set the EPUB's unique identifier (e.g., ISBN or UUID).
    #[wasm_bindgen]
    pub fn set_identifier(&mut self, id: &str) {
        let mut builder = std::mem::replace(&mut self.builder, EpubBuilder::new());
        builder.metadata.identifier = Some(id.to_string());
        self.builder = builder;
    }

    /// Set a default theme (e.g., `None`, `Modern`)
    #[wasm_bindgen]
    pub fn set_theme(&mut self, modern: bool) {
        let mut builder = std::mem::replace(&mut self.builder, EpubBuilder::new());
        builder = builder.theme(if modern { Theme::Modern } else { Theme::None });
        self.builder = builder;
    }

    /// Check if the current EPUB setup is compliant and contains no broken links.
    /// Will throw a JS error with a formatted array of strings if broken.
    #[wasm_bindgen]
    pub fn validate(&self) -> Result<(), JsValue> {
        self.builder.validate().map_err(|e| e.to_string().into())
    }

    /// Add a CSS stylesheet to the EPUB.
    #[wasm_bindgen]
    pub fn add_stylesheet(&mut self, id: &str, href: &str, css_content: &str) {
        let mut builder = std::mem::replace(&mut self.builder, EpubBuilder::new());
        builder = builder.add_resource(id, href, "text/css", css_content.as_bytes().to_vec());
        self.builder = builder;
    }

    /// Add an image to the EPUB (e.g., `image/jpeg`, `image/png`).
    #[wasm_bindgen]
    pub fn add_image(&mut self, id: &str, href: &str, media_type: &str, data: &[u8]) {
        let mut builder = std::mem::replace(&mut self.builder, EpubBuilder::new());
        builder = builder.add_resource(id, href, media_type, data.to_vec());
        self.builder = builder;
    }

    /// Set the cover image of the EPUB. The image must have been added via `add_image` first,
    /// or you can provide the raw bytes directly here to do both.
    #[wasm_bindgen]
    pub fn set_cover_image(&mut self, href: &str, media_type: &str, data: &[u8]) {
        let mut builder = std::mem::replace(&mut self.builder, EpubBuilder::new());
        builder = builder.set_cover(href, media_type, data.to_vec());
        self.builder = builder;
    }

    /// Add a chapter (HTML/XHTML) to the EPUB manifest AND append it to the spine (reading order).
    #[wasm_bindgen]
    pub fn add_chapter(&mut self, id: &str, href: &str, html_content: &str) {
        let mut builder = std::mem::replace(&mut self.builder, EpubBuilder::new());
        builder = builder.add_chapter(id, href, html_content.as_bytes().to_vec());
        self.builder = builder;
    }

    /// Add a chapter to the spine AND the Table of Contents (Nav Map).
    #[wasm_bindgen]
    pub fn add_chapter_with_nav(&mut self, id: &str, href: &str, title: &str, html_content: &str) {
        let mut builder = std::mem::replace(&mut self.builder, EpubBuilder::new());
        builder = builder.add_chapter_with_nav(id, href, title, html_content.as_bytes().to_vec());
        self.builder = builder;
    }

    /// Set the EPUB's complete multi-level Table of Contents via JSON.
    #[wasm_bindgen]
    pub fn set_toc(&mut self, toc_js: JsValue) -> Result<(), JsValue> {
        let toc: Vec<crate::model::TocEntry> = serde_wasm_bindgen::from_value(toc_js)?;
        let mut builder = std::mem::replace(&mut self.builder, EpubBuilder::new());
        builder = builder.set_toc(toc);
        self.builder = builder;
        Ok(())
    }

    /// Set the complete metadata via JSON.
    #[wasm_bindgen]
    pub fn set_metadata(&mut self, metadata_js: JsValue) -> Result<(), JsValue> {
        let metadata: crate::model::Metadata = serde_wasm_bindgen::from_value(metadata_js)?;
        let mut builder = std::mem::replace(&mut self.builder, EpubBuilder::new());
        builder = builder.metadata(metadata);
        self.builder = builder;
        Ok(())
    }

    /// Add a landmark (structural reference like `cover`, `toc`, `bodymatter`).
    #[wasm_bindgen]
    pub fn add_landmark(&mut self, epub_type: &str, href: &str, title: &str) {
        let mut builder = std::mem::replace(&mut self.builder, EpubBuilder::new());
        builder = builder.add_landmark(epub_type, href, title);
        self.builder = builder;
    }

    /// Add a physical page mapping entry (for academic/textbook parity).
    #[wasm_bindgen]
    pub fn add_page(&mut self, name: &str, href: &str) {
        let mut builder = std::mem::replace(&mut self.builder, EpubBuilder::new());
        builder = builder.add_page(name, href);
        self.builder = builder;
    }

    /// Build the EPUB and return it as a Uint8Array.
    #[wasm_bindgen]
    pub fn generate(&mut self) -> Result<Vec<u8>, JsValue> {
        let builder = std::mem::replace(&mut self.builder, EpubBuilder::new());
        let mut output = Cursor::new(Vec::new());
        builder.generate(&mut output).map_err(|e| e.to_string())?;
        Ok(output.into_inner())
    }
}

// -----------------------------------------------------------------------------
// Processor Utilities Wrapper
// -----------------------------------------------------------------------------

/// Extract structural and semantic content from HTML/XHTML for TTS or Accessibility.
/// Returns a JSON array of `ContentElement` objects.
#[wasm_bindgen]
pub fn extract_semantic_content(html: &str, base_cfi: &str) -> Result<JsValue, JsValue> {
    let elements = processor::extract_semantic_content(html, base_cfi);
    serde_wasm_bindgen::to_value(&elements).map_err(|e| e.to_string().into())
}

/// Injects custom JavaScript and CSS into an HTML document.
#[wasm_bindgen]
pub fn inject_script_and_style(
    html: &str,
    script: Option<String>,
    style: Option<String>,
) -> Result<String, JsValue> {
    let mut reader = Cursor::new(html);
    let mut writer = Cursor::new(Vec::new());

    let content_to_inject = format!(
        "{}\n{}",
        style
            .map(|s| format!("<style>{}</style>", s))
            .unwrap_or_default(),
        script
            .map(|s| format!("<script>{}</script>", s))
            .unwrap_or_default()
    );

    processor::inject_head_content(&mut reader, &mut writer, &content_to_inject)
        .map_err(|e| e.to_string())?;

    String::from_utf8(writer.into_inner()).map_err(|e| e.to_string().into())
}

/// Injects `epubcfi(...)` markers into every viable DOM node.
#[wasm_bindgen]
pub fn inject_cfi_markers(html: &str, base_cfi: &str) -> Result<String, JsValue> {
    processor::inject_cfi_dom(html, base_cfi).map_err(|e| e.to_string().into())
}

/// Search for a text query within a chapter's HTML content.
/// Returns a JSON array of `SearchResult` objects containing the matching text snippet and its exact CFI range.
#[wasm_bindgen]
pub fn search_text_in_chapter(html: &str, base_cfi: &str, query: &str) -> Result<JsValue, JsValue> {
    let regex = regex::Regex::new(&regex::escape(query)).map_err(|e| e.to_string())?;
    let results = processor::search_chapter(html, base_cfi, &regex).map_err(|e| e.to_string())?;
    serde_wasm_bindgen::to_value(&results).map_err(|e| e.to_string().into())
}

// -----------------------------------------------------------------------------
// CFI and Crypto Utilities Wrapper
// -----------------------------------------------------------------------------

/// Compare two CFI strings numerically (step by step) as per the EPUB CFI spec.
/// Returns -1 if cfi_a < cfi_b, 0 if equal, +1 if cfi_a > cfi_b.
/// Both CFIs must be Point CFIs; comparing Range CFIs returns an error.
#[wasm_bindgen]
pub fn compare_cfi(cfi_a: &str, cfi_b: &str) -> Result<i32, JsValue> {
    let a: crate::EpubCfi =
        std::str::FromStr::from_str(cfi_a).map_err(|e: crate::error::EpubError| e.to_string())?;
    let b: crate::EpubCfi =
        std::str::FromStr::from_str(cfi_b).map_err(|e: crate::error::EpubError| e.to_string())?;

    match a.partial_cmp(&b) {
        Some(std::cmp::Ordering::Less) => Ok(-1),
        Some(std::cmp::Ordering::Equal) => Ok(0),
        Some(std::cmp::Ordering::Greater) => Ok(1),
        None => Err("compare_cfi: cannot compare Range CFIs — provide two Point CFIs".into()),
    }
}

/// Combine two Point CFIs into a spec-compliant CFI range string.
/// The output format is `epubcfi(shared_path,start_local,end_local)` where the
/// shared path is the longest common ancestor of both inputs.
#[wasm_bindgen]
pub fn generate_cfi_range(start_cfi: &str, end_cfi: &str) -> Result<String, JsValue> {
    let start: crate::EpubCfi = std::str::FromStr::from_str(start_cfi)
        .map_err(|e: crate::error::EpubError| e.to_string())?;
    let end: crate::EpubCfi =
        std::str::FromStr::from_str(end_cfi).map_err(|e: crate::error::EpubError| e.to_string())?;

    crate::cfi::EpubCfi::generate_range(&start, &end).map_err(|e| e.to_string().into())
}

/// In browser, decrypt obfuscated font files (.ttf, .woff)
/// is_idpf: true for IDPF algorithm, false for Adobe algorithm
#[wasm_bindgen]
pub fn decrypt_font(
    encrypted_data: &[u8],
    epub_identifier: &str,
    is_idpf: bool,
) -> Result<Vec<u8>, JsValue> {
    let algo = if is_idpf {
        crate::crypto::ObfuscationAlgorithm::Idpf
    } else {
        crate::crypto::ObfuscationAlgorithm::Adobe
    };

    let cursor = std::io::Cursor::new(encrypted_data);
    let mut reader =
        crate::crypto::DeobfuscatingReader::new(Box::new(cursor), epub_identifier, algo);

    let mut decrypted = Vec::with_capacity(encrypted_data.len());
    reader
        .read_to_end(&mut decrypted)
        .map_err(|e| e.to_string())?;

    Ok(decrypted)
}

// -----------------------------------------------------------------------------
// WebAssembly Unit Tests
// -----------------------------------------------------------------------------
#[cfg(test)]
#[cfg(target_arch = "wasm32")]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    // Run tests in a Node.js environment since we don't rely on the DOM
    wasm_bindgen_test_configure!(run_in_node_experimental);

    #[wasm_bindgen_test]
    fn test_epub_generator_and_parser() {
        let mut generator = EpubGenerator::new();

        // Setup metadata
        generator.set_title("WASM Test Book");
        generator.set_language("en");
        generator.add_author("AI Engineer", Some("aut".to_string()));
        generator.set_identifier("urn:uuid:wasm-test-1234");

        // Add content
        generator.add_chapter(
            "chapter1",
            "ch1.xhtml",
            "<html><body><h1>Chapter 1</h1><p>Content</p></body></html>",
        );

        // Generate the EPUB byte array
        let bytes = generator.generate().expect("Failed to generate EPUB bytes");
        assert!(!bytes.is_empty(), "Generated EPUB should not be empty");

        // Now, pass the bytes into the parser
        let parser = EpubParser::new(&bytes);

        // Parse metadata into a JS object
        let book_js_val = parser.parse().expect("Failed to parse the generated EPUB");
        assert!(!book_js_val.is_null());
        assert!(!book_js_val.is_undefined());

        // Try extracting the chapter content
        let html_content = parser
            .get_file_string("OEBPS/ch1.xhtml")
            .expect("Failed to extract chapter string");
        assert!(
            html_content.contains("<h1>Chapter 1</h1>"),
            "Extracted HTML should match the input"
        );
    }

    #[wasm_bindgen_test]
    fn test_wasm_advanced_metadata_and_toc() {
        let mut generator = EpubGenerator::new();

        // 1. Test set_metadata with JSON
        let metadata_json = r#"{
            "title": "Advanced WASM Meta",
            "creators": [{"name": "Jane Doe", "role": "aut"}],
            "language": "en",
            "identifier": "urn:uuid:7777",
            "subjects": ["Rust", "WASM"],
            "layout": "Reflowable"
        }"#;
        // In actual JS this would be an Object, but for tests we parse from string using serde_wasm_bindgen
        let meta_obj: crate::model::Metadata = serde_json::from_str(metadata_json).unwrap();
        let meta_js = serde_wasm_bindgen::to_value(&meta_obj).unwrap();
        generator
            .set_metadata(meta_js)
            .expect("Failed to set full metadata via JSON");

        // 2. Add chapters before TOC to pass validation
        generator.add_chapter("part1", "part1.xhtml", "<html>Part 1</html>");
        generator.add_chapter("ch1", "ch1.xhtml", "<html>Chapter 1</html>");

        // 3. Test set_toc with nested JSON
        let toc_json = r#"[
            {
                "title": "Part 1",
                "href": "part1.xhtml",
                "children": [
                    { "title": "Chapter 1", "href": "ch1.xhtml", "children": [] }
                ]
            }
        ]"#;
        let toc_obj: Vec<crate::model::TocEntry> = serde_json::from_str(toc_json).unwrap();
        let toc_js = serde_wasm_bindgen::to_value(&toc_obj).unwrap();
        generator
            .set_toc(toc_js)
            .expect("Failed to set TOC via JSON");

        // 4. Test landmarks and pages
        generator.add_image("cover", "cover.jpg", "image/jpeg", &[1, 2, 3]);
        generator.add_chapter(
            "cover_xhtml",
            "cover.xhtml",
            "<html><img src='cover.jpg'/></html>",
        );
        generator.add_landmark("cover", "cover.xhtml", "Cover Page");
        generator.add_page("1", "ch1.xhtml#p1");

        let bytes = generator
            .generate()
            .expect("Failed to generate EPUB with advanced metadata");
        let parser = EpubParser::new(&bytes);
        let book_js = parser.parse().unwrap();

        // We verify that the metadata survived the roundtrip
        let book: crate::model::EpubBook = serde_wasm_bindgen::from_value(book_js).unwrap();
        assert_eq!(book.metadata.title.unwrap(), "Advanced WASM Meta");
        assert_eq!(book.metadata.subjects.len(), 2);
    }

    #[wasm_bindgen_test]
    fn test_wasm_cfi_utilities() {
        let start_cfi = "epubcfi(/6/4[chap01ref]!/4[body01]/10[para05]/2/1:3)";
        let end_cfi = "epubcfi(/6/4[chap01ref]!/4[body01]/10[para05],/2/1:1,/3:4)";

        // Compare same strings
        let cmp = compare_cfi(start_cfi, start_cfi).unwrap();
        assert_eq!(cmp, 0);

        let range = generate_cfi_range(start_cfi, end_cfi).expect("Failed to generate range CFI");
        assert!(range.starts_with("epubcfi("));
        assert!(range.ends_with(')'));
    }

    #[wasm_bindgen_test]
    fn test_wasm_crypto_decrypt() {
        // IDPF Key logic (SHA1 of identifier stripped of spaces)
        let identifier = "urn:uuid:test-obfuscation";
        let expected_key = crate::crypto::generate_idpf_key(identifier);

        // Dummy encrypted payload
        let mut encrypted_data = vec![0u8; 1040]; // minimum IDPF block size
        for (i, byte) in encrypted_data.iter_mut().enumerate().take(1040) {
            *byte = (i % 256) as u8 ^ expected_key[i % 20];
        }

        // Add some trailing unencrypted data
        encrypted_data.push(0xAA);
        encrypted_data.push(0xBB);

        let decrypted_bytes =
            decrypt_font(&encrypted_data, identifier, true).expect("WASM decryption failed");

        assert_eq!(decrypted_bytes.len(), 1042);
        // The first 1040 bytes should be a linear sequence 0..255 due to XOR logic inversion
        assert_eq!(decrypted_bytes[0], 0);
        assert_eq!(decrypted_bytes[1], 1);
        // Trailing data should be unchanged
        assert_eq!(decrypted_bytes[1040], 0xAA);
        assert_eq!(decrypted_bytes[1041], 0xBB);
    }

    #[wasm_bindgen_test]
    fn test_wasm_rewrite_assets() {
        let mut generator = EpubGenerator::new();

        // Setup metadata
        generator.set_title("WASM Rewrite Test");
        generator.set_language("en");
        generator.set_identifier("urn:uuid:rewrite-123");

        // Add content: An image and a chapter referencing it using relative path
        // The generator uses "OEBPS/" internally, so the href provided here should not have it.
        generator.add_image("img1", "Images/cover.jpg", "image/jpeg", &[1, 2, 3, 4]);
        generator.add_chapter(
            "chapter1",
            "Text/ch1.xhtml",
            r#"
            <html>
                <body>
                    <img src="../Images/cover.jpg" alt="Cover" />
                    <a href="ch2.xhtml#section1">Link</a>
                    <link href="http://external.css" rel="stylesheet" />
                </body>
            </html>
        "#,
        );

        // Generate the EPUB byte array
        let bytes = generator.generate().expect("Failed to generate EPUB bytes");
        let parser = EpubParser::new(&bytes);
        parser.parse().unwrap();

        // Note: The generator prefixes "OEBPS/" automatically for resources and chapters
        // so "OEBPS/Images/cover.jpg" becomes "OEBPS/OEBPS/Images/cover.jpg" in the archive if not handled properly.
        // Based on epub-rs builder internal structure, everything gets put under a base folder (usually "OEBPS/").
        // We will just rewrite based on the generated path format.

        let cb = js_sys::Function::new_with_args(
            "path",
            r#"
                if (path === "OEBPS/Images/cover.jpg") {
                    return "blob:http://fake-blob-url";
                } else if (path === "OEBPS/Text/ch2.xhtml") {
                    return "app://ch2";
                }
                return null;
            "#,
        );

        let rewritten_html = parser
            .get_chapter_with_rewritten_assets("OEBPS/Text/ch1.xhtml", cb)
            .expect("Failed to rewrite assets");

        // The image src should be rewritten to the blob URL
        assert!(rewritten_html.contains(r#"src="blob:http://fake-blob-url""#));
        // The link href should be rewritten but keep the anchor
        assert!(rewritten_html.contains(r#"href="app://ch2#section1""#));
        // External URLs should be left alone
        assert!(rewritten_html.contains(r#"href="http://external.css""#));
    }

    #[wasm_bindgen_test]
    fn test_wasm_validation_errors() {
        let mut generator = EpubGenerator::new();

        // Empty generator should fail validation (missing metadata and spine)
        let err = generator.validate().unwrap_err().as_string().unwrap();
        assert!(err.contains("Missing mandatory metadata: <dc:title>"));
        assert!(err.contains("The spine (reading order) is completely empty."));

        // Partially fix metadata
        generator.set_title("Valid Book");
        generator.set_language("en");
        generator.set_identifier("uuid:12345");

        // Add a TOC pointing to a non-existent file
        let toc_json = r#"[{"title": "Ghost", "href": "ghost.xhtml", "children": []}]"#;
        generator
            .set_toc(
                serde_wasm_bindgen::to_value(
                    &serde_json::from_str::<Vec<crate::model::TocEntry>>(toc_json).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();

        // Attempt to generate, which should now fail due to TOC link and empty spine
        let gen_err = generator.generate().unwrap_err().as_string().unwrap();
        assert!(gen_err.contains("TOC entry 'Ghost' points to missing file: ghost.xhtml"));
    }

    #[wasm_bindgen_test]
    fn test_wasm_generate_locations() {
        let mut generator = EpubGenerator::new();
        generator.set_title("WASM Locations Test");
        generator.set_language("en");
        generator.set_identifier("urn:uuid:loc-123");

        let long_text = "This is a long sentence designed to be split. ".repeat(50);
        generator.add_chapter(
            "chapter1",
            "ch1.xhtml",
            &format!("<html><body><p>{}</p></body></html>", long_text),
        );
        generator.add_chapter(
            "chapter2",
            "ch2.xhtml",
            &format!("<html><body><p>{}</p></body></html>", long_text),
        );

        let bytes = generator.generate().expect("Failed to generate EPUB bytes");
        let parser = EpubParser::new(&bytes);

        // Generate locations, roughly every 50 characters
        let locations_js = parser
            .generate_locations(10)
            .expect("Failed to generate locations");
        let locations: Vec<crate::model::Position> =
            serde_wasm_bindgen::from_value(locations_js).unwrap();

        // Spot check the first location if any
        if !locations.is_empty() {
            let first_loc = &locations[0];
            assert_eq!(first_loc.spine_index, 0);
            assert_eq!(first_loc.global_position, 1);
            assert!(first_loc.cfi.starts_with("epubcfi(/6/2!"));
        }

        // Spot check middle location (should be in chapter 2) if possible
        if locations.len() > 10 {
            let middle_loc = &locations[locations.len() / 2 + 5];
            assert_eq!(middle_loc.spine_index, 1);
            assert!(middle_loc.total_progression > 0.4 && middle_loc.total_progression < 0.6);
        }
    }
}
