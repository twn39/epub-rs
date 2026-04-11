#[cfg(test)]
mod tests {
    use epub_rs::generator::EpubBuilder;
    use epub_rs::model::{Creator, EpubVersion, Metadata};
    use std::io::{Cursor, Read};

    fn extract_zip_file(zip_bytes: &[u8], name: &str) -> String {
        let reader = Cursor::new(zip_bytes);
        let mut archive = zip::ZipArchive::new(reader).unwrap();
        let mut file = archive.by_name(name).unwrap();
        let mut content = String::new();
        file.read_to_string(&mut content).unwrap();
        content
    }

    fn sanitize_xml(xml: &str) -> String {
        // Remove the dynamic dcterms:modified line to make the snapshot deterministic
        xml.lines()
            .filter(|line| !line.contains("dcterms:modified"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn test_opf_golden_snapshot() {
        let mut author = Creator::new("Golden Author");
        author.role = Some("aut".to_string());

        let metadata = Metadata {
            title: Some("Golden Book".to_string()),
            identifier: Some("urn:uuid:golden-12345".to_string()),
            language: Some("en-US".to_string()),
            creators: vec![author],
            ..Default::default()
        };

        let builder = EpubBuilder::new()
            .version(EpubVersion::V30)
            .metadata(metadata)
            .add_chapter("chapter1", "text/ch1.xhtml", b"Hello".to_vec())
            .add_resource_with_properties(
                "style",
                "css/style.css",
                "text/css",
                b"".to_vec(),
                "nav",
            );

        let mut buffer = Cursor::new(Vec::new());
        builder.generate(&mut buffer).unwrap();

        let opf_xml = extract_zip_file(&buffer.into_inner(), "OEBPS/content.opf");
        let sanitized_opf = sanitize_xml(&opf_xml);

        let expected_opf = r##"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" unique-identifier="pub-id" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Golden Book</dc:title>
    <dc:creator id="creator_0">Golden Author</dc:creator>
    <meta refines="#creator_0" property="role" scheme="marc:relators">aut</meta>
    <dc:language>en-US</dc:language>
    <dc:identifier id="pub-id">urn:uuid:golden-12345</dc:identifier>
  </metadata>
  <manifest>
    <item id="chapter1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="style" href="css/style.css" media-type="text/css" properties="nav"/>
  </manifest>
  <spine>
    <itemref idref="chapter1"/>
  </spine>
</package>"##;

        assert_eq!(sanitized_opf.trim(), expected_opf.trim());
    }
}
