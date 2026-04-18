//! EPUB Error Types

use thiserror::Error;

#[derive(Error, Debug)]
pub enum EpubError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("ZIP archive error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("XML parsing error: {0}")]
    Xml(#[from] quick_xml::Error),

    #[error("XML attribute parsing error: {0}")]
    XmlAttr(#[from] quick_xml::events::attributes::AttrError),

    #[error("XML escape error: {0}")]
    XmlEscape(#[from] quick_xml::escape::EscapeError),

    #[error("Missing META-INF/container.xml")]
    MissingContainer,

    #[error("Invalid EPUB format: {0}")]
    InvalidFormat(String),

    #[error("HTML processing error: {0}")]
    HtmlParse(String),
}
