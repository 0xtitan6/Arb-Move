use anyhow::{Context, Result};
use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use base64::Engine as _;

/// Ed25519 transaction signer for Sui.
///
/// Sui uses a specific signature scheme:
/// - Flag byte: 0x00 for Ed25519
/// - 64-byte Ed25519 signature
/// - 32-byte public key
pub struct Signer {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
}

impl Signer {
    /// Create a signer from a private key string.
    /// Accepts:
    /// - Hex-encoded 32-byte key (with or without "0x" prefix)
    /// - Sui bech32-encoded key (`suiprivkey1q...`)
    pub fn from_hex(key: &str) -> Result<Self> {
        let key_bytes = if key.starts_with("suiprivkey") {
            Self::decode_bech32(key)?
        } else {
            Self::decode_hex(key)?
        };

        let signing_key = SigningKey::from_bytes(&key_bytes);
        let verifying_key = signing_key.verifying_key();

        Ok(Self {
            signing_key,
            verifying_key,
        })
    }

    /// Decode a hex-encoded private key (with or without "0x" prefix).
    fn decode_hex(hex_key: &str) -> Result<[u8; 32]> {
        let clean = hex_key.strip_prefix("0x").unwrap_or(hex_key);
        let bytes = hex::decode(clean).context("Invalid hex private key")?;

        if bytes.len() != 32 {
            anyhow::bail!(
                "Private key must be 32 bytes, got {} bytes",
                bytes.len()
            );
        }

        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&bytes);
        Ok(key_bytes)
    }

    /// Decode a Sui bech32-encoded private key (`suiprivkey1q...`).
    /// Format: bech32(hrp="suiprivkey", data = flag_byte || 32_byte_key)
    fn decode_bech32(bech32_key: &str) -> Result<[u8; 32]> {
        let (_hrp, data) =
            bech32::decode(bech32_key).context("Invalid bech32 private key")?;

        // data = 1-byte flag + 32-byte key
        if data.len() != 33 {
            anyhow::bail!(
                "Bech32 key data must be 33 bytes (1 flag + 32 key), got {}",
                data.len()
            );
        }

        let flag = data[0];
        if flag != 0x00 {
            anyhow::bail!(
                "Expected Ed25519 flag (0x00), got 0x{:02x}. Only Ed25519 keys are supported.",
                flag
            );
        }

        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&data[1..]);
        Ok(key_bytes)
    }

    /// Get the Sui address derived from this key.
    /// Sui address = BLAKE2b-256(flag_byte || public_key)[0..32]
    pub fn address(&self) -> String {
        use std::io::Write;
        let pk_bytes = self.verifying_key.to_bytes();

        // Sui address = blake2b_256(0x00 || pk_bytes)
        let mut hasher = blake2b_simd::Params::new()
            .hash_length(32)
            .to_state();
        hasher.write_all(&[0x00]).unwrap(); // Ed25519 flag
        hasher.write_all(&pk_bytes).unwrap();
        let hash = hasher.finalize();

        format!("0x{}", hex::encode(hash.as_bytes()))
    }

    /// Sign transaction bytes and return the serialized signature.
    /// Format: base64(flag_byte || ed25519_signature || public_key)
    pub fn sign_transaction(&self, tx_bytes_base64: &str) -> Result<String> {
        let tx_bytes = base64::engine::general_purpose::STANDARD
            .decode(tx_bytes_base64)
            .context("Invalid base64 tx bytes")?;

        // Sui signs blake2b_256(intent || tx_bytes)
        // Intent: [0, 0, 0] for TransactionData
        let mut intent_message = vec![0u8, 0, 0];
        intent_message.extend_from_slice(&tx_bytes);

        use std::io::Write;
        let mut hasher = blake2b_simd::Params::new()
            .hash_length(32)
            .to_state();
        hasher.write_all(&intent_message).unwrap();
        let digest = hasher.finalize();

        let signature = self.signing_key.sign(digest.as_bytes());

        // Serialize: flag || signature || public_key
        let mut sig_bytes = Vec::with_capacity(1 + 64 + 32);
        sig_bytes.push(0x00); // Ed25519 flag
        sig_bytes.extend_from_slice(&signature.to_bytes());
        sig_bytes.extend_from_slice(&self.verifying_key.to_bytes());

        Ok(base64::engine::general_purpose::STANDARD.encode(&sig_bytes))
    }

    /// Get the public key bytes (32 bytes).
    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.verifying_key.to_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signer_from_hex() {
        // Generate a test key
        let key_hex = "0x".to_string() + &hex::encode([42u8; 32]);
        let signer = Signer::from_hex(&key_hex).unwrap();
        let addr = signer.address();
        assert!(addr.starts_with("0x"));
        assert_eq!(addr.len(), 66); // "0x" + 64 hex chars
    }

    #[test]
    fn test_signer_rejects_invalid_key() {
        assert!(Signer::from_hex("0xabc").is_err()); // too short
        assert!(Signer::from_hex("not_hex").is_err());
    }

    #[test]
    fn test_signer_from_bech32() {
        // Encode a known key in bech32 format: suiprivkey + flag(0x00) + 32 bytes
        let mut data = vec![0x00u8]; // Ed25519 flag
        data.extend_from_slice(&[42u8; 32]); // key bytes
        let encoded = bech32::encode::<bech32::Bech32>(
            bech32::Hrp::parse("suiprivkey").unwrap(),
            &data,
        ).unwrap();

        let signer = Signer::from_hex(&encoded).unwrap();
        let addr = signer.address();
        assert!(addr.starts_with("0x"));
        assert_eq!(addr.len(), 66);

        // Should produce same address as hex version
        let hex_signer = Signer::from_hex(&("0x".to_string() + &hex::encode([42u8; 32]))).unwrap();
        assert_eq!(signer.address(), hex_signer.address());
    }

    #[test]
    fn test_signer_rejects_non_ed25519_bech32() {
        // flag = 0x01 (not Ed25519)
        let mut data = vec![0x01u8];
        data.extend_from_slice(&[42u8; 32]);
        let encoded = bech32::encode::<bech32::Bech32>(
            bech32::Hrp::parse("suiprivkey").unwrap(),
            &data,
        ).unwrap();

        assert!(Signer::from_hex(&encoded).is_err());
    }
}
