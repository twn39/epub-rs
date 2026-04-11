#[cfg(test)]
mod tests {
    use epub_rs::parser::EpubArchive;
    use std::fs::File;

    #[test]
    fn test_parse_real_epub() {
        // Use one of the real EPUB files from the `ebooks` directory
        let file_path = "ebooks/软件设计的哲学 (John Ousterhout) (z-library.sk, 1lib.sk, z-lib.sk).epub";
        
        let file = File::open(file_path).expect("Failed to open EPUB file");
        
        let mut archive = EpubArchive::new(file).expect("Failed to create EpubArchive");
        
        let book = archive.parse().expect("Failed to parse EPUB");
        
        // Basic assertions to ensure metadata and structures are populated
        println!("--- Parsed EPUB Metadata ---");
        println!("Title: {:?}", book.metadata.title);
        println!("Creators: {:?}", book.metadata.creators);
        println!("Language: {:?}", book.metadata.language);
        println!("Identifier: {:?}", book.metadata.identifier);
        
        println!("--- Structure Info ---");
        println!("Manifest items count: {}", book.manifest.len());
        println!("Spine items count: {}", book.spine.len());
        
        assert!(book.metadata.title.is_some(), "Title should not be empty");
        assert!(!book.manifest.is_empty(), "Manifest should not be empty");
        assert!(!book.spine.is_empty(), "Spine should not be empty");
        
        // Print the first few spine items for debugging
        println!("--- First 3 Spine Items ---");
        for (i, id) in book.spine.iter().take(3).enumerate() {
            if let Some(item) = book.manifest.get(id) {
                println!("{}. ID: {}, HREF: {}, Media-Type: {}", i + 1, item.id, item.href, item.media_type);
            }
        }
        
        // --- Phase 2: Test Content Processor ---
        println!("\n--- Phase 2: Testing Content Processor ---");
        
        // 1. Fetch raw bytes of the second chapter (index 1) from the archive
        let second_chapter_id = &book.spine[1];
        let raw_html = archive.get_resource_by_id(&book, second_chapter_id).expect("Failed to get HTML resource");
        
        println!("Chapter 2 Raw HTML Size: {} bytes", raw_html.len());
        assert!(!raw_html.is_empty(), "Chapter HTML should not be empty");

        // 2. Test extracting text (using slice)
        let extracted_text = epub_rs::processor::extract_text(&raw_html).expect("Failed to extract text");
        println!("Extracted Text Preview: {:.100}...", extracted_text);
        assert!(!extracted_text.is_empty(), "Extracted text should not be empty");

        // 2.1 Test extracting text via stream (lazy)
        let mut chapter_stream = archive.read_resource_by_id(&book, second_chapter_id).expect("Failed to get HTML stream");
        let stream_extracted = epub_rs::processor::extract_text_stream(&mut chapter_stream).expect("Failed to extract text from stream");
        assert_eq!(extracted_text, stream_extracted, "Stream extracted text should match slice extracted text");

        // 3. Test link rewriting (using slice)
        let rewritten_html = epub_rs::processor::rewrite_links(&raw_html, |tag, url| {
            if tag == "img" {
                Some(format!("https://cdn.example.com/{}", url))
            } else {
                None
            }
        }).expect("Failed to rewrite links");
        
        println!("Rewritten HTML Size: {} bytes", rewritten_html.len());
        assert!(!rewritten_html.is_empty(), "Rewritten HTML should not be empty");
    }
}
