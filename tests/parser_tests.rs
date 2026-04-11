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
        println!("Creators: {:?}", book.metadata.creators.iter().map(|c| &c.name).collect::<Vec<_>>());
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
        for (i, item) in book.spine.iter().take(3).enumerate() {
            if let Some(manifest_item) = book.manifest.get(&item.idref) {
                println!("{}. ID: {}, HREF: {}, Media-Type: {}, Linear: {}", i + 1, manifest_item.id, manifest_item.href, manifest_item.media_type, item.linear);
            }
        }
        
        // --- Phase 2: Test Content Processor ---
        println!("\n--- Phase 2: Testing Content Processor ---");
        
        // 1. Fetch raw bytes of the second chapter (index 1) from the archive
        let second_chapter_id = &book.spine[1].idref;
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

    #[test]
    fn test_parse_from_directory() {
        use epub_rs::parser::EpubArchive;
        use std::fs;
        use std::io::Write;
        
        // 1. Unzip an EPUB to a temporary directory to test DirProvider
        let epub_path = "ebooks/软件设计的哲学 (John Ousterhout) (z-library.sk, 1lib.sk, z-lib.sk).epub";
        let file = std::fs::File::open(epub_path).expect("Failed to open EPUB file");
        let mut archive = zip::ZipArchive::new(file).expect("Failed to open ZIP");
        
        let temp_dir = std::env::temp_dir().join("epub_rs_test_explode");
        let _ = fs::remove_dir_all(&temp_dir); // clean up if exists
        
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).unwrap();
            let outpath = match file.enclosed_name() {
                Some(path) => temp_dir.join(path),
                None => continue,
            };

            if (*file.name()).ends_with('/') {
                fs::create_dir_all(&outpath).unwrap();
            } else {
                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        fs::create_dir_all(&p).unwrap();
                    }
                }
                let mut outfile = fs::File::create(&outpath).unwrap();
                std::io::copy(&mut file, &mut outfile).unwrap();
            }
        }
        
        // 2. Test parsing using DirProvider
        let mut dir_archive = EpubArchive::from_dir(&temp_dir);
        let book = dir_archive.parse().expect("Failed to parse EPUB from directory");
        
        assert_eq!(book.metadata.title.as_deref(), Some("\u{200b}软件设计的哲学"));
        assert_eq!(book.manifest.len(), 40);
        assert_eq!(book.spine.len(), 25);
        
        let second_chapter_id = &book.spine[1].idref;
        let raw_html = dir_archive.get_resource_by_id(&book, second_chapter_id).expect("Failed to get HTML resource");
        assert!(!raw_html.is_empty());
        
        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_multiple_renditions() {
        use epub_rs::parser::EpubArchive;
        use std::io::Cursor;
        use std::io::Write;

        let mut buffer = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            
            zip.start_file("META-INF/container.xml", options).unwrap();
            zip.write_all(b"<?xml version=\"1.0\"?>
                <container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">
                    <rootfiles>
                        <rootfile full-path=\"OEBPS/book_reflowable.opf\" media-type=\"application/oebps-package+xml\"/>
                        <rootfile full-path=\"OEBPS/book_fixed.opf\" media-type=\"application/oebps-package+xml\"/>
                    </rootfiles>
                </container>").unwrap();
                
            zip.start_file("OEBPS/book_reflowable.opf", options).unwrap();
            zip.write_all(b"<package><metadata><dc:title>Reflowable Version</dc:title></metadata></package>").unwrap();
            
            zip.start_file("OEBPS/book_fixed.opf", options).unwrap();
            zip.write_all(b"<package><metadata><dc:title>Fixed Version</dc:title></metadata></package>").unwrap();
            
            zip.finish().unwrap();
        }
        
        buffer.set_position(0);
        let mut archive = EpubArchive::new(buffer).expect("Failed to read test ZIP");
        
        let renditions = archive.get_renditions().expect("Failed to get renditions");
        assert_eq!(renditions.len(), 2);
        assert_eq!(renditions[0], "OEBPS/book_reflowable.opf");
        assert_eq!(renditions[1], "OEBPS/book_fixed.opf");
        
        let book1 = archive.parse_rendition(&renditions[0]).expect("Failed to parse first rendition");
        assert_eq!(book1.metadata.title.as_deref(), Some("Reflowable Version"));
        
        let book2 = archive.parse_rendition(&renditions[1]).expect("Failed to parse second rendition");
        assert_eq!(book2.metadata.title.as_deref(), Some("Fixed Version"));
        
        // Calling default parse() should return the first one
        let default_book = archive.parse().expect("Failed to parse default rendition");
        assert_eq!(default_book.metadata.title.as_deref(), Some("Reflowable Version"));
    }
}
