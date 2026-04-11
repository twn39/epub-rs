#[cfg(test)]
mod tests {
    use epub_rs::generator::{EpubBuilder, TocEntry};
    use epub_rs::model::Metadata;
    use epub_rs::parser::EpubArchive;
    use std::io::Cursor;

    #[test]
    fn test_generate_and_parse_epub() {
        // 1. Build an EPUB in memory
        let mut metadata = Metadata::default();
        metadata.title = Some("Test Generated Book".to_string());
        metadata.creators.push("Rust Developer".to_string());
        metadata.language = Some("zh-CN".to_string());
        metadata.publisher = Some("Gemini Press".to_string());
        metadata.subjects.push("Technology".to_string());
        metadata.subjects.push("Rust".to_string());
        
        let chapter1_html = br#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Chapter 1</title></head>
<body><h1>Hello World</h1><p>This is a generated EPUB chapter.</p></body>
</html>"#;

        let chapter2_html = br#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Chapter 1.1</title></head>
<body><h2>Nested Section</h2><p>This is nested.</p></body>
</html>"#;

        let css_content = b"h1 { color: red; }";
        let cover_content = b"fake image bytes";

        // Create a nested TOC
        let root_entry = TocEntry::new("第一章", "text/ch1.xhtml")
            .add_child(TocEntry::new("第一节", "text/ch1_1.xhtml"));
            
        // Test resource stream via Cursor
        let stream_content = b"Streamed content".to_vec();
        let stream_reader = std::io::Cursor::new(stream_content);
        
        let builder = EpubBuilder::new()
            .metadata(metadata)
            .set_cover("images/cover.jpg", "image/jpeg", cover_content.to_vec())
            .add_resource("style.css", "css/style.css", "text/css", css_content.to_vec())
            .add_chapter("chapter1", "text/ch1.xhtml", chapter1_html.to_vec())
            .add_chapter("chapter1_1", "text/ch1_1.xhtml", chapter2_html.to_vec())
            .add_resource_stream("stream1", "stream.txt", "text/plain", stream_reader)
            .set_toc(vec![root_entry]);
        
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
        assert_eq!(book.metadata.publisher.as_deref(), Some("Gemini Press"));
        assert_eq!(book.metadata.subjects, vec!["Technology".to_string(), "Rust".to_string()]);
        
        // Verify Manifest (css, cover, ch1, ch1_1, nav, ncx)
        assert_eq!(book.manifest.len(), 7);
        assert!(book.manifest.contains_key("cover-image"));
        assert!(book.manifest.contains_key("style.css"));
        assert!(book.manifest.contains_key("chapter1"));
        assert!(book.manifest.contains_key("chapter1_1"));
        assert!(book.manifest.contains_key("stream1"));
        assert!(book.manifest.contains_key("nav"));
        assert!(book.manifest.contains_key("ncx"));
        
        let extracted_stream = archive.get_resource_by_id(&book, "stream1").expect("Failed to get stream resource");
        assert_eq!(extracted_stream, b"Streamed content".to_vec());
        
        let cover_item = book.manifest.get("cover-image").unwrap();
        assert_eq!(cover_item.media_type, "image/jpeg");

        // Verify Spine
        assert_eq!(book.spine.len(), 2);
        assert_eq!(book.spine[0], "chapter1");
        assert_eq!(book.spine[1], "chapter1_1");
        
        // Check generated ncx content
        let extracted_ncx = archive.get_resource_by_id(&book, "ncx").expect("Failed to get ncx");
        let ncx_str = String::from_utf8_lossy(&extracted_ncx);
        assert!(ncx_str.contains("<text>第一章</text>"));
        assert!(ncx_str.contains("<text>第一节</text>")); // nested TOC element
        assert!(ncx_str.contains("playOrder=\"1\""));
        assert!(ncx_str.contains("playOrder=\"2\""));
        
        // Check nav.xhtml content
        let extracted_nav = archive.get_resource_by_id(&book, "nav").expect("Failed to get nav");
        let nav_str = String::from_utf8_lossy(&extracted_nav);
        // It should contain nested <ol> inside <li>
        assert!(nav_str.contains("<li><a href=\"text/ch1.xhtml\">第一章</a>"));
        assert!(nav_str.contains("<li><a href=\"text/ch1_1.xhtml\">第一节</a>"));
        
        // Extract and verify content
        let extracted_css = archive.get_resource_by_id(&book, "style.css").expect("Failed to get css");
        assert_eq!(extracted_css, css_content);
        
        let extracted_html = archive.get_resource_by_id(&book, "chapter1").expect("Failed to get html");
        assert_eq!(extracted_html, chapter1_html);
        
        let extracted_text = epub_rs::processor::extract_text(&extracted_html).expect("Failed to extract text");
        assert!(extracted_text.contains("Hello World"));
        assert!(extracted_text.contains("This is a generated EPUB chapter."));
    }
}
