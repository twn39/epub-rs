//! C FFI layer for epub-rs.
//!
//! Exposes an opaque-handle, JSON-centric C API so that any language with a
//! C FFI bridge (Swift, Python/ctypes, Go/cgo, Kotlin/JNI, etc.) can use the
//! library without Rust toolchain knowledge.

pub mod cfi;
pub mod common;
pub mod crypto;
pub mod generator;
pub mod parser;
pub mod path;

#[cfg(test)]
mod tests;
