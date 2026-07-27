use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

/// BLAKE3 digest of a payload. Doubles as the key in the content-addressed blob
/// store and as the dedup / loop-guard key in the sync engine.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// Hash of a multi-format clip: the digest covers every representation so
    /// that "copy from Word" (text + HTML + RTF) is one identity, not three.
    pub fn of_parts(parts: &[(&str, &[u8])]) -> Self {
        let mut hasher = blake3::Hasher::new();
        // Length-prefix everything; without it ("ab", "c") and ("a", "bc")
        // would collide.
        for (label, bytes) in parts {
            hasher.update(&(label.len() as u64).to_le_bytes());
            hasher.update(label.as_bytes());
            hasher.update(&(bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        }
        Self(*hasher.finalize().as_bytes())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Blob store layout: `blobs/ab/cd/abcdef...` keeps directories small on
    /// filesystems that degrade with tens of thousands of entries.
    pub fn shard_path(&self) -> (String, String, String) {
        let hex = self.to_hex();
        (hex[0..2].to_string(), hex[2..4].to_string(), hex)
    }
}

impl FromStr for ContentHash {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let raw = hex::decode(s).map_err(|_| Error::InvalidHash(s.to_string()))?;
        let bytes: [u8; 32] = raw
            .try_into()
            .map_err(|_| Error::InvalidHash(s.to_string()))?;
        Ok(Self(bytes))
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({}…)", &self.to_hex()[..12])
    }
}

impl Serialize for ContentHash {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_bytes_hash_equal() {
        assert_eq!(ContentHash::of(b"hello"), ContentHash::of(b"hello"));
        assert_ne!(ContentHash::of(b"hello"), ContentHash::of(b"hell0"));
    }

    #[test]
    fn parts_are_length_prefixed() {
        let a = ContentHash::of_parts(&[("text", b"ab"), ("html", b"c")]);
        let b = ContentHash::of_parts(&[("text", b"a"), ("html", b"bc")]);
        assert_ne!(a, b, "concatenation collision — prefixing is broken");
    }

    #[test]
    fn part_order_matters() {
        let a = ContentHash::of_parts(&[("text", b"x"), ("html", b"y")]);
        let b = ContentHash::of_parts(&[("html", b"y"), ("text", b"x")]);
        assert_ne!(a, b, "callers must supply parts in a canonical order");
    }

    #[test]
    fn hex_roundtrip() {
        let h = ContentHash::of(b"clipse");
        assert_eq!(h.to_hex().parse::<ContentHash>().unwrap(), h);
        assert!("nothex".parse::<ContentHash>().is_err());
        assert!("aabb".parse::<ContentHash>().is_err(), "wrong length");
    }

    #[test]
    fn shard_path_splits_prefix() {
        let h = ContentHash::of(b"clipse");
        let (a, b, full) = h.shard_path();
        assert_eq!(a, full[0..2]);
        assert_eq!(b, full[2..4]);
        assert_eq!(full.len(), 64);
    }
}
