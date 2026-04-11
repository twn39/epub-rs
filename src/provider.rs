//! EPUB Archive Providers (Storage Layer)

use std::fs::File;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
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
pub struct DirProvider {
    root: PathBuf,
}

impl DirProvider {
    pub fn new<P: AsRef<Path>>(root: P) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }
}

impl EpubProvider for DirProvider {
    fn read_file<'a>(&'a mut self, path: &str) -> Result<Box<dyn Read + 'a>, EpubError> {
        let full_path = self.root.join(path);
        let file = File::open(&full_path).map_err(|e| EpubError::Io(e))?;
        Ok(Box::new(file))
    }
}
