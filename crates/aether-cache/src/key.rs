use std::error::Error;
use std::fmt::{Display, Formatter};

use sha2::{Digest, Sha256};

/// Maximum byte length of a cache-key domain identifier.
pub const CACHE_KEY_DOMAIN_MAX_BYTES: usize = 64;

const CACHE_KEY_PREFIX: &[u8] = b"aetherstack-cache-key-v1\0";

/// Lowercase SHA-256 key for one canonical processing operation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CacheKey(String);

impl CacheKey {
    /// Derives a key from a domain and its exact canonical descriptor bytes.
    ///
    /// Domain separation and explicit byte lengths prevent two independent
    /// operation schemas or concatenation choices from sharing a key. Callers
    /// own canonicalization of the descriptor and must version scientific
    /// parameters inside it.
    ///
    /// # Errors
    ///
    /// Returns a typed error for an invalid domain or a descriptor length that
    /// cannot be represented by the stable key encoding.
    pub fn derive(domain: &str, canonical_descriptor: &[u8]) -> Result<Self, CacheKeyError> {
        validate_domain(domain)?;
        let descriptor_length = u64::try_from(canonical_descriptor.len())
            .map_err(|_| CacheKeyError::DescriptorTooLarge)?;
        let domain_length =
            u64::try_from(domain.len()).map_err(|_| CacheKeyError::InvalidDomain)?;

        let mut hasher = Sha256::new();
        hasher.update(CACHE_KEY_PREFIX);
        hasher.update(domain_length.to_be_bytes());
        hasher.update(domain.as_bytes());
        hasher.update(descriptor_length.to_be_bytes());
        hasher.update(canonical_descriptor);
        Ok(Self(encode_lower_hex(&hasher.finalize())))
    }

    /// Parses an existing lowercase SHA-256 key.
    ///
    /// # Errors
    ///
    /// Returns [`CacheKeyParseError`] unless the input contains exactly 64
    /// lowercase hexadecimal digits.
    pub fn from_sha256_hex(value: impl Into<String>) -> Result<Self, CacheKeyParseError> {
        let value = value.into();
        if !is_lower_sha256(&value) {
            return Err(CacheKeyParseError);
        }
        Ok(Self(value))
    }

    /// Canonical lowercase hexadecimal representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for CacheKey {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Failure while deriving a canonical operation key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheKeyError {
    /// Domain is empty, oversized, or outside the portable character set.
    InvalidDomain,
    /// Descriptor length cannot be represented by the versioned encoding.
    DescriptorTooLarge,
}

impl Display for CacheKeyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDomain => formatter.write_str("cache-key domain is not canonical"),
            Self::DescriptorTooLarge => {
                formatter.write_str("cache-key descriptor length exceeds 64-bit encoding")
            }
        }
    }
}

impl Error for CacheKeyError {}

/// Failure to parse a lowercase SHA-256 cache key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheKeyParseError;

impl Display for CacheKeyParseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("cache key must be 64 lowercase hexadecimal digits")
    }
}

impl Error for CacheKeyParseError {}

fn validate_domain(domain: &str) -> Result<(), CacheKeyError> {
    let bytes = domain.as_bytes();
    if bytes.is_empty()
        || bytes.len() > CACHE_KEY_DOMAIN_MAX_BYTES
        || !bytes[0].is_ascii_lowercase()
        || !bytes.iter().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(*byte, b'.' | b'_' | b'-')
        })
    {
        return Err(CacheKeyError::InvalidDomain);
    }
    Ok(())
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(crate) fn encode_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(*byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(*byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_stable_domain_separated_keys() -> Result<(), Box<dyn Error>> {
        let first = CacheKey::derive("calibrated-tile-v1", b"same descriptor")?;
        let repeated = CacheKey::derive("calibrated-tile-v1", b"same descriptor")?;
        let other_domain = CacheKey::derive("integrated-tile-v1", b"same descriptor")?;
        let other_descriptor = CacheKey::derive("calibrated-tile-v1", b"other descriptor")?;

        assert_eq!(first, repeated);
        assert_ne!(first, other_domain);
        assert_ne!(first, other_descriptor);
        assert_eq!(first.as_str().len(), 64);
        assert!(first.as_str().bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(CacheKey::from_sha256_hex(first.to_string())?, first);
        Ok(())
    }

    #[test]
    fn rejects_noncanonical_domains_and_external_keys() {
        for domain in ["", "Upper", "-prefix", "bad/domain"] {
            assert_eq!(
                CacheKey::derive(domain, b"descriptor"),
                Err(CacheKeyError::InvalidDomain)
            );
        }
        assert_eq!(
            CacheKey::derive(&"a".repeat(CACHE_KEY_DOMAIN_MAX_BYTES + 1), b"descriptor"),
            Err(CacheKeyError::InvalidDomain)
        );
        for value in ["a", &"A".repeat(64), &"g".repeat(64)] {
            assert_eq!(CacheKey::from_sha256_hex(value), Err(CacheKeyParseError));
        }
    }
}
