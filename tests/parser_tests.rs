#[cfg(test)]
mod tests {
    use epub_rs::parser::EpubArchive;
    

    #[test]
    fn test_parse_real_epub() {
        // Use one of the real EPUB files from the `ebooks` directory
        let file_path = "ebooks/软件设计的哲学 (John Ousterhout) (z-library.sk, 1lib.sk, z-lib.sk).epub";

        // Skip the test in CI if the file is not found
        let file = match std::fs::File::open(file_path) {
            Ok(f) => f,
            Err(_) => return,
        };

        let mut archive = EpubArchive::new(file).expect("Failed to create EpubArchive");

        let book = archive.parse().expect("Failed to parse EPUB");

        // Basic assertions to ensure metadata and structures are populated
        println!("--- Parsed EPUB Metadata ---");
        println!("Title: {:?}", book.metadata.title);
        println!(
            "Creators: {:?}",
            book.metadata
                .creators
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
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
                println!(
                    "{}. ID: {}, HREF: {}, Media-Type: {}, Linear: {}",
                    i + 1,
                    manifest_item.id,
                    manifest_item.href,
                    manifest_item.media_type,
                    item.linear
                );
            }
        }

        // --- Phase 2: Test Content Processor ---
        println!("\n--- Phase 2: Testing Content Processor ---");

        // 1. Fetch raw bytes of the second chapter (index 1) from the archive
        let second_chapter_id = &book.spine[1].idref;
        let raw_html = archive
            .get_resource_by_id(&book, second_chapter_id)
            .expect("Failed to get HTML resource");

        println!("Chapter 2 Raw HTML Size: {} bytes", raw_html.len());
        assert!(!raw_html.is_empty(), "Chapter HTML should not be empty");

        // 2. Test extracting text (using slice)
        let extracted_text =
            epub_rs::processor::extract_text(&raw_html).expect("Failed to extract text");
        println!("Extracted Text Preview: {:.100}...", extracted_text);
        assert!(
            !extracted_text.is_empty(),
            "Extracted text should not be empty"
        );

        // 2.1 Test extracting text via stream (lazy)
        {
            let mut chapter_stream = archive
                .read_resource_by_id(&book, second_chapter_id)
                .expect("Failed to get HTML stream");
            let stream_extracted = epub_rs::processor::extract_text_stream(&mut chapter_stream)
                .expect("Failed to extract text from stream");
            assert_eq!(
                extracted_text, stream_extracted,
                "Stream extracted text should match slice extracted text"
            );
        }

        // 3. Test link rewriting (using slice)
        let rewritten_html = epub_rs::processor::rewrite_links(&raw_html, |tag, url| {
            if tag == "img" {
                Some(format!("https://cdn.example.com/{}", url))
            } else {
                None
            }
        })
        .expect("Failed to rewrite links");

        println!("Rewritten HTML Size: {} bytes", rewritten_html.len());
        assert!(
            !rewritten_html.is_empty(),
            "Rewritten HTML should not be empty"
        );

        // --- Phase 5: Test Cover & TOC ---
        println!("\n--- Phase 5: Testing Smart API Extraction ---");
        let (cover_bytes, cover_mime) = archive
            .get_cover_image(&book)
            .expect("Failed to get cover image");
        println!(
            "Cover Image extracted! Size: {} bytes, Type: {}",
            cover_bytes.len(),
            cover_mime
        );
        assert!(!cover_bytes.is_empty());
        assert!(cover_mime.starts_with("image/"));

        let toc = archive.get_toc(&book).expect("Failed to get TOC");
        println!("TOC Root Entries: {}", toc.len());
        assert!(!toc.is_empty());

        // Print first TOC entry
        if let Some(first) = toc.first() {
            println!("First TOC Entry: '{}' -> {}", first.title, first.href);
            assert!(!first.title.is_empty());
        }
    }

    #[test]
    fn test_parse_from_directory() {
        use epub_rs::parser::EpubArchive;
        use std::fs;

        // 1. Unzip an EPUB to a temporary directory to test DirProvider
        let epub_path = "ebooks/软件设计的哲学 (John Ousterhout) (z-library.sk, 1lib.sk, z-lib.sk).epub";

        // Skip the test in CI if the file is not found
        let file = match std::fs::File::open(epub_path) {
            Ok(f) => f,
            Err(_) => return,
        };

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
                if let Some(p) = outpath.parent()
                    && !p.exists()
                {
                    fs::create_dir_all(p).unwrap();
                }
                let mut outfile = fs::File::create(&outpath).unwrap();
                std::io::copy(&mut file, &mut outfile).unwrap();
            }
        }

        // 2. Test parsing using DirProvider
        let mut dir_archive = EpubArchive::from_dir(&temp_dir);
        let book = dir_archive
            .parse()
            .expect("Failed to parse EPUB from directory");

        assert_eq!(
            book.metadata.title.as_deref(),
            Some("\u{200b}软件设计的哲学")
        );
        assert_eq!(book.manifest.len(), 40);
        assert_eq!(book.spine.len(), 25);

        let second_chapter_id = &book.spine[1].idref;
        let raw_html = dir_archive
            .get_resource_by_id(&book, second_chapter_id)
            .expect("Failed to get HTML resource");
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

            zip.start_file("OEBPS/book_reflowable.opf", options)
                .unwrap();
            zip.write_all(
                b"<package><metadata><dc:title>Reflowable Version</dc:title></metadata></package>",
            )
            .unwrap();

            zip.start_file("OEBPS/book_fixed.opf", options).unwrap();
            zip.write_all(
                b"<package><metadata><dc:title>Fixed Version</dc:title></metadata></package>",
            )
            .unwrap();

            zip.finish().unwrap();
        }

        buffer.set_position(0);
        let mut archive = EpubArchive::new(buffer).expect("Failed to read test ZIP");

        let renditions = archive.get_renditions().expect("Failed to get renditions");
        assert_eq!(renditions.len(), 2);
        assert_eq!(renditions[0], "OEBPS/book_reflowable.opf");
        assert_eq!(renditions[1], "OEBPS/book_fixed.opf");

        let book1 = archive
            .parse_rendition(&renditions[0])
            .expect("Failed to parse first rendition");
        assert_eq!(book1.metadata.title.as_deref(), Some("Reflowable Version"));

        let book2 = archive
            .parse_rendition(&renditions[1])
            .expect("Failed to parse second rendition");
        assert_eq!(book2.metadata.title.as_deref(), Some("Fixed Version"));

        // Calling default parse() should return the first one
        let default_book = archive.parse().expect("Failed to parse default rendition");
        assert_eq!(
            default_book.metadata.title.as_deref(),
            Some("Reflowable Version")
        );
    }

    #[test]
    fn test_error_handling_bad_zip() {
        use epub_rs::parser::EpubArchive;
        use std::io::Cursor;

        let bad_zip_bytes = b"This is clearly not a ZIP file at all".to_vec();
        let buffer = Cursor::new(bad_zip_bytes);

        let result = EpubArchive::new(buffer);
        assert!(result.is_err());

        if let Err(e) = result {
            match e {
                epub_rs::error::EpubError::Zip(_) => {} // Expected
                e => panic!("Expected Zip error, got {:?}", e),
            }
        } else {
            panic!("Expected error, but got Ok");
        }
    }

    #[test]
    fn test_error_handling_missing_container() {
        use epub_rs::parser::EpubArchive;
        use std::io::{Cursor, Write};

        let mut buffer = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            // We intentionally do NOT write META-INF/container.xml
            zip.start_file("mimetype", options).unwrap();
            zip.write_all(b"application/epub+zip").unwrap();
            zip.finish().unwrap();
        }

        buffer.set_position(0);
        let mut archive = EpubArchive::new(buffer).expect("Should open valid ZIP");

        let result = archive.parse();
        assert!(result.is_err());

        if let Err(e) = result {
            match e {
                epub_rs::error::EpubError::MissingContainer => {} // Expected
                e => panic!("Expected MissingContainer error, got {:?}", e),
            }
        } else {
            panic!("Expected error, but got Ok");
        }
    }

    #[test]
    fn test_error_handling_malformed_container() {
        use epub_rs::parser::EpubArchive;
        use std::io::{Cursor, Write};

        let mut buffer = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();

            // Malformed XML without the rootfile element
            zip.start_file("META-INF/container.xml", options).unwrap();
            zip.write_all(b"<?xml version=\"1.0\"?>
                <container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">
                    <rootfiles>
                        <!-- MISSING ROOTFILE -->
                    </rootfiles>
                </container>").unwrap();
            zip.finish().unwrap();
        }

        buffer.set_position(0);
        let mut archive = EpubArchive::new(buffer).expect("Should open valid ZIP");

        let result = archive.parse();
        assert!(result.is_err());

        if let Err(e) = result {
            match e {
                epub_rs::error::EpubError::InvalidFormat(msg) => {
                    assert!(msg.contains("No rootfile full-path found"));
                }
                e => panic!("Expected InvalidFormat error, got {:?}", e),
            }
        } else {
            panic!("Expected error, but got Ok");
        }
    }

    #[test]
    fn test_relative_path_resolution() {
        use epub_rs::parser::EpubArchive;
        use std::io::{Cursor, Write};

        let mut buffer = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();

            zip.start_file("META-INF/container.xml", options).unwrap();
            zip.write_all(b"<?xml version=\"1.0\"?>
                <container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">
                    <rootfiles>
                        <rootfile full-path=\"OEBPS/content/book.opf\" media-type=\"application/oebps-package+xml\"/>
                    </rootfiles>
                </container>").unwrap();

            zip.start_file("OEBPS/content/book.opf", options).unwrap();
            zip.write_all(
                b"<package><metadata></metadata><manifest>
                <item id=\"img\" href=\"../images/cover.jpg\" media-type=\"image/jpeg\"/>
                <item id=\"ch1\" href=\"./ch1.xhtml\" media-type=\"application/xhtml+xml\"/>
            </manifest></package>",
            )
            .unwrap();

            zip.start_file("OEBPS/images/cover.jpg", options).unwrap();
            zip.write_all(b"fake_image_bytes").unwrap();

            zip.start_file("OEBPS/content/ch1.xhtml", options).unwrap();
            zip.write_all(b"fake_html").unwrap();

            zip.finish().unwrap();
        }

        buffer.set_position(0);
        let mut archive = EpubArchive::new(buffer).unwrap();
        let book = archive.parse().unwrap();

        assert_eq!(book.opf_dir, "OEBPS/content");

        // Retrieve the image mapped by a relative URL ("../images/cover.jpg")
        let img_bytes = archive
            .get_resource_by_id(&book, "img")
            .expect("Failed to resolve ../ relative path");
        assert_eq!(img_bytes, b"fake_image_bytes");

        // Retrieve the html mapped by a relative URL ("./ch1.xhtml")
        let html_bytes = archive
            .get_resource_by_id(&book, "ch1")
            .expect("Failed to resolve ./ relative path");
        assert_eq!(html_bytes, b"fake_html");
    }
}
