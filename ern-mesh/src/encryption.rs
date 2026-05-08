// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Zero-knowledge E2E encryption — X25519 key exchange + AES-256-GCM.
//!
//! ErnMesh implements end-to-end encryption per §14.6 with the following
//! properties:
//!
//! - **No master keys**: There is no backdoor. No server, operator, or
//!   government can decrypt messages. Only the two communicating peers
//!   hold the session key.
//! - **Perfect forward secrecy**: Each session uses a fresh ephemeral
//!   X25519 key pair. Compromising a long-term key does not reveal
//!   past session keys.
//! - **Authenticated encryption**: AES-256-GCM provides both
//!   confidentiality and integrity. Tampered ciphertext is rejected.
//! - **Unique nonces**: Every encryption uses a random 96-bit nonce.
//!   Nonce reuse is cryptographically impossible in practice.
//!
//! ## Architecture
//!
//! 1. **Key Exchange**: Both peers generate ephemeral X25519 key pairs.
//!    They exchange public keys and derive a shared secret via ECDH.
//! 2. **Key Derivation**: The shared secret is fed through HKDF-SHA256
//!    to produce the 256-bit AES session key.
//! 3. **Encryption**: Messages are encrypted with AES-256-GCM using
//!    random nonces. The nonce is prepended to the ciphertext.
//! 4. **Decryption**: The nonce is extracted from the ciphertext prefix,
//!    and the message is decrypted and authenticated.
//!
//! This module provides the cryptographic primitives. The session
//! management and ratcheting protocol build on top in future PRs.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{Context, Result};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{EphemeralSecret, PublicKey, SharedSecret};

/// Size of AES-256-GCM nonce in bytes.
const NONCE_SIZE: usize = 12;

/// Info string for HKDF key derivation — domain separation.
const HKDF_INFO: &[u8] = b"ernmesh-e2e-v1";

/// An ephemeral X25519 key pair for one side of a key exchange.
///
/// The secret is consumed during key exchange and cannot be reused
/// (enforced by `EphemeralSecret`'s move semantics).
pub struct EphemeralKeyPair {
    /// The secret key (consumed on `derive_shared_secret`).
    secret: EphemeralSecret,
    /// The public key to send to the peer.
    public: PublicKey,
}

impl std::fmt::Debug for EphemeralKeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EphemeralKeyPair")
            .field("public", &hex::encode(self.public.as_bytes()))
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

impl EphemeralKeyPair {
    /// Generate a fresh ephemeral X25519 key pair.
    pub fn generate() -> Self {
        let secret = EphemeralSecret::random_from_rng(rand::rngs::OsRng);
        let public = PublicKey::from(&secret);

        tracing::debug!(
            public_key = hex::encode(public.as_bytes()),
            "Generated ephemeral X25519 key pair"
        );

        Self { secret, public }
    }

    /// Returns the public key to send to the peer.
    pub fn public_key(&self) -> &PublicKey {
        &self.public
    }

    /// Returns the public key as bytes.
    pub fn public_key_bytes(&self) -> [u8; 32] {
        *self.public.as_bytes()
    }

    /// Perform X25519 Diffie-Hellman and derive the AES-256 session key.
    ///
    /// Consumes `self` — the ephemeral secret cannot be reused.
    pub fn derive_session_key(self, peer_public: &PublicKey) -> Result<SessionKey> {
        let shared_secret = self.secret.diffie_hellman(peer_public);
        SessionKey::from_shared_secret(&shared_secret)
    }
}

/// A derived AES-256-GCM session key — used for encrypting/decrypting messages.
///
/// Created from a shared secret via HKDF-SHA256 key derivation.
/// No master key exists — only the two peers who completed the key exchange
/// can derive this key.
pub struct SessionKey {
    /// The 256-bit AES key (derived, never transmitted).
    key_bytes: [u8; 32],
}

impl std::fmt::Debug for SessionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionKey")
            .field("key_bytes", &"[REDACTED]")
            .finish()
    }
}

impl SessionKey {
    /// Derive a session key from an X25519 shared secret via HKDF.
    fn from_shared_secret(shared: &SharedSecret) -> Result<Self> {
        let hkdf = Hkdf::<Sha256>::new(None, shared.as_bytes());
        let mut key_bytes = [0u8; 32];
        hkdf.expand(HKDF_INFO, &mut key_bytes)
            .map_err(|_| anyhow::anyhow!("HKDF expand failed"))?;

        tracing::debug!("Session key derived via HKDF-SHA256");

        Ok(Self { key_bytes })
    }

    /// Encrypt a plaintext message.
    ///
    /// Returns: `nonce (12 bytes) || ciphertext (with GCM tag)`.
    /// The nonce is randomly generated and prepended to the output.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let cipher = Aes256Gcm::new_from_slice(&self.key_bytes)
            .context("Failed to create AES-256-GCM cipher")?;

        // Generate random nonce
        let nonce_bytes: [u8; NONCE_SIZE] = rand::random();
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher.encrypt(nonce, plaintext)
            .map_err(|e| anyhow::anyhow!("AES-256-GCM encryption failed: {}", e))?;

        // Prepend nonce to ciphertext
        let mut output = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
        output.extend_from_slice(&nonce_bytes);
        output.extend_from_slice(&ciphertext);

        tracing::debug!(
            plaintext_len = plaintext.len(),
            ciphertext_len = output.len(),
            "Message encrypted"
        );

        Ok(output)
    }

    /// Decrypt a ciphertext produced by [`encrypt`].
    ///
    /// Input format: `nonce (12 bytes) || ciphertext (with GCM tag)`.
    /// Returns the original plaintext, or an error if:
    /// - The input is too short (< 12 bytes)
    /// - The GCM tag verification fails (tampered or wrong key)
    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() < NONCE_SIZE {
            anyhow::bail!(
                "Ciphertext too short: {} bytes (minimum {} for nonce)",
                data.len(),
                NONCE_SIZE
            );
        }

        let (nonce_bytes, ciphertext) = data.split_at(NONCE_SIZE);
        let nonce = Nonce::from_slice(nonce_bytes);

        let cipher = Aes256Gcm::new_from_slice(&self.key_bytes)
            .context("Failed to create AES-256-GCM cipher")?;

        let plaintext = cipher.decrypt(nonce, ciphertext)
            .map_err(|e| anyhow::anyhow!(
                "AES-256-GCM decryption failed (wrong key or tampered data): {}", e
            ))?;

        tracing::debug!(
            ciphertext_len = data.len(),
            plaintext_len = plaintext.len(),
            "Message decrypted"
        );

        Ok(plaintext)
    }
}

/// Reconstruct a peer's public key from raw bytes.
pub fn public_key_from_bytes(bytes: &[u8; 32]) -> PublicKey {
    PublicKey::from(*bytes)
}

/// Encode a public key as a hex string (for wire format / display).
pub fn public_key_to_hex(key: &PublicKey) -> String {
    hex::encode(key.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_ephemeral_keypair() {
        let kp = EphemeralKeyPair::generate();
        let pub_bytes = kp.public_key_bytes();
        assert_eq!(pub_bytes.len(), 32);
    }

    #[test]
    fn test_two_keypairs_different_public_keys() {
        let kp1 = EphemeralKeyPair::generate();
        let kp2 = EphemeralKeyPair::generate();
        assert_ne!(
            kp1.public_key_bytes(),
            kp2.public_key_bytes(),
            "Two ephemeral keypairs must have different public keys"
        );
    }

    #[test]
    fn test_key_exchange_produces_session_key() {
        let alice = EphemeralKeyPair::generate();
        let bob = EphemeralKeyPair::generate();

        let alice_pub = *alice.public_key();
        let bob_pub = *bob.public_key();

        let alice_key = alice.derive_session_key(&bob_pub);
        let bob_key = bob.derive_session_key(&alice_pub);

        assert!(alice_key.is_ok());
        assert!(bob_key.is_ok());
    }

    #[test]
    fn test_shared_session_keys_match() {
        let alice = EphemeralKeyPair::generate();
        let bob = EphemeralKeyPair::generate();

        let alice_pub = *alice.public_key();
        let bob_pub = *bob.public_key();

        let alice_key = alice.derive_session_key(&bob_pub).unwrap();
        let bob_key = bob.derive_session_key(&alice_pub).unwrap();

        // Both sides must derive the same key
        assert_eq!(
            alice_key.key_bytes,
            bob_key.key_bytes,
            "Both peers must derive the same session key"
        );
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let alice = EphemeralKeyPair::generate();
        let bob = EphemeralKeyPair::generate();

        let alice_pub = *alice.public_key();
        let bob_pub = *bob.public_key();

        let alice_key = alice.derive_session_key(&bob_pub).unwrap();
        let bob_key = bob.derive_session_key(&alice_pub).unwrap();

        let message = b"Hello, ErnMesh! This is a secret message.";
        let encrypted = alice_key.encrypt(message).unwrap();
        let decrypted = bob_key.decrypt(&encrypted).unwrap();

        assert_eq!(decrypted, message);
    }

    #[test]
    fn test_encrypt_produces_different_ciphertexts() {
        let alice = EphemeralKeyPair::generate();
        let bob = EphemeralKeyPair::generate();
        let key = alice.derive_session_key(bob.public_key()).unwrap();

        let msg = b"same message";
        let ct1 = key.encrypt(msg).unwrap();
        let ct2 = key.encrypt(msg).unwrap();

        assert_ne!(ct1, ct2, "Different nonces must produce different ciphertexts");
    }

    #[test]
    fn test_decrypt_wrong_key_fails() {
        let alice = EphemeralKeyPair::generate();
        let bob = EphemeralKeyPair::generate();
        let eve = EphemeralKeyPair::generate();

        let alice_key = alice.derive_session_key(bob.public_key()).unwrap();
        let eve_key = eve.derive_session_key(&PublicKey::from([0u8; 32])).unwrap();

        let encrypted = alice_key.encrypt(b"secret").unwrap();
        let result = eve_key.decrypt(&encrypted);

        assert!(result.is_err(), "Wrong key must fail decryption");
    }

    #[test]
    fn test_decrypt_tampered_ciphertext_fails() {
        let alice = EphemeralKeyPair::generate();
        let bob = EphemeralKeyPair::generate();

        let alice_pub = *alice.public_key();
        let bob_pub = *bob.public_key();

        let alice_key = alice.derive_session_key(&bob_pub).unwrap();
        let bob_key = bob.derive_session_key(&alice_pub).unwrap();

        let mut encrypted = alice_key.encrypt(b"authentic message").unwrap();

        // Tamper with the ciphertext (flip a bit in the data, not the nonce)
        if encrypted.len() > NONCE_SIZE + 1 {
            encrypted[NONCE_SIZE + 1] ^= 0xFF;
        }

        let result = bob_key.decrypt(&encrypted);
        assert!(result.is_err(), "Tampered ciphertext must fail GCM verification");
    }

    #[test]
    fn test_decrypt_too_short_input_fails() {
        let alice = EphemeralKeyPair::generate();
        let bob = EphemeralKeyPair::generate();
        let key = alice.derive_session_key(bob.public_key()).unwrap();

        let result = key.decrypt(&[0u8; 5]);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("too short"),
            "Must report input too short"
        );
    }

    #[test]
    fn test_encrypt_empty_message() {
        let alice = EphemeralKeyPair::generate();
        let bob = EphemeralKeyPair::generate();

        let alice_pub = *alice.public_key();
        let bob_pub = *bob.public_key();

        let alice_key = alice.derive_session_key(&bob_pub).unwrap();
        let bob_key = bob.derive_session_key(&alice_pub).unwrap();

        let encrypted = alice_key.encrypt(b"").unwrap();
        let decrypted = bob_key.decrypt(&encrypted).unwrap();

        assert!(decrypted.is_empty());
    }

    #[test]
    fn test_encrypt_large_message() {
        let alice = EphemeralKeyPair::generate();
        let bob = EphemeralKeyPair::generate();

        let alice_pub = *alice.public_key();
        let bob_pub = *bob.public_key();

        let alice_key = alice.derive_session_key(&bob_pub).unwrap();
        let bob_key = bob.derive_session_key(&alice_pub).unwrap();

        let large_msg = vec![0xAB; 1024 * 1024]; // 1MB
        let encrypted = alice_key.encrypt(&large_msg).unwrap();
        let decrypted = bob_key.decrypt(&encrypted).unwrap();

        assert_eq!(decrypted, large_msg);
    }

    #[test]
    fn test_public_key_roundtrip() {
        let kp = EphemeralKeyPair::generate();
        let bytes = kp.public_key_bytes();
        let recovered = public_key_from_bytes(&bytes);
        assert_eq!(recovered.as_bytes(), kp.public_key().as_bytes());
    }

    #[test]
    fn test_public_key_hex() {
        let kp = EphemeralKeyPair::generate();
        let hex = public_key_to_hex(kp.public_key());
        assert_eq!(hex.len(), 64, "X25519 public key hex must be 64 chars");
    }
}
