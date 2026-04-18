use crate::generator::EpubBuilder;
use crate::parser::EpubArchive;
use wasm_bindgen::prelude::*;

/// Parse an EPUB file from a byte array.
/// Returns a JSON string containing the book's metadata and structure.
#[wasm_bindgen]
pub fn parse_epub(data: &[u8]) -> Result<JsValue, JsValue> {
    let cursor = std::io::Cursor::new(data);
    let mut archive = EpubArchive::new(cursor).map_err(|e| e.to_string())?;
    
    // As a simple example, we just return the parsed book structure.
    let book = archive.parse().map_err(|e| e.to_string())?;
    
    // Serialize to JSON to easily pass it back to JS
    let json = serde_json::to_string(&book).map_err(|e| e.to_string())?;
    Ok(JsValue::from_str(&json))
}

/// Expose a simple way to generate an empty EPUB or you could expand this
/// to take JS objects and build the EPUB in WASM.
#[wasm_bindgen]
pub fn generate_empty_epub() -> Result<Vec<u8>, JsValue> {
    let builder = EpubBuilder::new();
    // Add some default content if desired...
    
    let mut output = std::io::Cursor::new(Vec::new());
    builder.generate(&mut output).map_err(|e| e.to_string())?;
    Ok(output.into_inner())
}
