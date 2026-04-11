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
        
        let mut author = epub_rs::model::Creator::new("Rust Developer");
        author.role = Some("aut".to_string());
        author.file_as = Some("Developer, Rust".to_string());
        metadata.creators.push(author);
        
        let mut translator = epub_rs::model::Creator::new("Gemini Bot");
        translator.role = Some("trl".to_string());
        metadata.creators.push(translator);

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
            .add_landmark("cover", "text/ch1.xhtml", "封面")
            .add_landmark("toc", "nav.xhtml", "目录")
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
        
        assert_eq!(book.metadata.creators.len(), 2);
        assert_eq!(book.metadata.creators[0].name, "Rust Developer");
        assert_eq!(book.metadata.creators[0].role.as_deref(), Some("aut"));
        assert_eq!(book.metadata.creators[0].file_as.as_deref(), Some("Developer, Rust"));
        
        assert_eq!(book.metadata.creators[1].name, "Gemini Bot");
        assert_eq!(book.metadata.creators[1].role.as_deref(), Some("trl"));

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
        assert_eq!(book.spine[0].idref, "chapter1");
        assert_eq!(book.spine[1].idref, "chapter1_1");
        
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
        
        // Check Landmarks
        assert!(nav_str.contains("epub:type=\"landmarks\""));
        assert!(nav_str.contains("epub:type=\"cover\" href=\"text/ch1.xhtml\">封面</a>"));
        
        // Extract and verify content
        let extracted_css = archive.get_resource_by_id(&book, "style.css").expect("Failed to get css");
        assert_eq!(extracted_css, css_content);
        
        let extracted_html = archive.get_resource_by_id(&book, "chapter1").expect("Failed to get html");
        assert_eq!(extracted_html, chapter1_html);
        
        let extracted_text = epub_rs::processor::extract_text(&extracted_html).expect("Failed to extract text");
        assert!(extracted_text.contains("Hello World"));
        assert!(extracted_text.contains("This is a generated EPUB chapter."));
    }

    #[test]
    fn test_generate_epub_v2() {
        use epub_rs::model::{EpubVersion, Metadata, Creator};

        let mut metadata = Metadata::default();
        metadata.title = Some("Legacy Book".to_string());
        metadata.creators.push(Creator {
            name: "V2 Author".to_string(),
            role: Some("aut".to_string()),
            file_as: Some("Author, V2".to_string()),
        });

        let builder = EpubBuilder::new()
            .version(EpubVersion::V20)
            .metadata(metadata)
            .add_chapter("chapter1", "text/ch1.xhtml", b"<html><body>Legacy</body></html>".to_vec());

        let mut buffer = Cursor::new(Vec::new());
        builder.generate(&mut buffer).expect("Failed to generate V2 EPUB");

        buffer.set_position(0);
        let mut archive = EpubArchive::new(buffer).expect("Failed to open V2 generated EPUB");
        let book = archive.parse().expect("Failed to parse V2 generated EPUB");

        assert_eq!(book.metadata.title.as_deref(), Some("Legacy Book"));
        assert_eq!(book.manifest.len(), 1); // Only chapter1, no nav since V2, no NCX since TOC is empty
        assert!(book.manifest.contains_key("chapter1"));
        assert!(!book.manifest.contains_key("nav"));
    }

    #[test]
    fn test_fixed_layout_generation() {
        use epub_rs::model::{EpubVersion, Metadata, LayoutType, PageSpread};

        let mut metadata = Metadata::default();
        metadata.title = Some("Comic Book".to_string());
        metadata.layout = LayoutType::PrePaginated; // Global fixed layout

        let builder = EpubBuilder::new()
            .version(EpubVersion::V30)
            .metadata(metadata)
            // page 1 is a double-page spread center
            .add_chapter_with_layout("page1", "text/p1.xhtml", b"<html><body>Page 1</body></html>".to_vec(), None, Some(PageSpread::Center))
            // page 2 forces reflowable (override) and goes on the left
            .add_chapter_with_layout("page2", "text/p2.xhtml", b"<html><body>Page 2</body></html>".to_vec(), Some(LayoutType::Reflowable), Some(PageSpread::Left));

        let mut buffer = Cursor::new(Vec::new());
        builder.generate(&mut buffer).expect("Failed to generate FXL EPUB");

        buffer.set_position(0);
        let mut archive = EpubArchive::new(buffer).expect("Failed to open FXL EPUB");
        let book = archive.parse().expect("Failed to parse FXL EPUB");

        // Verify Global Layout
        assert_eq!(book.metadata.layout, LayoutType::PrePaginated);

        // Verify Spine Items overrides
        assert_eq!(book.spine[0].page_spread, Some(PageSpread::Center));
        assert_eq!(book.spine[0].layout_override, None);

        assert_eq!(book.spine[1].page_spread, Some(PageSpread::Left));
        assert_eq!(book.spine[1].layout_override, Some(LayoutType::Reflowable));
    }
}
