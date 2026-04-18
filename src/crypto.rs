//! EPUB Font Deobfuscation
//!
//! Provides support for decrypting obfuscated fonts using IDPF and Adobe algorithms
//! as defined in the EPUB standard and `META-INF/encryption.xml`.

use sha1::{Digest, Sha1};
use std::io::{Read, Result};

/// Recognized obfuscation algorithms for EPUB resources.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObfuscationAlgorithm {
    /// IDPF Font Obfuscation algorithm (`http://www.idpf.org/2008/embedding`)
    Idpf,
    /// Adobe Font Obfuscation algorithm (`http://ns.adobe.com/pdf/enc#RC`)
    Adobe,
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
/// of the stream using the provided obfuscation algorithm and key.
pub struct DeobfuscatingReader<'a> {
    inner: Box<dyn Read + 'a>,
    key: Vec<u8>,
    obfuscation_length: usize,
    current_offset: usize,
}

impl<'a> DeobfuscatingReader<'a> {
    /// Creates a new `DeobfuscatingReader`.
    pub fn new(
        inner: Box<dyn Read + 'a>,
        identifier: &str,
        algorithm: ObfuscationAlgorithm,
    ) -> Self {
        let (key, obfuscation_length) = match algorithm {
            ObfuscationAlgorithm::Idpf => (generate_idpf_key(identifier), 1040),
            ObfuscationAlgorithm::Adobe => (generate_adobe_key(identifier), 1024),
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
}
