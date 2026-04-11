#[cfg(test)]
mod tests {
    use epub_rs::generator::EpubBuilder;
    use epub_rs::model::Metadata;
    use epub_rs::parser::EpubArchive;
    use std::fs::File;
    use std::io::Cursor;

    #[test]
    fn test_generate_and_parse_epub() {
        // 1. Build an EPUB in memory
        let mut metadata = Metadata::default();
        metadata.title = Some("Test Generated Book".to_string());
        metadata.creators.push("Rust Developer".to_string());
        metadata.language = Some("zh-CN".to_string());
        
        let chapter_html = br#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Chapter 1</title></head>
<body><h1>Hello World</h1><p>This is a generated EPUB chapter.</p></body>
</html>"#;

        let css_content = b"h1 { color: red; }";

        let builder = EpubBuilder::new()
            .metadata(metadata)
            .add_resource("style.css", "css/style.css", "text/css", css_content.to_vec())
            .add_chapter_with_nav("chapter1", "text/ch1.xhtml", "第一章", chapter_html.to_vec());
        
        // Write to an in-memory buffer
        let mut buffer = Cursor::new(Vec::new());
        builder.generate(&mut buffer).expect("Failed to generate EPUB");

        // 2. Parse the generated EPUB back
        buffer.set_position(0);
        let mut archive = EpubArchive::new(buffer).expect("Failed to open generated EPUB");
        let book = archive.parse().expect("Failed to parse generated EPUB");

        // 3. Verify contents
        assert_eq!(book.metadata.title.as_deref(), Some("Test Generated Book"));
        assert_eq!(book.metadata.creators, vec!["Rust Developer".to_string()]);
        assert_eq!(book.metadata.language.as_deref(), Some("zh-CN"));
        
        // Verify Manifest
        assert_eq!(book.manifest.len(), 4); // css, chapter1, nav, ncx
        assert!(book.manifest.contains_key("style.css"));
        assert!(book.manifest.contains_key("chapter1"));
        assert!(book.manifest.contains_key("nav"));
        assert!(book.manifest.contains_key("ncx"));
        
        let nav_item = book.manifest.get("nav").unwrap();
        assert_eq!(nav_item.media_type, "application/xhtml+xml");

        // Verify Spine
        assert_eq!(book.spine.len(), 1);
        assert_eq!(book.spine[0], "chapter1");
        
        // Check generated ncx content
        let extracted_ncx = archive.get_resource_by_id(&book, "ncx").expect("Failed to get ncx");
        let ncx_str = String::from_utf8_lossy(&extracted_ncx);
        assert!(ncx_str.contains("<text>第一章</text>"));
        assert!(ncx_str.contains("src=\"text/ch1.xhtml\""));
        
        // Extract and verify content
        let extracted_css = archive.get_resource_by_id(&book, "style.css").expect("Failed to get css");
        assert_eq!(extracted_css, css_content);
        
        let extracted_html = archive.get_resource_by_id(&book, "chapter1").expect("Failed to get html");
        assert_eq!(extracted_html, chapter_html);
        
        let extracted_text = epub_rs::processor::extract_text(&extracted_html).expect("Failed to extract text");
        assert!(extracted_text.contains("Hello World"));
        assert!(extracted_text.contains("This is a generated EPUB chapter."));
    }
}
