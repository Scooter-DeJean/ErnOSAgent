// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Mesh protocol — message envelope, signing, verification, and replay protection.
//!
//! Every message sent between mesh nodes is wrapped in a [`SignedEnvelope`]:
//! a standardised wire format that provides authentication, integrity, and
//! replay protection per §14.3.
//!
//! **Signing**: The sender signs the serialised envelope body (sender PeerId,
//! sequence number, timestamp, and payload) with their Ed25519 private key.
//!
//! **Verification**: The receiver verifies the signature using the sender's
//! public key (obtained via the libp2p Identify protocol). Unsigned or
//! invalid-signature messages are dropped and logged at `warn` level.
//!
//! **Replay protection**: Each sender maintains a monotonically increasing
//! sequence number. The receiver tracks the last seen sequence per sender
//! and rejects messages with sequence numbers it has already seen.

use anyhow::{Context, Result};
use libp2p_identity::{Keypair, PeerId, PublicKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Maximum serialised message size in bytes.
/// Read from config at runtime — this constant is the absolute ceiling
/// to prevent memory exhaustion from malicious payloads before config
/// is loaded. Actual limit is `config.security.max_payload_bytes`.
const MAX_WIRE_BYTES: usize = 16 * 1024 * 1024; // 16 MiB hard ceiling

// ─── Message Envelope ────────────────────────────────────────────────

/// The unsigned body of a mesh message — everything that gets signed.
///
/// This is serialised to bytes, then signed by the sender's Ed25519 key.
/// The signature covers all fields: sender, sequence, timestamp, and payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnvelopeBody {
    /// PeerId of the sending node (base58-encoded).
    pub sender: String,

    /// Monotonically increasing sequence number per sender.
    /// Used for replay protection — receivers reject duplicates.
    pub sequence: u64,

    /// Unix timestamp in seconds when the message was created.
    pub timestamp_secs: u64,

    /// The message payload — the actual content being transmitted.
    pub payload: MessagePayload,
}

/// A signed message envelope — the complete wire format.
///
/// Every message on the mesh is wrapped in this envelope. The receiver
/// must call [`verify_envelope`] before processing the payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SignedEnvelope {
    /// The message body (sender, sequence, timestamp, payload).
    pub body: EnvelopeBody,

    /// Ed25519 signature over the serialised `body` bytes.
    pub signature: Vec<u8>,
}

/// Message payload types — the actual content carried by the envelope.
///
/// New payload variants are added as mesh services are built (ErnChat,
/// ErnDrive, etc.). Each variant is a self-contained, typed message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "data")]
pub enum MessagePayload {
    /// Basic connectivity check — sent periodically to verify peer liveness.
    Ping { nonce: u64 },

    /// Response to a Ping — echo back the nonce for RTT measurement.
    Pong { nonce: u64 },

    /// Contribution proof — bilateral attestation of resource sharing.
    /// Both the provider and consumer sign proofs for ErnPoints verification.
    ContributionProof {
        /// What was contributed: "bandwidth", "storage", "compute", "uptime".
        resource_type: String,
        /// Quantity contributed (unit depends on resource_type).
        quantity: f64,
        /// PeerId of the peer who consumed this resource.
        consumer: String,
        /// Unix timestamp when the contribution period started.
        period_start_secs: u64,
        /// Unix timestamp when the contribution period ended.
        period_end_secs: u64,
    },

    /// Generic data message — used for Gossipsub topic payloads.
    /// The `topic` field identifies the Gossipsub topic, and `data`
    /// carries the application-layer content (chat message, post, etc.).
    TopicMessage {
        /// Gossipsub topic identifier (e.g., "ernchat/server123/general").
        topic: String,
        /// Application-layer content bytes (opaque to the protocol layer).
        data: Vec<u8>,
    },
}

// ─── Signing ─────────────────────────────────────────────────────────

/// Create a signed envelope from a payload.
///
/// Serialises the envelope body, signs it with the node's Ed25519 key,
/// and returns the complete [`SignedEnvelope`] ready for transmission.
pub fn create_signed_envelope(
    keypair: &Keypair,
    peer_id: &PeerId,
    sequence: u64,
    payload: MessagePayload,
) -> Result<SignedEnvelope> {
    let body = EnvelopeBody {
        sender: peer_id.to_base58(),
        sequence,
        timestamp_secs: current_timestamp_secs(),
        payload,
    };

    let body_bytes = serialize_body(&body)?;

    let signature = keypair
        .sign(&body_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to sign envelope: {}", e))?;

    tracing::debug!(
        peer_id = %peer_id,
        sequence = sequence,
        payload_size = body_bytes.len(),
        "Created signed envelope"
    );

    Ok(SignedEnvelope { body, signature })
}

/// Serialise a [`SignedEnvelope`] to wire bytes.
///
/// Uses bincode-style JSON for human-debuggability during development.
/// The output is a UTF-8 JSON byte vector.
pub fn serialize_envelope(envelope: &SignedEnvelope) -> Result<Vec<u8>> {
    serde_json::to_vec(envelope)
        .context("Failed to serialise signed envelope to wire bytes")
}

/// Deserialise a [`SignedEnvelope`] from wire bytes.
///
/// Enforces a size limit before parsing to prevent memory exhaustion
/// from malicious payloads (§13.2, R18). The `max_bytes` parameter
/// should come from `config.security.max_payload_bytes`.
pub fn deserialize_envelope(
    data: &[u8],
    max_bytes: u64,
) -> Result<SignedEnvelope> {
    let limit = max_bytes.min(MAX_WIRE_BYTES as u64) as usize;

    if data.len() > limit {
        anyhow::bail!(
            "Message exceeds size limit: {} bytes > {} byte limit. \
             Dropped to prevent memory exhaustion.",
            data.len(),
            limit
        );
    }

    serde_json::from_slice(data)
        .context("Failed to deserialise signed envelope from wire bytes")
}

// ─── Verification ────────────────────────────────────────────────────

/// Verify a signed envelope's signature against the sender's public key.
///
/// Returns `Ok(())` if the signature is valid.
/// Returns `Err` if the signature is invalid — the message must be dropped.
///
/// The caller is responsible for obtaining the sender's public key
/// (typically via the libp2p Identify protocol handshake).
pub fn verify_envelope(
    envelope: &SignedEnvelope,
    sender_public_key: &PublicKey,
) -> Result<()> {
    let body_bytes = serialize_body(&envelope.body)?;

    let ed25519_pub = sender_public_key
        .clone()
        .try_into_ed25519()
        .map_err(|_| anyhow::anyhow!(
            "Sender public key is not Ed25519 — cannot verify signature"
        ))?;

    if ed25519_pub.verify(&body_bytes, &envelope.signature) {
        tracing::debug!(
            sender = %envelope.body.sender,
            sequence = envelope.body.sequence,
            "Envelope signature verified"
        );
        Ok(())
    } else {
        tracing::warn!(
            sender = %envelope.body.sender,
            sequence = envelope.body.sequence,
            "Envelope signature verification FAILED — dropping message"
        );
        anyhow::bail!(
            "Invalid signature from sender {} (seq {})",
            envelope.body.sender,
            envelope.body.sequence
        )
    }
}

// ─── Replay Protection ──────────────────────────────────────────────

/// Tracks the last seen sequence number per sender for replay protection.
///
/// Per §14.3: messages include a monotonically increasing sequence number
/// per sender. The receiver rejects messages with sequence numbers it has
/// already seen.
#[derive(Debug, Default)]
pub struct SequenceTracker {
    /// Maps sender PeerId (base58) → last accepted sequence number.
    last_seen: HashMap<String, u64>,
}

impl SequenceTracker {
    /// Create a new, empty sequence tracker.
    pub fn new() -> Self {
        Self {
            last_seen: HashMap::new(),
        }
    }

    /// Check if a message's sequence number is valid (not a replay).
    ///
    /// Returns `true` if the sequence is newer than the last seen.
    /// Returns `false` if it is a duplicate or out-of-order replay.
    ///
    /// If valid, updates the tracker with the new sequence number.
    pub fn accept(&mut self, sender: &str, sequence: u64) -> bool {
        let last = self.last_seen.get(sender).copied().unwrap_or(0);

        if sequence > last {
            self.last_seen.insert(sender.to_string(), sequence);
            true
        } else {
            tracing::warn!(
                sender = sender,
                sequence = sequence,
                last_seen = last,
                "Replay detected — rejecting message with stale sequence number"
            );
            false
        }
    }

    /// Returns the last seen sequence number for a sender, or 0 if unknown.
    pub fn last_sequence(&self, sender: &str) -> u64 {
        self.last_seen.get(sender).copied().unwrap_or(0)
    }

    /// Returns the number of unique senders being tracked.
    pub fn tracked_senders(&self) -> usize {
        self.last_seen.len()
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────

/// Serialise the envelope body to bytes for signing/verification.
fn serialize_body(body: &EnvelopeBody) -> Result<Vec<u8>> {
    serde_json::to_vec(body)
        .context("Failed to serialise envelope body for signing")
}

/// Get the current Unix timestamp in seconds.
fn current_timestamp_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// Tests extracted to protocol_tests.rs per §1.1 (file length limit).
#[cfg(test)]
#[path = "protocol_tests.rs"]
mod protocol_tests;
