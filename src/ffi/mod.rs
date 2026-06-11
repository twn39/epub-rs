//! C FFI layer for epub-rs.
//!
//! Exposes an opaque-handle, JSON-centric C API so that any language with a
//! C FFI bridge (Swift, Python/ctypes, Go/cgo, Kotlin/JNI, etc.) can use the
//! library without Rust toolchain knowledge.

pub mod common;
pub mod parser;
pub mod generator;
pub mod cfi;
pub mod crypto;

#[cfg(test)]
mod tests;
