//! EPUB Font Deobfuscation & encryption metadata
//!
//! Provides:
//! - IDPF / Adobe **font** obfuscation (XOR header; reversible without a DRM key)
//! - Metadata for **full-content** encryption (AES-CBC / LCP): algorithm URI +
//!   `OriginalLength` for accurate reading positions
//!
//! Full AES/LCP content decryption requires a content key and is **not**
//! performed by this crate — resource APIs return ciphertext unchanged for
//! non-font encryption entries.

use sha1::{Digest, Sha1};
use std::io::{Read, Result};

/// Encryption / obfuscation algorithm recorded in `META-INF/encryption.xml`.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObfuscationAlgorithm {
    /// IDPF Font Obfuscation (`http://www.idpf.org/2008/embedding`)
    Idpf,
    /// Adobe Font Obfuscation (`http://ns.adobe.com/pdf/enc#RC`)
    Adobe,
    /// Full-content AES-CBC (e.g. W3C xmlenc AES-128/256, LCP).
    /// Not XOR-deobfuscated by this library.
    AesCbc,
    /// Recognized `EncryptionMethod` URI that is neither font obfuscation nor AES-CBC.
    Unknown,
}

impl ObfuscationAlgorithm {
    /// True for IDPF/Adobe font schemes that this crate can reverse.
    pub fn is_font_obfuscation(self) -> bool {
        matches!(self, Self::Idpf | Self::Adobe)
    }

    /// Parse an `EncryptionMethod Algorithm="..."` URI.
    pub fn from_algorithm_uri(uri: &str) -> Self {
        let u = uri.trim();
        if u == "http://www.idpf.org/2008/embedding" {
            Self::Idpf
        } else if u == "http://ns.adobe.com/pdf/enc#RC" {
            Self::Adobe
        } else if u.contains("aes256-cbc")
            || u.contains("aes128-cbc")
            || u.contains("xmlenc#aes")
            || u.ends_with("#aes256-cbc")
            || u.ends_with("#aes128-cbc")
        {
            Self::AesCbc
        } else {
            Self::Unknown
        }
    }
}

/// Full encryption metadata for a single entry listed in `META-INF/encryption.xml`.
///
/// Stored in [`crate::model::EpubBook::encryptions`] keyed by the ZIP-relative
/// path of the encrypted resource.
///
/// Mirrors go-toolkit's `manifest.Encryption` struct, which is the authoritative
/// reference for how `OriginalLength` flows from `encryption.xml` into position
/// computation via the `OriginalLength` reflowable strategy.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct EncryptionInfo {
    /// The obfuscation or encryption algorithm applied to this entry.
    pub algorithm: ObfuscationAlgorithm,

    /// Original **plaintext** byte length before encryption (and before any
    /// pre-encryption compression), as declared by the
    /// `<comp:Compression OriginalLength="N">` element inside
    /// `<EncryptionProperties>` in `encryption.xml`.
    ///
    /// - `None` — element absent; typical for IDPF/Adobe *font* obfuscation,
    ///   which only XORs the header and does not change the file size.
    /// - `Some(n)` — present for LCP / AES-CBC full-content encryption where
    ///   AES padding and the IV inflate the stored cipher-text beyond the
    ///   original content length.
    ///
    /// Used by the [`crate::parser::OriginalLength`] reflowable strategy to
    /// compute accurate reading positions for encrypted EPUBs.
    pub original_length: Option<u64>,

    /// Raw `Algorithm` URI from `EncryptionMethod` (when known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub algorithm_uri: Option<String>,
}

impl EncryptionInfo {
    /// Font schemes that [`DeobfuscatingReader`] can reverse.
    pub fn font_obfuscation(&self) -> Option<ObfuscationAlgorithm> {
        if self.algorithm.is_font_obfuscation() {
            Some(self.algorithm)
        } else {
            None
        }
    }
}

/// Generates a 20-byte key for the IDPF font obfuscation algorithm based on the EPUB Unique Identifier.
pub fn generate_idpf_key(identifier: &str) -> Vec<u8> {
    // Strip all whitespace characters from the identifier
    let stripped: String = identifier.chars().filter(|c| !c.is_whitespace()).collect();
    let mut hasher = Sha1::new();
    hasher.update(stripped.as_bytes());
    hasher.finalize().to_vec()
}

/// Generates a 16-byte key for the Adobe font obfuscation algorithm based on the EPUB Unique Identifier.
pub fn generate_adobe_key(identifier: &str) -> Vec<u8> {
    // Strip "urn:uuid:" prefix and dashes "-"
    let stripped = identifier.replace("urn:uuid:", "").replace("-", "");

    let mut key = Vec::new();
    if stripped.len() == 32 {
        for i in (0..32).step_by(2) {
            if let Ok(byte) = u8::from_str_radix(&stripped[i..i + 2], 16) {
                key.push(byte);
            }
        }
    }

    // Fallback if parsing fails, though it shouldn't for a valid UUID
    if key.len() != 16 {
        key = vec![0; 16];
    }

    key
}

/// A wrapper around a `Read` stream that transparently deobfuscates the first 1024 or 1040 bytes
/// of the stream using the provided **font** obfuscation algorithm and key.
///
/// Only [`ObfuscationAlgorithm::Idpf`] and [`ObfuscationAlgorithm::Adobe`] are valid.
/// Passing AES/unknown algorithms yields a zero-key no-op (prefer not calling this path).
pub struct DeobfuscatingReader<'a> {
    inner: Box<dyn Read + 'a>,
    key: Vec<u8>,
    obfuscation_length: usize,
    current_offset: usize,
}

impl<'a> DeobfuscatingReader<'a> {
    /// Creates a new `DeobfuscatingReader` for a font obfuscation algorithm.
    pub fn new(
        inner: Box<dyn Read + 'a>,
        identifier: &str,
        algorithm: ObfuscationAlgorithm,
    ) -> Self {
        let (key, obfuscation_length) = match algorithm {
            ObfuscationAlgorithm::Idpf => (generate_idpf_key(identifier), 1040),
            ObfuscationAlgorithm::Adobe => (generate_adobe_key(identifier), 1024),
            // Identity: should not be used for content encryption.
            ObfuscationAlgorithm::AesCbc | ObfuscationAlgorithm::Unknown => (Vec::new(), 0),
        };

        Self {
            inner,
            key,
            obfuscation_length,
            current_offset: 0,
        }
    }
}

impl<'a> Read for DeobfuscatingReader<'a> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let bytes_read = self.inner.read(buf)?;
        if bytes_read == 0 {
            return Ok(0);
        }

        // Apply XOR mask to the obfuscated portion of the stream
        if self.current_offset < self.obfuscation_length {
            let key_len = self.key.len();
            if key_len > 0 {
                for (i, byte) in buf.iter_mut().enumerate().take(bytes_read) {
                    let pos = self.current_offset + i;
                    if pos < self.obfuscation_length {
                        *byte ^= self.key[pos % key_len];
                    } else {
                        break;
                    }
                }
            }
        }

        self.current_offset += bytes_read;
        Ok(bytes_read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_idpf_key_generation() {
        let key = generate_idpf_key(" urn:uuid:550e8400-e29b-41d4-a716-446655440000 ");
        assert_eq!(key.len(), 20);
        // Should match the sha1 hash of "urn:uuid:550e8400-e29b-41d4-a716-446655440000"
        let expected_hash = Sha1::digest(b"urn:uuid:550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(key, expected_hash.to_vec());
    }

    #[test]
    fn test_adobe_key_generation() {
        let key = generate_adobe_key("urn:uuid:550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(key.len(), 16);
        assert_eq!(
            key,
            vec![
                0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44,
                0x00, 0x00
            ]
        );
    }

    #[test]
    fn test_deobfuscating_reader() {
        let identifier = "test-id";
        let key = generate_idpf_key(identifier);

        // Create 2000 bytes of data
        let original_data = vec![42u8; 2000];

        // Obfuscate the first 1040 bytes
        let mut obfuscated_data = original_data.clone();
        for i in 0..1040 {
            obfuscated_data[i] ^= key[i % 20];
        }

        let cursor = Cursor::new(obfuscated_data);
        let mut reader =
            DeobfuscatingReader::new(Box::new(cursor), identifier, ObfuscationAlgorithm::Idpf);

        let mut output = Vec::new();
        reader.read_to_end(&mut output).unwrap();

        assert_eq!(output.len(), 2000);
        assert_eq!(output, original_data);
    }

    #[test]
    fn from_algorithm_uri_table() {
        assert_eq!(
            ObfuscationAlgorithm::from_algorithm_uri("http://www.idpf.org/2008/embedding"),
            ObfuscationAlgorithm::Idpf
        );
        assert_eq!(
            ObfuscationAlgorithm::from_algorithm_uri("http://ns.adobe.com/pdf/enc#RC"),
            ObfuscationAlgorithm::Adobe
        );
        assert_eq!(
            ObfuscationAlgorithm::from_algorithm_uri("http://www.w3.org/2001/04/xmlenc#aes256-cbc"),
            ObfuscationAlgorithm::AesCbc
        );
        assert_eq!(
            ObfuscationAlgorithm::from_algorithm_uri("http://www.w3.org/2001/04/xmlenc#aes128-cbc"),
            ObfuscationAlgorithm::AesCbc
        );
        assert_eq!(
            ObfuscationAlgorithm::from_algorithm_uri("http://example.com/custom"),
            ObfuscationAlgorithm::Unknown
        );
        assert!(ObfuscationAlgorithm::Idpf.is_font_obfuscation());
        assert!(!ObfuscationAlgorithm::AesCbc.is_font_obfuscation());
    }

    #[test]
    fn encryption_info_font_obfuscation_helper() {
        let font = EncryptionInfo {
            algorithm: ObfuscationAlgorithm::Adobe,
            original_length: None,
            algorithm_uri: None,
        };
        assert_eq!(font.font_obfuscation(), Some(ObfuscationAlgorithm::Adobe));
        let aes = EncryptionInfo {
            algorithm: ObfuscationAlgorithm::AesCbc,
            original_length: Some(100),
            algorithm_uri: Some("http://www.w3.org/2001/04/xmlenc#aes256-cbc".into()),
        };
        assert!(aes.font_obfuscation().is_none());
    }
}
