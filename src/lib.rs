pub mod cfi;
pub mod crypto;
pub mod error;
pub mod generator;
pub mod model;
pub mod parser;
pub mod processor;
pub mod provider;

pub use cfi::EpubCfi;
pub use error::EpubError;
pub use model::EpubBook;
pub use model::{MediaOverlayMetadata, SmilDocument, SmilObject};
pub use processor::rewrite_css;

#[cfg(target_arch = "wasm32")]
pub mod wasm;

/// C FFI layer — enabled when the `ffi` feature is active (native targets only).
#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
pub mod ffi;
