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

impl<R: Read + Seek> ZipProvider<R> {
    pub fn new(reader: R) -> Result<Self, EpubError> {
        let archive = ZipArchive::new(reader)?;
        Ok(Self { archive })
    }
}

impl<R: Read + Seek> EpubProvider for ZipProvider<R> {
    fn read_file<'a>(&'a mut self, path: &str) -> Result<Box<dyn Read + 'a>, EpubError> {
        let file = self.archive.by_name(path)?;
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
            Component::ParentDir => {
                // Pop one level; if we can no longer stay inside root, reject.
                if !resolved.pop() || !resolved.starts_with(root) {
                    return Err(EpubError::InvalidFormat(format!(
                        "Security: path '{}' attempts to escape the EPUB root directory",
                        path
                    )));
                }
            }
            Component::Normal(c) => resolved.push(c),
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
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::*;

    #[test]
    fn test_safe_join_normal_paths() {
        let root = PathBuf::from("/epub/root");
        assert!(safe_join(&root, "META-INF/container.xml").is_ok());
        assert!(safe_join(&root, "OEBPS/content.opf").is_ok());
        assert!(safe_join(&root, "./OEBPS/text/ch1.xhtml").is_ok());
    }

    #[test]
    fn test_safe_join_rejects_path_traversal() {
        let root = PathBuf::from("/epub/root");
        // Direct traversal
        assert!(safe_join(&root, "../../etc/passwd").is_err());
        // Traversal after a valid segment
        assert!(safe_join(&root, "OEBPS/../../../etc/hosts").is_err());
    }

    #[test]
    fn test_safe_join_allows_internal_dotdot() {
        // A path like "a/../b" that stays inside root should be allowed.
        let root = PathBuf::from("/epub/root");
        let result = safe_join(&root, "OEBPS/Text/../Images/cover.jpg");
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            PathBuf::from("/epub/root/OEBPS/Images/cover.jpg")
        );
    }
}
