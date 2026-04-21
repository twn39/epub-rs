//! EPUB Archive Providers (Storage Layer)

#[cfg(not(target_arch = "wasm32"))]
use std::fs::File;
use std::io::{Read, Seek};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Component, Path, PathBuf};
use zip::ZipArchive;

use crate::error::EpubError;

/// An abstract trait for reading files from an EPUB package.
/// This allows EPUBs to be loaded from ZIP files, plain directories, or even network streams.
pub trait EpubProvider {
    /// Read a file from the package into a reader stream.
    fn read_file<'a>(&'a mut self, path: &str) -> Result<Box<dyn Read + 'a>, EpubError>;
}

/// A provider that reads EPUB files from a standard ZIP archive.
pub struct ZipProvider<R: Read + Seek> {
    archive: ZipArchive<R>,
}

/// Image file extensions that are considered interchangeable when a path can't be found.
/// Some EPUBs are incorrectly authored with mismatched extensions (e.g. referencing `.jpg`
/// when the actual file is stored as `.png`). We perform a best-effort alias lookup to
/// recover from this class of EPUB authoring errors.
const IMAGE_EXT_ALIASES: &[&[&str]] = &[&["jpg", "jpeg", "png", "gif", "webp", "bmp"]];

impl<R: Read + Seek> ZipProvider<R> {
    pub fn new(reader: R) -> Result<Self, EpubError> {
        let archive = ZipArchive::new(reader)?;
        Ok(Self { archive })
    }

    /// Resolve a logical path to a real entry name that exists in the archive.
    ///
    /// Lookup order:
    /// 1. Exact match (`by_name`).
    /// 2. Case-insensitive scan across all entries.
    /// 3. Case-insensitive scan with image-extension aliases (handles mismatched `.jpg`/`.png`).
    ///
    /// Returns `None` if no matching entry can be found.
    fn resolve_zip_name(&self, path: &str) -> Option<String> {
        // 1. Exact match — cheapest, try first.
        if self.archive.index_for_name(path).is_some() {
            return Some(path.to_string());
        }

        let path_lower = path.to_lowercase();

        // Split the stem and extension for alias fallback.
        let (stem_lower, ext_lower): (&str, &str) = match path_lower.rfind('.') {
            Some(dot) => (&path_lower[..dot], &path_lower[dot + 1..]),
            None => (path_lower.as_str(), ""),
        };

        // Determine the alias group for this extension (if any).
        let alias_group: Option<&[&str]> = IMAGE_EXT_ALIASES
            .iter()
            .find(|group| group.contains(&ext_lower))
            .copied();

        // 2 & 3: Single linear scan over all entry names.
        // Collect all names then do the comparison (avoids borrow conflict with archive).
        let names: Vec<String> = self.archive.file_names().map(|s| s.to_string()).collect();

        for name in &names {
            let name_lower = name.to_lowercase();

            // 2. Case-insensitive exact path match.
            if name_lower == path_lower {
                return Some(name.clone());
            }

            // 3. Case-insensitive match with extension alias.
            if let Some(aliases) = alias_group {
                let (name_stem_lower, name_ext_lower): (&str, &str) = match name_lower.rfind('.') {
                    Some(dot) => (&name_lower[..dot], &name_lower[dot + 1..]),
                    None => (name_lower.as_str(), ""),
                };

                if name_stem_lower == stem_lower && aliases.contains(&name_ext_lower) {
                    return Some(name.clone());
                }
            }
        }

        None
    }
}

impl<R: Read + Seek> EpubProvider for ZipProvider<R> {
    fn read_file<'a>(&'a mut self, path: &str) -> Result<Box<dyn Read + 'a>, EpubError> {
        // Use the tolerant resolver to handle case mismatches and extension aliases.
        let resolved = self
            .resolve_zip_name(path)
            .ok_or(EpubError::Zip(zip::result::ZipError::FileNotFound))?;

        let file = self.archive.by_name(&resolved)?;
        Ok(Box::new(file))
    }
}

/// A provider that reads EPUB files directly from an unzipped, exploded directory.
#[cfg(not(target_arch = "wasm32"))]
pub struct DirProvider {
    root: PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl DirProvider {
    pub fn new<P: AsRef<Path>>(root: P) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }
}

/// Resolves `path` relative to `root` while normalising `.` and `..` segments.
/// Returns an error if the resolved path escapes `root` (path-traversal guard).
#[cfg(not(target_arch = "wasm32"))]
fn safe_join(root: &Path, path: &str) -> Result<PathBuf, EpubError> {
    let mut resolved = root.to_path_buf();

    for component in Path::new(path).components() {
        match component {
            Component::ParentDir if !resolved.pop() || !resolved.starts_with(root) => {
                // Pop one level; if we can no longer stay inside root, reject.
                return Err(EpubError::InvalidFormat(format!(
                    "Security: path '{}' attempts to escape the EPUB root directory",
                    path
                )));
            }
            Component::ParentDir => {} // Still within bounds after pop
            Component::Normal(c) => {
                resolved.push(c);
            }
            Component::CurDir => {} // '.' — stay where we are
            // RootDir or Prefix would also be suspicious; ignore silently (they can't
            // appear in well-formed relative paths that come from inside a ZIP).
            _ => {}
        }
    }

    // Final safety check: resolved path must still be inside root.
    if !resolved.starts_with(root) {
        return Err(EpubError::InvalidFormat(format!(
            "Security: resolved path for '{}' is outside the EPUB root directory",
            path
        )));
    }

    Ok(resolved)
}

#[cfg(not(target_arch = "wasm32"))]
impl EpubProvider for DirProvider {
    fn read_file<'a>(&'a mut self, path: &str) -> Result<Box<dyn Read + 'a>, EpubError> {
        let full_path = safe_join(&self.root, path)?;
        let file = File::open(&full_path).map_err(EpubError::Io)?;
        Ok(Box::new(file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn create_mock_zip() -> Vec<u8> {
        let mut buf = Vec::new();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("OEBPS/image.png", opts).unwrap();
        writer.write_all(b"fake-png").unwrap();
        writer
            .start_file("OEBPS/Text/Chapter1.xhtml", opts)
            .unwrap();
        writer.write_all(b"content").unwrap();
        writer.finish().unwrap();
        buf
    }

    #[test]
    fn test_zip_provider_lookup() {
        let data = create_mock_zip();
        let provider = ZipProvider::new(Cursor::new(data)).unwrap();

        // 1. Exact match
        assert!(provider.resolve_zip_name("OEBPS/image.png").is_some());

        // 2. Case-insensitive match
        assert_eq!(
            provider
                .resolve_zip_name("oebps/text/chapter1.xhtml")
                .unwrap(),
            "OEBPS/Text/Chapter1.xhtml"
        );

        // 3. Extension alias (jpg -> png)
        assert_eq!(
            provider.resolve_zip_name("OEBPS/image.jpg").unwrap(),
            "OEBPS/image.png"
        );

        // 4. Missing file
        assert!(provider.resolve_zip_name("missing.txt").is_none());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn test_safe_join_normal_paths() {
        let root = PathBuf::from("/epub/root");
        assert!(safe_join(&root, "META-INF/container.xml").is_ok());
        assert!(safe_join(&root, "OEBPS/content.opf").is_ok());
        assert!(safe_join(&root, "./OEBPS/text/ch1.xhtml").is_ok());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn test_safe_join_rejects_path_traversal() {
        let root = PathBuf::from("/epub/root");
        assert!(safe_join(&root, "../../etc/passwd").is_err());
        assert!(safe_join(&root, "OEBPS/../../../etc/hosts").is_err());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn test_safe_join_allows_internal_dotdot() {
        let root = PathBuf::from("/epub/root");
        let result = safe_join(&root, "OEBPS/Text/../Images/cover.jpg");
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            PathBuf::from("/epub/root/OEBPS/Images/cover.jpg")
        );
    }
}
