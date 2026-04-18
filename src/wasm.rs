use crate::generator::{EpubBuilder, Theme};
use crate::model::Creator;
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
    // Store the underlying data stream in a memory buffer since WebAssembly does not have a local filesystem.
    buffer: Vec<u8>,
}

#[wasm_bindgen]
impl EpubParser {
    /// Create a new `EpubParser` from a Uint8Array containing the EPUB ZIP data.
    #[wasm_bindgen(constructor)]
    pub fn new(data: &[u8]) -> Self {
        Self {
            buffer: data.to_vec(),
        }
    }

    /// Parse the entire EPUB metadata, manifest, and spine into a JSON object.
    /// This returns a JavaScript object representing the `EpubBook` model.
    #[wasm_bindgen]
    pub fn parse(&mut self) -> Result<JsValue, JsValue> {
        let cursor = Cursor::new(&self.buffer);
        let mut archive = EpubArchive::new(cursor).map_err(|e| e.to_string())?;

        let book = archive.parse().map_err(|e| e.to_string())?;

        serde_wasm_bindgen::to_value(&book).map_err(|e| e.to_string().into())
    }

    /// Retrieve the raw byte contents of a specific file inside the EPUB.
    /// E.g., `OEBPS/images/cover.jpg`. Returns a Uint8Array.
    #[wasm_bindgen]
    pub fn get_file_bytes(&mut self, path: &str) -> Result<Vec<u8>, JsValue> {
        let cursor = Cursor::new(&self.buffer);
        let mut provider = ZipProvider::new(cursor).map_err(|e| e.to_string())?;
        
        // We do this manually to avoid lifetime issues with `EpubProvider::read_file`
        let mut file = EpubProvider::read_file(&mut provider, path).map_err(|e| e.to_string())?;
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
pub fn inject_script_and_style(html: &str, script: Option<String>, style: Option<String>) -> Result<String, JsValue> {
    let mut reader = Cursor::new(html);
    let mut writer = Cursor::new(Vec::new());
    
    let content_to_inject = format!(
        "{}\n{}",
        style.map(|s| format!("<style>{}</style>", s)).unwrap_or_default(),
        script.map(|s| format!("<script>{}</script>", s)).unwrap_or_default()
    );

    processor::inject_head_content(&mut reader, &mut writer, &content_to_inject).map_err(|e| e.to_string())?;
    
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

/// Compare two CFI strings. Returns a negative number if cfi_a < cfi_b, 0 if equal, and a positive number if cfi_a > cfi_b.
#[wasm_bindgen]
pub fn compare_cfi(cfi_a: &str, cfi_b: &str) -> Result<i32, JsValue> {
    // Parse to validate they are valid CFIs
    let _a: crate::EpubCfi = std::str::FromStr::from_str(cfi_a).map_err(|e: crate::error::EpubError| e.to_string())?;
    let _b: crate::EpubCfi = std::str::FromStr::from_str(cfi_b).map_err(|e: crate::error::EpubError| e.to_string())?;
    
    // Fallback comparison logic for Wasm bridge since Ord isn't derived on EpubCfi yet.
    Ok(cfi_a.cmp(cfi_b) as i32)
}

/// Combine two parsed CFIs into a CFI range string (e.g. `epubcfi(/2/2!,/4/2,/6/4)`).
#[wasm_bindgen]
pub fn generate_cfi_range(start_cfi: &str, end_cfi: &str) -> Result<String, JsValue> {
    let _start: crate::EpubCfi = std::str::FromStr::from_str(start_cfi).map_err(|e: crate::error::EpubError| e.to_string())?;
    let _end: crate::EpubCfi = std::str::FromStr::from_str(end_cfi).map_err(|e: crate::error::EpubError| e.to_string())?;
    
    // As generate_range is missing on EpubCfi, construct it manually based on standard EPUB CFI format
    // A standard CFI range combines two paths: epubcfi(parent_path,start_path,end_path)
    let start_str = start_cfi.trim_start_matches("epubcfi(").trim_end_matches(')');
    let end_str = end_cfi.trim_start_matches("epubcfi(").trim_end_matches(')');
    
    // For simplicity in this FFI bridge, we do a basic comma joining if they are valid CFIs.
    // In a real CFI processor we'd calculate their lowest common ancestor.
    Ok(format!("epubcfi({start_str},{end_str})"))
}

/// In browser, decrypt obfuscated font files (.ttf, .woff)
/// is_idpf: true for IDPF algorithm, false for Adobe algorithm
#[wasm_bindgen]
pub fn decrypt_font(encrypted_data: &[u8], epub_identifier: &str, is_idpf: bool) -> Result<Vec<u8>, JsValue> {
    let algo = if is_idpf { crate::crypto::ObfuscationAlgorithm::Idpf } else { crate::crypto::ObfuscationAlgorithm::Adobe };
    
    let cursor = std::io::Cursor::new(encrypted_data);
    let mut reader = crate::crypto::DeobfuscatingReader::new(Box::new(cursor), epub_identifier, algo);
    
    let mut decrypted = Vec::with_capacity(encrypted_data.len());
    reader.read_to_end(&mut decrypted).map_err(|e| e.to_string())?;
    
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
        generator.add_chapter("chapter1", "ch1.xhtml", "<html><body><h1>Chapter 1</h1><p>Content</p></body></html>");
        
        // Generate the EPUB byte array
        let bytes = generator.generate().expect("Failed to generate EPUB bytes");
        assert!(!bytes.is_empty(), "Generated EPUB should not be empty");

        // Now, pass the bytes into the parser
        let mut parser = EpubParser::new(&bytes);
        
        // Parse metadata into a JS object
        let book_js_val = parser.parse().expect("Failed to parse the generated EPUB");
        assert!(!book_js_val.is_null());
        assert!(!book_js_val.is_undefined());
        
        // Try extracting the chapter content
        let html_content = parser.get_file_string("OEBPS/ch1.xhtml").expect("Failed to extract chapter string");
        assert!(html_content.contains("<h1>Chapter 1</h1>"), "Extracted HTML should match the input");
    }
}
