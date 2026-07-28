//! Extension-based media type table.
//!
//! Last-resort MIME guessing for package paths that do not match a manifest
//! item (e.g. mis-authored EPUBs referencing unlisted files). The manifest
//! remains the source of truth — see `EpubArchive::resource_media_type`.

/// Best-effort media type for a package path based on its file extension.
///
/// Returns `"application/octet-stream"` when the extension is unknown, so
/// callers always get a usable value for `data:` URI construction.
pub fn mime_for_path(path: &str) -> &'static str {
    // Strip any query/fragment before looking at the extension.
    let clean = path.split(['?', '#']).next().unwrap_or(path);
    let ext = match clean.rfind('.') {
        Some(i) => &clean[i + 1..],
        None => return "application/octet-stream",
    };
    let mut lower_buf = [0u8; 16];
    let ext_bytes = ext.as_bytes();
    if ext_bytes.len() > lower_buf.len() {
        return "application/octet-stream";
    }
    for (i, b) in ext_bytes.iter().enumerate() {
        lower_buf[i] = b.to_ascii_lowercase();
    }
    let ext_lower = std::str::from_utf8(&lower_buf[..ext_bytes.len()]).unwrap_or("");
    match ext_lower {
        "xhtml" | "html" | "htm" => "application/xhtml+xml",
        "xml" | "xsl" | "opf" => "application/xml",
        "ncx" => "application/x-dtbncx+xml",
        "smil" => "application/smil+xml",
        "pls" => "application/pls+xml",
        "css" => "text/css",
        "js" => "application/javascript",
        "txt" => "text/plain",
        "vtt" => "text/vtt",
        "json" | "map" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp3" => "audio/mpeg",
        "m4a" | "m4b" => "audio/mp4",
        "mp4" => "video/mp4",
        "ogg" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_epub_types() {
        assert_eq!(mime_for_path("a/b/ch1.xhtml"), "application/xhtml+xml");
        assert_eq!(mime_for_path("styles/main.css"), "text/css");
        assert_eq!(mime_for_path("Images/Cover.JPG"), "image/jpeg");
        assert_eq!(mime_for_path("fonts/a.woff2"), "font/woff2");
        assert_eq!(mime_for_path("toc.ncx"), "application/x-dtbncx+xml");
    }

    #[test]
    fn strips_query_and_fragment() {
        assert_eq!(mime_for_path("img/a.png?v=2#x"), "image/png");
    }

    #[test]
    fn unknown_and_missing_extension() {
        assert_eq!(mime_for_path("data/blob.xyz"), "application/octet-stream");
        assert_eq!(mime_for_path("README"), "application/octet-stream");
    }
}
