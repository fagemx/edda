//! Shared device-token utilities (generation + hashing).
//!
//! Both `edda-cli` (pairing commands) and `edda-serve` (auth middleware)
//! need to generate and hash device tokens. This module is the single
//! source of truth so the two never diverge.

use sha2::{Digest, Sha256};

/// Generate a device token: `edda_dev_<64-hex-chars>`.
///
/// Uses a CSPRNG (`rand::thread_rng`) for 32 bytes of randomness.
pub fn generate_device_token() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 32];
    rng.fill(&mut bytes);
    format!("edda_dev_{}", hex::encode(bytes))
}

/// Hash a raw token string with SHA-256 and return the hex digest.
pub fn hash_token(raw_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_token.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_format() {
        let tok = generate_device_token();
        assert!(
            tok.starts_with("edda_dev_"),
            "token should start with prefix"
        );
        // prefix (9 chars) + 64 hex chars = 73
        assert_eq!(tok.len(), 73, "token should be 73 chars");
    }

    #[test]
    fn tokens_are_unique() {
        let t1 = generate_device_token();
        let t2 = generate_device_token();
        assert_ne!(t1, t2, "two generated tokens must differ");
    }

    #[test]
    fn hash_round_trip() {
        let tok = generate_device_token();
        let h1 = hash_token(&tok);
        let h2 = hash_token(&tok);
        assert_eq!(h1, h2, "same input must produce same hash");
        // SHA-256 hex = 64 chars
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn different_tokens_different_hashes() {
        let t1 = generate_device_token();
        let t2 = generate_device_token();
        assert_ne!(hash_token(&t1), hash_token(&t2));
    }
}
