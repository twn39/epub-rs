//! ZIP package assembly for generated EPUBs.
//!
//! Keeps mimetype-first / compression policy out of the builder facade so
//! `EpubBuilder` can focus on validation and resource orchestration.

use crate::error::EpubError;
use std::io::{Seek, Write};
use zip::CompressionMethod;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use super::ResourceContent;

/// Write a complete EPUB ZIP: uncompressed `mimetype` first, then remaining entries.
pub(super) fn write_epub_zip<W: Write + Seek>(
    writer: W,
    resources: impl IntoIterator<Item = (String, ResourceContent)>,
    content_opf: &[u8],
) -> Result<(), EpubError> {
    let mut zip = ZipWriter::new(writer);

    // 1. Write `mimetype` (MUST be first, MUST be uncompressed)
    let options_stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", options_stored)?;
    zip.write_all(b"application/epub+zip")?;

    let options_deflated =
        SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    // 2. Write `META-INF/container.xml`
    zip.start_file("META-INF/container.xml", options_deflated)?;
    let container_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
    <rootfiles>
        <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
    </rootfiles>
</container>"#;
    zip.write_all(container_xml.as_bytes())?;

    // 3. Write package resources under OEBPS/
    for (href, content) in resources {
        let zip_path = format!("OEBPS/{href}");
        zip.start_file(&zip_path, options_deflated)?;
        match content {
            ResourceContent::Bytes(bytes) => {
                zip.write_all(&bytes)?;
            }
            ResourceContent::Stream(mut stream) => {
                std::io::copy(&mut stream, &mut zip)?;
            }
        }
    }

    // 4. Write `OEBPS/content.opf`
    zip.start_file("OEBPS/content.opf", options_deflated)?;
    zip.write_all(content_opf)?;

    zip.finish()?;
    Ok(())
}
