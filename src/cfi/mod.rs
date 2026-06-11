//! EPUB Canonical Fragment Identifier (CFI) implementation.
//!
//! CFI allows pinpointing a specific location within an EPUB document without modifying the underlying files.
//! Format Example: `epubcfi(/6/4[chap01ref]!/4[body01]/10[para05]/2/1:3)`
//! Range Example: `epubcfi(/6/4[chap01ref]!/4[body01]/10[para05],/2/1:1,/3:4)`

pub mod model;
pub mod parser;
pub mod resolver;

#[cfg(test)]
mod tests;

pub use model::{
    CfiPath, CfiResolution, CfiResolved, CfiSide, CfiStep, EpubCfi, NodeType, ResolvedStep,
    SpatialOffset,
};
