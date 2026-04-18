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
