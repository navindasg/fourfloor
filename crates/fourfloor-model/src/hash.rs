//! Content hashes for the audio pool and plugin state blobs.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Number of hex characters in a [`ContentHash`].
pub const HASH_HEX_LEN: usize = 64;

/// A BLAKE3 hash written as 64 lowercase hex characters.
///
/// Because the value is checked on construction and on deserialize, it is
/// always safe to use as a file name.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct ContentHash(String);

/// The text is not 64 hex characters.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("content hash must be {HASH_HEX_LEN} hex characters")]
pub struct InvalidHash;

impl ContentHash {
    /// Parses a hex string, lowercasing it.
    pub fn parse(text: &str) -> Result<Self, InvalidHash> {
        let well_formed = text.len() == HASH_HEX_LEN && text.bytes().all(|b| b.is_ascii_hexdigit());
        if well_formed {
            Ok(Self(text.to_ascii_lowercase()))
        } else {
            Err(InvalidHash)
        }
    }

    /// Hashes a byte slice.
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self::from_blake3(&blake3::hash(bytes))
    }

    pub(crate) fn from_blake3(hash: &blake3::Hash) -> Self {
        Self(hash.to_hex().as_str().to_owned())
    }

    /// The hash as lowercase hex.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for ContentHash {
    type Error = InvalidHash;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text)
    }
}

impl From<ContentHash> for String {
    fn from(hash: ContentHash) -> Self {
        hash.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashing_is_stable_and_hex() {
        let hash = ContentHash::of_bytes(b"kick");
        assert_eq!(hash, ContentHash::of_bytes(b"kick"));
        assert_ne!(hash, ContentHash::of_bytes(b"hat"));
        assert_eq!(hash.as_str().len(), HASH_HEX_LEN);
        assert_eq!(ContentHash::parse(hash.as_str()), Ok(hash));
    }

    #[test]
    fn parse_rejects_bad_text_and_lowercases() {
        assert_eq!(ContentHash::parse("abc"), Err(InvalidHash));
        assert_eq!(ContentHash::parse(&"g".repeat(64)), Err(InvalidHash));
        assert_eq!(ContentHash::parse(&"../".repeat(22)), Err(InvalidHash));
        let upper = "AB".repeat(32);
        assert_eq!(
            ContentHash::parse(&upper).unwrap().as_str(),
            "ab".repeat(32)
        );
    }

    #[test]
    fn deserialize_validates() {
        let good = format!("\"{}\"", "0f".repeat(32));
        assert!(serde_json::from_str::<ContentHash>(&good).is_ok());
        assert!(serde_json::from_str::<ContentHash>("\"../../etc/passwd\"").is_err());
    }
}
