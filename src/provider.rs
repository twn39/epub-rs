//! EPUB Archive Providers (Storage Layer)

use std::collections::HashMap;
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

    /// Returns the **uncompressed** byte size of the entry at `path`.
    ///
    /// # Semantics
    ///
    /// This always returns the *plaintext* size — i.e. the number of bytes the
    /// file occupies **after** decompression but **before** any EPUB-level
    /// encryption is removed.  This matches what `zip::ZipFile::size()` returns
    /// and what the Readium `ArchiveEntryLength` position strategy expects:
    ///
    /// | Entry state | What `entry_length` returns |
    /// |-------------|----------------------------|
    /// | Stored (no compression) | raw file size |
    /// | Deflate-compressed | **decompressed** size |
    /// | AES-encrypted (LCP) | cipher-text size (uncompressed) |
    ///
    /// For encrypted EPUB resources the [`crate::parser::OriginalLength`] strategy
    /// should be used instead, which reads the `OriginalLength` attribute from
    /// `META-INF/encryption.xml` to obtain the true plaintext byte count.
    fn entry_length(&mut self, path: &str) -> Result<u64, EpubError>;
}

/// A provider that reads EPUB files from a standard ZIP archive.
pub struct ZipProvider<R: Read + Seek> {
    archive: ZipArchive<R>,

    /// Lazy fallback cache: logical path → resolved ZIP entry name.
    ///
    /// Populated **only** when a case-insensitive or extension-alias fallback
    /// is triggered (i.e., the exact-match fast path missed). For well-formed
    /// EPUBs this map stays permanently empty — zero memory overhead,
    /// zero construction cost.
    ///
    /// Once a broken path has been resolved via the O(n) scan, the result is
    /// stored here so all subsequent accesses to the same path are O(1).
    fallback_cache: HashMap<String, String>,
}

/// Image file extensions that are considered interchangeable when a path can't be found.
/// Some EPUBs are incorrectly authored with mismatched extensions (e.g. referencing `.jpg`
/// when the actual file is stored as `.png`). We perform a best-effort alias lookup to
/// recover from this class of EPUB authoring errors.
const IMAGE_EXT_ALIASES: &[&[&str]] = &[&["jpg", "jpeg", "png", "gif", "webp", "bmp"]];

impl<R: Read + Seek> ZipProvider<R> {
    pub fn new(reader: R) -> Result<Self, EpubError> {
        let archive = ZipArchive::new(reader)?;
        Ok(Self {
            archive,
            fallback_cache: HashMap::new(),
        })
    }

    /// Resolve a logical path to a real entry name that exists in the archive.
    ///
    /// # Lookup order and complexity
    ///
    /// 1. **Exact match** — O(1) via the zip crate's internal `IndexMap`.
    ///    This covers 99%+ of calls for well-formed EPUBs; the function returns
    ///    immediately with no heap allocation.
    ///
    /// 2. **Fallback cache** — O(1) `HashMap` lookup. Populated lazily on the
    ///    first time a broken path is resolved. Subsequent accesses to the same
    ///    broken path skip the O(n) scan.
    ///
    /// 3. **Linear scan** — O(n) over all ZIP entry names, runs **at most once
    ///    per unique broken path**. Handles both case-insensitive matching and
    ///    image-extension aliases (`.jpg` ↔ `.png` etc.).
    ///
    /// Returns `None` if no matching entry can be found.
    fn resolve_zip_name(&mut self, path: &str) -> Option<String> {
        // 1. Exact match — O(1). Fast path for all well-formed EPUBs.
        if self.archive.index_for_name(path).is_some() {
            return Some(path.to_string());
        }

        // 2. Fallback cache — O(1). Avoids re-scanning for previously resolved
        //    broken paths (e.g. a misnamed cover image accessed many times).
        if let Some(cached) = self.fallback_cache.get(path) {
            return Some(cached.clone());
        }

        // 3. Linear scan — O(n). Runs at most once per unique broken path.
        //    Pass &self.archive separately so we can borrow &mut self.fallback_cache
        //    afterwards (disjoint field borrows, allowed by Rust NLL).
        let resolved = Self::scan_fallback(&self.archive, path)?;
        self.fallback_cache
            .insert(path.to_string(), resolved.clone());
        Some(resolved)
    }

    /// O(n) linear scan for case-insensitive and extension-alias fallbacks.
    ///
    /// Accepts `&ZipArchive<R>` instead of `&self` so the caller can mutably
    /// borrow `self.fallback_cache` in the same scope (disjoint field borrow).
    fn scan_fallback(archive: &ZipArchive<R>, path: &str) -> Option<String> {
        let path_lower = path.to_lowercase();
        let (stem_lower, ext_lower) = split_stem_ext(&path_lower);

        // Determine the alias group for this extension (if any).
        let alias_group: Option<&[&str]> = IMAGE_EXT_ALIASES
            .iter()
            .find(|group| group.contains(&ext_lower))
            .copied();

        // Collect all entry names into an owned Vec to avoid holding an immutable
        // borrow on `archive` when the caller later calls `archive.by_name()` mutably.
        let names: Vec<String> = archive.file_names().map(|s| s.to_string()).collect();

        names.into_iter().find_map(|name| {
            let name_lower = name.to_lowercase();

            // Case-insensitive exact path match.
            if name_lower == path_lower {
                return Some(name);
            }

            // Case-insensitive match with extension alias.
            if let Some(aliases) = alias_group {
                let (name_stem, name_ext) = split_stem_ext(&name_lower);
                if name_stem == stem_lower && aliases.contains(&name_ext) {
                    return Some(name);
                }
            }

            None
        })
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

    fn entry_length(&mut self, path: &str) -> Result<u64, EpubError> {
        let resolved = self
            .resolve_zip_name(path)
            .ok_or(EpubError::Zip(zip::result::ZipError::FileNotFound))?;
        // `ZipFile::size()` returns the *uncompressed* byte size of the entry.
        // This matches the Readium `ArchiveEntryLength` strategy expectation:
        // the position count is based on the content length, not the compressed storage size.
        Ok(self.archive.by_name(&resolved)?.size())
    }
}

// ── Utility ───────────────────────────────────────────────────────────────────

/// Split a **lowercase** path into `(stem, extension)` where `extension` is the
/// part after the last `.`, or `""` if no dot is present.
///
/// Both slices reference the original string to avoid extra allocations.
#[inline]
fn split_stem_ext(lower_path: &str) -> (&str, &str) {
    match lower_path.rfind('.') {
        Some(dot) => (&lower_path[..dot], &lower_path[dot + 1..]),
        None => (lower_path, ""),
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

    fn entry_length(&mut self, path: &str) -> Result<u64, EpubError> {
        let full_path = safe_join(&self.root, path)?;
        Ok(std::fs::metadata(&full_path).map_err(EpubError::Io)?.len())
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
        let mut provider = ZipProvider::new(Cursor::new(data)).unwrap();

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

    #[test]
    fn exact_match_does_not_populate_fallback_cache() {
        // For well-formed EPUBs the fallback_cache must stay empty.
        let data = create_mock_zip();
        let mut provider = ZipProvider::new(Cursor::new(data)).unwrap();

        // Exact-match paths must not touch the fallback_cache.
        provider.resolve_zip_name("OEBPS/image.png");
        provider.resolve_zip_name("OEBPS/Text/Chapter1.xhtml");

        assert!(
            provider.fallback_cache.is_empty(),
            "fallback_cache must stay empty when all lookups are exact matches"
        );
    }

    #[test]
    fn fallback_cache_populated_after_first_broken_path() {
        // A case-mismatched path triggers the O(n) scan exactly once, then
        // the result is stored in fallback_cache for O(1) subsequent access.
        let data = create_mock_zip();
        let mut provider = ZipProvider::new(Cursor::new(data)).unwrap();

        assert!(provider.fallback_cache.is_empty(), "cache must start empty");

        // First access: triggers O(n) scan, caches result.
        let resolved = provider.resolve_zip_name("oebps/text/chapter1.xhtml");
        assert_eq!(resolved.as_deref(), Some("OEBPS/Text/Chapter1.xhtml"));
        assert_eq!(
            provider.fallback_cache.len(),
            1,
            "cache must contain exactly one entry after first broken-path access"
        );

        // Second access: hits the cache, no re-scan.
        let resolved2 = provider.resolve_zip_name("oebps/text/chapter1.xhtml");
        assert_eq!(resolved2.as_deref(), Some("OEBPS/Text/Chapter1.xhtml"));
        assert_eq!(
            provider.fallback_cache.len(),
            1,
            "cache size must not grow on repeated access to the same broken path"
        );
    }

    #[test]
    fn fallback_cache_extension_alias_cached() {
        // Extension-alias fallback is also cached after first resolution.
        let data = create_mock_zip();
        let mut provider = ZipProvider::new(Cursor::new(data)).unwrap();

        // First access via alias: scan + cache.
        let r1 = provider.resolve_zip_name("OEBPS/image.jpg");
        assert_eq!(r1.as_deref(), Some("OEBPS/image.png"));
        assert_eq!(provider.fallback_cache.len(), 1);

        // Second access: O(1) cache hit, result identical.
        let r2 = provider.resolve_zip_name("OEBPS/image.jpg");
        assert_eq!(r2, r1, "cached alias result must equal initial scan result");
        assert_eq!(
            provider.fallback_cache.len(),
            1,
            "cache must not grow on repeated alias lookup"
        );
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
