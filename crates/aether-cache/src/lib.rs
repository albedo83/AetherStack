//! Verified immutable artifact storage for restartable processing.
//!
//! Cache keys identify canonical operations. Each stored artifact additionally
//! carries the exact payload length and SHA-256 digest, so lookup never treats a
//! partial, corrupt, or colliding file as a valid checkpoint.

mod key;
mod store;

pub use key::{CACHE_KEY_DOMAIN_MAX_BYTES, CacheKey, CacheKeyError, CacheKeyParseError};
pub use store::{
    ARTIFACT_FORMAT_VERSION, ArtifactDigest, ArtifactPublication, ArtifactPublicationState,
    ArtifactStore, CacheReadError, CacheWriteError, VerifiedArtifact,
};
