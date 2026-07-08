use std::{
    fs::File,
    io::{Read, Result as IoResult},
    path::Path,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{AppError, Result};

/// Number of bytes used by each streaming read while calculating a recovery
/// fingerprint.
///
/// Recovery records may persist the resulting digest, but they must never
/// persist the source content. Keeping this buffer size fixed and small makes
/// fingerprint calculation bounded by a constant amount of memory.
pub const STREAM_BUFFER_SIZE_BYTES: usize = 64 * 1024;

/// The content fingerprint algorithm recorded in durable recovery records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RecoveryFingerprintAlgorithm {
    /// SHA-256, serialized as lowercase hexadecimal in [`RecoveryFingerprint`].
    #[serde(rename = "sha256")]
    Sha256,
}

impl RecoveryFingerprintAlgorithm {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
        }
    }
}

/// A bounded-memory, content-only safety fingerprint for durable recovery.
///
/// This is intentionally separate from any future status hash cache. Recovery
/// uses it to decide whether an unfinished operation is still looking at the
/// same content it recorded before a crash. Routine status should continue to
/// compare streams directly unless/until a dedicated cache is designed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RecoveryFingerprint {
    algorithm: RecoveryFingerprintAlgorithm,
    digest_hex: String,
}

impl RecoveryFingerprint {
    /// The byte length of a SHA-256 digest.
    pub const SHA256_DIGEST_BYTES: usize = 32;

    /// The serialized lowercase-hex length of a SHA-256 digest.
    pub const SHA256_DIGEST_HEX_LEN: usize = Self::SHA256_DIGEST_BYTES * 2;

    /// Creates a validated SHA-256 recovery fingerprint from canonical
    /// lowercase hexadecimal.
    pub fn new_sha256_hex(digest_hex: impl Into<String>) -> Option<Self> {
        let digest_hex = digest_hex.into();
        is_canonical_sha256_hex(&digest_hex).then_some(Self {
            algorithm: RecoveryFingerprintAlgorithm::Sha256,
            digest_hex,
        })
    }

    /// Calculates the recovery fingerprint by streaming bytes from `reader`.
    pub fn from_reader(reader: impl Read) -> IoResult<Self> {
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; STREAM_BUFFER_SIZE_BYTES];
        let mut reader = reader;

        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }

        Ok(Self::from_sha256_bytes(&hasher.finalize()))
    }

    /// Calculates the recovery fingerprint for a file.
    pub fn from_file(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(|e| AppError::io(path, e))?;
        Self::from_reader(file).map_err(|e| AppError::io(path, e))
    }

    /// Returns `true` when the file currently has this exact fingerprint.
    pub fn matches_file(&self, path: &Path) -> Result<bool> {
        Self::from_file(path).map(|actual| actual == *self)
    }

    pub const fn algorithm(&self) -> RecoveryFingerprintAlgorithm {
        self.algorithm
    }

    pub fn digest_hex(&self) -> &str {
        &self.digest_hex
    }

    fn from_sha256_bytes(bytes: &[u8]) -> Self {
        debug_assert_eq!(bytes.len(), Self::SHA256_DIGEST_BYTES);
        Self {
            algorithm: RecoveryFingerprintAlgorithm::Sha256,
            digest_hex: encode_lower_hex(bytes),
        }
    }
}

#[derive(Serialize)]
struct RecoveryFingerprintRef<'a> {
    algorithm: RecoveryFingerprintAlgorithm,
    digest_hex: &'a str,
}

#[derive(Deserialize)]
struct RecoveryFingerprintRepr {
    algorithm: RecoveryFingerprintAlgorithm,
    digest_hex: String,
}

impl Serialize for RecoveryFingerprint {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        RecoveryFingerprintRef {
            algorithm: self.algorithm,
            digest_hex: &self.digest_hex,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RecoveryFingerprint {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let repr = RecoveryFingerprintRepr::deserialize(deserializer)?;
        match repr.algorithm {
            RecoveryFingerprintAlgorithm::Sha256 => Self::new_sha256_hex(repr.digest_hex)
                .ok_or_else(|| {
                    serde::de::Error::custom(
                        "sha256 recovery fingerprint must be 64 lowercase hex characters",
                    )
                }),
        }
    }
}

fn is_canonical_sha256_hex(value: &str) -> bool {
    value.len() == RecoveryFingerprint::SHA256_DIGEST_HEX_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn encode_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::{
        cmp,
        io::{Cursor, Read},
    };

    use serde_json::json;

    use super::*;

    const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn sha256_known_vector_is_stable() {
        let fingerprint = RecoveryFingerprint::from_reader(Cursor::new(b"abc")).unwrap();

        assert_eq!(
            fingerprint.algorithm(),
            RecoveryFingerprintAlgorithm::Sha256
        );
        assert_eq!(fingerprint.digest_hex(), SHA256_ABC);
    }

    #[test]
    fn serialized_shape_is_stable_and_contains_no_content() {
        let fingerprint = RecoveryFingerprint::from_reader(Cursor::new(b"secret-token")).unwrap();

        let serialized = serde_json::to_value(&fingerprint).unwrap();

        assert_eq!(
            serialized,
            json!({
                "algorithm": "sha256",
                "digest_hex": "930bbdc51b6aed5c2a5678fd6e28dee7a05e8a4b643cfc0b4427c3efb86c0d94"
            })
        );
        assert!(!serialized.to_string().contains("secret-token"));

        let round_tripped: RecoveryFingerprint = serde_json::from_value(serialized).unwrap();
        assert_eq!(round_tripped, fingerprint);
    }

    #[test]
    fn deserialization_rejects_non_canonical_digest_hex() {
        for digest_hex in [
            "",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015a",
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD",
            "ga7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ] {
            let value = json!({
                "algorithm": "sha256",
                "digest_hex": digest_hex,
            });

            assert!(serde_json::from_value::<RecoveryFingerprint>(value).is_err());
        }
    }

    #[test]
    fn deserialization_rejects_unknown_algorithm() {
        let value = json!({
            "algorithm": "sha512",
            "digest_hex": SHA256_ABC,
        });

        assert!(serde_json::from_value::<RecoveryFingerprint>(value).is_err());
    }

    #[test]
    fn fingerprint_changes_when_same_size_file_content_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("same-size.bin");

        std::fs::write(&path, b"content-A").unwrap();
        let before_len = std::fs::metadata(&path).unwrap().len();
        let before = RecoveryFingerprint::from_file(&path).unwrap();

        std::fs::write(&path, b"content-B").unwrap();
        let after_len = std::fs::metadata(&path).unwrap().len();
        let after = RecoveryFingerprint::from_file(&path).unwrap();

        assert_eq!(before_len, after_len);
        assert_ne!(before, after);
        assert!(before.matches_file(&path).is_ok_and(|matches| !matches));
        assert!(after.matches_file(&path).unwrap());
    }

    #[test]
    fn reader_chunking_does_not_change_digest() {
        let bytes = (0..(STREAM_BUFFER_SIZE_BYTES * 2 + 17))
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();

        let direct = RecoveryFingerprint::from_reader(Cursor::new(&bytes)).unwrap();
        let chunked = RecoveryFingerprint::from_reader(TinyChunks {
            inner: Cursor::new(&bytes),
            max_chunk: 3,
        })
        .unwrap();

        assert_eq!(chunked, direct);
    }

    struct TinyChunks<R> {
        inner: R,
        max_chunk: usize,
    }

    impl<R: Read> Read for TinyChunks<R> {
        fn read(&mut self, buffer: &mut [u8]) -> IoResult<usize> {
            let len = cmp::min(buffer.len(), self.max_chunk);
            self.inner.read(&mut buffer[..len])
        }
    }
}
