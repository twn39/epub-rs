use std::io::{Read, Seek};

pub trait EpubArchiveReader {
    fn read_file<'a>(&'a mut self, path: &str) -> Result<Box<dyn Read + 'a>, std::io::Error>;
}

pub struct ZipProvider<R: Read + Seek> {
    archive: zip::ZipArchive<R>,
}

impl<R: Read + Seek> EpubArchiveReader for ZipProvider<R> {
    fn read_file<'a>(&'a mut self, path: &str) -> Result<Box<dyn Read + 'a>, std::io::Error> {
        let file = self.archive.by_name(path)?;
        Ok(Box::new(file))
    }
}

fn main() {}
