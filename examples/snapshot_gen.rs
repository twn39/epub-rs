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
    // Remove the dynamic dcterms:modified line
    xml.lines()
        .filter(|line| !line.contains("dcterms:modified"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn main() {
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
        .add_resource_with_properties("style", "css/style.css", "text/css", b"".to_vec(), "nav");

    let mut buffer = Cursor::new(Vec::new());
    builder.generate(&mut buffer).unwrap();

    let opf_xml = extract_zip_file(&buffer.into_inner(), "OEBPS/content.opf");
    let sanitized_opf = sanitize_xml(&opf_xml);

    println!("=== SANITIZED OPF ===");
    println!("{}", sanitized_opf);
    println!("=====================");
}
