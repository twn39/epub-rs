//! `META-INF/encryption.xml` parsing.
//!
//! Extracted from OPF package parsing so format kernels stay focused.

use crate::crypto::{EncryptionInfo, ObfuscationAlgorithm};
use crate::error::EpubError;
use crate::provider::EpubProvider;
use quick_xml::Reader;
use quick_xml::events::Event;
use std::collections::HashMap;
use std::io::Read;

use super::EpubArchive;

impl<P: EpubProvider> EpubArchive<P> {
    /// Reads `META-INF/encryption.xml` to find obfuscated/encrypted resources.
    ///
    /// Returns a map from ZIP-relative path to [`EncryptionInfo`], which carries
    /// the algorithm (font obfuscation or content AES), optional raw URI, and
    /// optional original plaintext length from `<Compression OriginalLength="N">`.
    ///
    /// Entries with only `OriginalLength` (LCP) are stored even when the
    /// algorithm is not a font scheme — required for accurate position counts.
    pub(super) fn parse_encryption(
        &mut self,
    ) -> Result<HashMap<String, EncryptionInfo>, EpubError> {
        let mut encryptions = HashMap::new();

        let mut enc_file = match self.provider.read_file("META-INF/encryption.xml") {
            Ok(f) => f,
            Err(_) => return Ok(encryptions),
        };

        let mut buf = String::new();
        if enc_file.read_to_string(&mut buf).is_err() {
            return Ok(encryptions);
        }

        let mut reader = Reader::from_str(&buf);
        reader.config_mut().trim_text(true);

        let mut current_algo: Option<ObfuscationAlgorithm> = None;
        let mut current_algo_uri: Option<String> = None;
        let mut current_original_length: Option<u64> = None;
        let mut current_uri: Option<String> = None;
        let mut event_buf = Vec::new();

        loop {
            match reader.read_event_into(&mut event_buf) {
                Ok(Event::Empty(ref e)) | Ok(Event::Start(ref e)) => {
                    let name = String::from_utf8_lossy(e.name().into_inner()).into_owned();

                    if name.ends_with("EncryptionMethod") {
                        for attr in e.attributes() {
                            if let Ok(attr) = attr
                                && attr.key.as_ref() == b"Algorithm"
                            {
                                let val = String::from_utf8_lossy(&attr.value).into_owned();
                                current_algo_uri = Some(val.clone());
                                current_algo = Some(ObfuscationAlgorithm::from_algorithm_uri(&val));
                            }
                        }
                    } else if name.ends_with("CipherReference") {
                        for attr in e.attributes() {
                            if let Ok(attr) = attr
                                && attr.key.as_ref() == b"URI"
                            {
                                let uri = String::from_utf8_lossy(&attr.value).into_owned();
                                let decoded_uri = percent_encoding::percent_decode_str(&uri)
                                    .decode_utf8_lossy()
                                    .into_owned();
                                current_uri = Some(decoded_uri);
                            }
                        }
                    } else if name.ends_with("Compression") {
                        for attr in e.attributes() {
                            if let Ok(attr) = attr
                                && attr.key.as_ref() == b"OriginalLength"
                            {
                                let val = String::from_utf8_lossy(&attr.value);
                                current_original_length = val.parse::<u64>().ok();
                            }
                        }
                    }
                }
                Ok(Event::End(ref e)) => {
                    let name = String::from_utf8_lossy(e.name().into_inner()).into_owned();
                    if name.ends_with("EncryptedData") {
                        if let Some(uri) = current_uri.take() {
                            let algo = current_algo.unwrap_or(ObfuscationAlgorithm::Unknown);
                            if current_algo.is_some() || current_original_length.is_some() {
                                encryptions.insert(
                                    uri,
                                    EncryptionInfo {
                                        algorithm: algo,
                                        original_length: current_original_length,
                                        algorithm_uri: current_algo_uri.take(),
                                    },
                                );
                            }
                        }
                        current_algo = None;
                        current_algo_uri = None;
                        current_original_length = None;
                    }
                }
                Ok(Event::Eof) => break,
                Err(_) => break,
                _ => {}
            }
            event_buf.clear();
        }

        Ok(encryptions)
    }
}
