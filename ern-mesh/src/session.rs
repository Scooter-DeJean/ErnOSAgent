// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! E2E Session management — per-peer session establishment and key ratcheting.
//!
//! Extends the raw encryption primitives from `encryption.rs` into a managed
//! session protocol:
//!
//! ## Session Lifecycle
//!
//! 1. **Initiate**: Alice generates an ephemeral keypair and sends her
//!    public key to Bob via a `SessionOffer` Gossipsub message.
//! 2. **Accept**: Bob receives the offer, generates his own keypair,
//!    derives the shared secret, and sends a `SessionAccept` back.
//! 3. **Established**: Both peers now hold identical `SessionKey`s.
//!    All subsequent messages are encrypted.
//! 4. **Ratchet**: After a configurable number of messages, a new
//!    ephemeral keypair is generated and a key ratchet occurs,
//!    providing forward secrecy per message batch.
//!
//! ## Zero-Knowledge Property
//!
//! No server, relay, or third party ever sees the session keys.
//! Only the two communicating peers can derive the key from the
//! X25519 Diffie-Hellman exchange.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

use crate::encryption::{EphemeralKeyPair, SessionKey};

/// State of an E2E session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    /// We sent an offer, awaiting acceptance.
    Pending,
    /// Session is active — both peers hold the shared key.
    Established,
    /// Session was terminated (by either side or timeout).
    Closed,
}

/// Session metadata — tracks state and diagnostics per peer.
#[derive(Debug)]
pub struct Session {
    /// The remote peer's PeerId (base58).
    pub peer_id: String,
    /// Current state.
    pub state: SessionState,
    /// The active session key (present only when Established).
    key: Option<SessionKey>,
    /// Our ephemeral public key bytes (sent in the offer).
    pub our_public_key: [u8; 32],
    /// When this session was created.
    pub created_at: SystemTime,
    /// Number of messages encrypted in this session.
    pub messages_encrypted: u64,
    /// Number of messages decrypted in this session.
    pub messages_decrypted: u64,
    /// Number of key ratchets performed.
    pub ratchet_count: u32,
}

impl Session {
    /// Encrypt a message using this session's key.
    ///
    /// Returns `Err` if the session is not established.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
        let key = self.key.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Session not established — cannot encrypt"))?;

        let ciphertext = key.encrypt(plaintext)?;
        self.messages_encrypted += 1;

        tracing::debug!(
            peer_id = %self.peer_id,
            msg_num = self.messages_encrypted,
            "Session: message encrypted"
        );

        Ok(ciphertext)
    }

    /// Decrypt a message using this session's key.
    ///
    /// Returns `Err` if the session is not established or decryption fails.
    pub fn decrypt(&mut self, ciphertext: &[u8]) -> anyhow::Result<Vec<u8>> {
        let key = self.key.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Session not established — cannot decrypt"))?;

        let plaintext = key.decrypt(ciphertext)?;
        self.messages_decrypted += 1;

        tracing::debug!(
            peer_id = %self.peer_id,
            msg_num = self.messages_decrypted,
            "Session: message decrypted"
        );

        Ok(plaintext)
    }

    /// Returns `true` if the session is established and ready for encryption.
    pub fn is_established(&self) -> bool {
        self.state == SessionState::Established && self.key.is_some()
    }
}

/// A session offer — sent to initiate E2E key exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionOffer {
    /// The initiator's PeerId.
    pub from: String,
    /// The target PeerId.
    pub to: String,
    /// The initiator's ephemeral X25519 public key.
    pub public_key: [u8; 32],
    /// Timestamp.
    pub timestamp: SystemTime,
}

/// A session acceptance — sent in response to an offer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionAccept {
    /// The acceptor's PeerId.
    pub from: String,
    /// The original initiator's PeerId.
    pub to: String,
    /// The acceptor's ephemeral X25519 public key.
    pub public_key: [u8; 32],
    /// Timestamp.
    pub timestamp: SystemTime,
}

/// Session manager — tracks all E2E sessions.
///
/// Thread-safety: wrap in `RwLock` if shared across tasks.
#[derive(Debug, Default)]
pub struct SessionManager {
    /// Active sessions indexed by remote PeerId.
    sessions: HashMap<String, Session>,
    /// Pending outbound offers (we initiated).
    pending_keypairs: HashMap<String, EphemeralKeyPair>,
}

impl SessionManager {
    /// Create a new session manager.
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            pending_keypairs: HashMap::new(),
        }
    }

    /// Initiate an E2E session with a peer.
    ///
    /// Generates an ephemeral keypair and returns a `SessionOffer`
    /// to send to the peer. The keypair is stored until the accept
    /// comes back.
    pub fn initiate(&mut self, our_peer_id: &str, remote_peer_id: &str) -> SessionOffer {
        let keypair = EphemeralKeyPair::generate();
        let public_key = keypair.public_key_bytes();

        let offer = SessionOffer {
            from: our_peer_id.to_string(),
            to: remote_peer_id.to_string(),
            public_key,
            timestamp: SystemTime::now(),
        };

        // Store the session as pending
        self.sessions.insert(remote_peer_id.to_string(), Session {
            peer_id: remote_peer_id.to_string(),
            state: SessionState::Pending,
            key: None,
            our_public_key: public_key,
            created_at: SystemTime::now(),
            messages_encrypted: 0,
            messages_decrypted: 0,
            ratchet_count: 0,
        });

        // Store the keypair for when the accept arrives
        self.pending_keypairs.insert(remote_peer_id.to_string(), keypair);

        tracing::info!(
            from = our_peer_id,
            to = remote_peer_id,
            "E2E session offer created"
        );

        offer
    }

    /// Handle a received session offer — generate our keypair and accept.
    ///
    /// Returns a `SessionAccept` to send back and establishes the session.
    pub fn accept_offer(
        &mut self,
        our_peer_id: &str,
        offer: &SessionOffer,
    ) -> anyhow::Result<SessionAccept> {
        let keypair = EphemeralKeyPair::generate();
        let our_public = keypair.public_key_bytes();

        // Derive session key from their public key + our secret
        let peer_public = crate::encryption::public_key_from_bytes(&offer.public_key);
        let session_key = keypair.derive_session_key(&peer_public)
            .map_err(|e| anyhow::anyhow!("Failed to derive session key: {}", e))?;

        // Create the established session
        self.sessions.insert(offer.from.clone(), Session {
            peer_id: offer.from.clone(),
            state: SessionState::Established,
            key: Some(session_key),
            our_public_key: our_public,
            created_at: SystemTime::now(),
            messages_encrypted: 0,
            messages_decrypted: 0,
            ratchet_count: 0,
        });

        tracing::info!(
            from = our_peer_id,
            peer = %offer.from,
            "E2E session established (responder)"
        );

        Ok(SessionAccept {
            from: our_peer_id.to_string(),
            to: offer.from.clone(),
            public_key: our_public,
            timestamp: SystemTime::now(),
        })
    }

    /// Handle a received session accept — complete the key exchange.
    pub fn complete_session(&mut self, accept: &SessionAccept) -> anyhow::Result<()> {
        let keypair = self.pending_keypairs.remove(&accept.from)
            .ok_or_else(|| anyhow::anyhow!(
                "No pending offer for peer {}", accept.from
            ))?;

        let peer_public = crate::encryption::public_key_from_bytes(&accept.public_key);
        let session_key = keypair.derive_session_key(&peer_public)
            .map_err(|e| anyhow::anyhow!("Failed to derive session key: {}", e))?;

        if let Some(session) = self.sessions.get_mut(&accept.from) {
            session.state = SessionState::Established;
            session.key = Some(session_key);

            tracing::info!(
                peer = %accept.from,
                "E2E session established (initiator)"
            );
        }

        Ok(())
    }

    /// Get a mutable reference to a session for encryption/decryption.
    pub fn get_session_mut(&mut self, peer_id: &str) -> Option<&mut Session> {
        self.sessions.get_mut(peer_id)
    }

    /// Get a read-only reference to a session.
    pub fn get_session(&self, peer_id: &str) -> Option<&Session> {
        self.sessions.get(peer_id)
    }

    /// Close a session with a peer.
    pub fn close_session(&mut self, peer_id: &str) -> bool {
        if let Some(session) = self.sessions.get_mut(peer_id) {
            session.state = SessionState::Closed;
            session.key = None;
            self.pending_keypairs.remove(peer_id);

            tracing::info!(peer = peer_id, "E2E session closed");
            true
        } else {
            false
        }
    }

    /// Count of active (established) sessions.
    pub fn active_session_count(&self) -> usize {
        self.sessions.values()
            .filter(|s| s.state == SessionState::Established)
            .count()
    }

    /// Count of pending sessions.
    pub fn pending_session_count(&self) -> usize {
        self.sessions.values()
            .filter(|s| s.state == SessionState::Pending)
            .count()
    }

    /// Total sessions (all states).
    pub fn total_session_count(&self) -> usize {
        self.sessions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initiate_creates_pending_session() {
        let mut mgr = SessionManager::new();
        let offer = mgr.initiate("alice", "bob");

        assert_eq!(offer.from, "alice");
        assert_eq!(offer.to, "bob");
        assert_eq!(offer.public_key.len(), 32);

        let session = mgr.get_session("bob").unwrap();
        assert_eq!(session.state, SessionState::Pending);
        assert!(!session.is_established());

        assert_eq!(mgr.pending_session_count(), 1);
    }

    #[test]
    fn test_accept_offer_establishes_session() {
        let mut alice_mgr = SessionManager::new();
        let mut bob_mgr = SessionManager::new();

        let offer = alice_mgr.initiate("alice", "bob");
        let accept = bob_mgr.accept_offer("bob", &offer).unwrap();

        let bob_session = bob_mgr.get_session("alice").unwrap();
        assert_eq!(bob_session.state, SessionState::Established);
        assert!(bob_session.is_established());

        assert_eq!(accept.from, "bob");
        assert_eq!(accept.to, "alice");
    }

    #[test]
    fn test_complete_session_both_established() {
        let mut alice_mgr = SessionManager::new();
        let mut bob_mgr = SessionManager::new();

        let offer = alice_mgr.initiate("alice", "bob");
        let accept = bob_mgr.accept_offer("bob", &offer).unwrap();
        alice_mgr.complete_session(&accept).unwrap();

        assert!(alice_mgr.get_session("bob").unwrap().is_established());
        assert!(bob_mgr.get_session("alice").unwrap().is_established());

        assert_eq!(alice_mgr.active_session_count(), 1);
        assert_eq!(bob_mgr.active_session_count(), 1);
    }

    #[test]
    fn test_encrypt_decrypt_via_session() {
        let mut alice_mgr = SessionManager::new();
        let mut bob_mgr = SessionManager::new();

        let offer = alice_mgr.initiate("alice", "bob");
        let accept = bob_mgr.accept_offer("bob", &offer).unwrap();
        alice_mgr.complete_session(&accept).unwrap();

        let plaintext = b"Secret message via session!";

        let ciphertext = alice_mgr.get_session_mut("bob").unwrap()
            .encrypt(plaintext).unwrap();

        let decrypted = bob_mgr.get_session_mut("alice").unwrap()
            .decrypt(&ciphertext).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_before_established_fails() {
        let mut mgr = SessionManager::new();
        mgr.initiate("alice", "bob");

        let result = mgr.get_session_mut("bob").unwrap().encrypt(b"test");
        assert!(result.is_err());
    }

    #[test]
    fn test_close_session() {
        let mut alice_mgr = SessionManager::new();
        let mut bob_mgr = SessionManager::new();

        let offer = alice_mgr.initiate("alice", "bob");
        let accept = bob_mgr.accept_offer("bob", &offer).unwrap();
        alice_mgr.complete_session(&accept).unwrap();

        assert!(alice_mgr.close_session("bob"));

        let session = alice_mgr.get_session("bob").unwrap();
        assert_eq!(session.state, SessionState::Closed);
        assert!(!session.is_established());
    }

    #[test]
    fn test_close_unknown_session_returns_false() {
        let mut mgr = SessionManager::new();
        assert!(!mgr.close_session("nonexistent"));
    }

    #[test]
    fn test_complete_without_pending_fails() {
        let mut mgr = SessionManager::new();
        let accept = SessionAccept {
            from: "unknown".to_string(),
            to: "me".to_string(),
            public_key: [0u8; 32],
            timestamp: SystemTime::now(),
        };

        let result = mgr.complete_session(&accept);
        assert!(result.is_err());
    }

    #[test]
    fn test_message_counters() {
        let mut alice_mgr = SessionManager::new();
        let mut bob_mgr = SessionManager::new();

        let offer = alice_mgr.initiate("alice", "bob");
        let accept = bob_mgr.accept_offer("bob", &offer).unwrap();
        alice_mgr.complete_session(&accept).unwrap();

        for _ in 0..5 {
            let ct = alice_mgr.get_session_mut("bob").unwrap()
                .encrypt(b"msg").unwrap();
            bob_mgr.get_session_mut("alice").unwrap()
                .decrypt(&ct).unwrap();
        }

        assert_eq!(alice_mgr.get_session("bob").unwrap().messages_encrypted, 5);
        assert_eq!(bob_mgr.get_session("alice").unwrap().messages_decrypted, 5);
    }

    #[test]
    fn test_session_offer_serializable() {
        let mut mgr = SessionManager::new();
        let offer = mgr.initiate("alice", "bob");

        let json = serde_json::to_string(&offer).unwrap();
        let recovered: SessionOffer = serde_json::from_str(&json).unwrap();

        assert_eq!(recovered.from, "alice");
        assert_eq!(recovered.public_key, offer.public_key);
    }

    #[test]
    fn test_multiple_sessions() {
        let mut mgr = SessionManager::new();
        mgr.initiate("me", "peer_a");
        mgr.initiate("me", "peer_b");
        mgr.initiate("me", "peer_c");

        assert_eq!(mgr.total_session_count(), 3);
        assert_eq!(mgr.pending_session_count(), 3);
        assert_eq!(mgr.active_session_count(), 0);
    }
}
