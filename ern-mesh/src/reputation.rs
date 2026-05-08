// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Peer reputation — trust scoring for mesh participants.
//!
//! Every peer has a reputation score that reflects their behaviour:
//!
//! - **Contributions earn trust**: Relaying bandwidth, storing content,
//!   uptime, and providing compute all increase reputation.
//! - **Violations reduce trust**: Spamming, providing invalid data,
//!   failing to honour relay commitments, or exceeding rate limits
//!   decrease reputation.
//! - **Consequences**: Low-reputation peers are deprioritised for
//!   resource allocation and may be pruned from the mesh.
//! - **Decay**: Reputation decays towards neutral over time to
//!   prevent score hoarding and encourage continued contribution.
//!
//! Reputation is local — each node independently scores its peers.
//! No global consensus is needed.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

/// Reason for a reputation change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReputationEvent {
    /// Peer relayed bandwidth successfully.
    BandwidthRelayed { megabytes: f64 },
    /// Peer provided storage.
    StorageProvided { gigabyte_hours: f64 },
    /// Peer provided compute.
    ComputeProvided { kilo_tokens: f64 },
    /// Peer maintained uptime.
    UptimeContribution { hours: f64 },
    /// Peer sent valid content.
    ValidContent,
    /// Peer sent invalid/corrupt data.
    InvalidData,
    /// Peer exceeded rate limits.
    RateLimitViolation,
    /// Peer sent spam messages.
    SpamDetected,
    /// Peer failed to honour a relay commitment.
    RelayFailure,
    /// Peer timed out during a file transfer.
    TransferTimeout,
    /// Manual trust grant by operator.
    ManualTrust { delta: f64 },
}

/// Per-peer reputation record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerReputation {
    /// PeerId (base58).
    pub peer_id: String,
    /// Current reputation score (0.0 = neutral, positive = trusted, negative = untrusted).
    pub score: f64,
    /// Total positive events.
    pub positive_events: u64,
    /// Total negative events.
    pub negative_events: u64,
    /// When this peer was first seen.
    pub first_seen: SystemTime,
    /// When the score was last updated.
    pub last_updated: SystemTime,
}

impl PeerReputation {
    /// Create a new reputation record with neutral score.
    fn new(peer_id: &str) -> Self {
        Self {
            peer_id: peer_id.to_string(),
            score: 0.0,
            positive_events: 0,
            negative_events: 0,
            first_seen: SystemTime::now(),
            last_updated: SystemTime::now(),
        }
    }

    /// Apply a reputation delta (positive or negative).
    fn apply(&mut self, delta: f64) {
        self.score += delta;
        self.last_updated = SystemTime::now();

        if delta > 0.0 {
            self.positive_events += 1;
        } else if delta < 0.0 {
            self.negative_events += 1;
        }
    }

    /// Whether this peer is considered trusted (above threshold).
    pub fn is_trusted(&self, threshold: f64) -> bool {
        self.score >= threshold
    }

    /// Whether this peer is considered hostile (below threshold).
    pub fn is_hostile(&self, threshold: f64) -> bool {
        self.score <= threshold
    }
}

/// Reputation engine configuration.
#[derive(Debug, Clone)]
pub struct ReputationConfig {
    /// Points gained per MB relayed.
    pub points_per_mb_relayed: f64,
    /// Points gained per GB·h stored.
    pub points_per_gbh_stored: f64,
    /// Points gained per kT computed.
    pub points_per_kt_computed: f64,
    /// Points gained per hour of uptime.
    pub points_per_hour_uptime: f64,
    /// Points gained per valid content delivery.
    pub points_per_valid_content: f64,
    /// Points lost per invalid data incident.
    pub penalty_invalid_data: f64,
    /// Points lost per rate limit violation.
    pub penalty_rate_limit: f64,
    /// Points lost per spam incident.
    pub penalty_spam: f64,
    /// Points lost per relay failure.
    pub penalty_relay_failure: f64,
    /// Points lost per transfer timeout.
    pub penalty_transfer_timeout: f64,
    /// Score above which a peer is considered trusted.
    pub trust_threshold: f64,
    /// Score below which a peer is considered hostile.
    pub hostile_threshold: f64,
}

impl Default for ReputationConfig {
    fn default() -> Self {
        Self {
            points_per_mb_relayed: 0.01,
            points_per_gbh_stored: 0.1,
            points_per_kt_computed: 0.05,
            points_per_hour_uptime: 0.5,
            points_per_valid_content: 1.0,
            penalty_invalid_data: -10.0,
            penalty_rate_limit: -5.0,
            penalty_spam: -20.0,
            penalty_relay_failure: -15.0,
            penalty_transfer_timeout: -3.0,
            trust_threshold: 10.0,
            hostile_threshold: -50.0,
        }
    }
}

/// The reputation engine — tracks and scores all known peers.
#[derive(Debug)]
pub struct ReputationEngine {
    /// Per-peer reputation records.
    peers: HashMap<String, PeerReputation>,
    /// Configuration.
    config: ReputationConfig,
}

impl ReputationEngine {
    /// Create a new reputation engine with the given configuration.
    pub fn new(config: ReputationConfig) -> Self {
        tracing::info!("Reputation engine initialised");
        Self {
            peers: HashMap::new(),
            config,
        }
    }

    /// Record a reputation event for a peer.
    pub fn record(&mut self, peer_id: &str, event: ReputationEvent) {
        let delta = self.score_event(&event);
        let rep = self.peers
            .entry(peer_id.to_string())
            .or_insert_with(|| PeerReputation::new(peer_id));

        rep.apply(delta);

        tracing::debug!(
            peer = peer_id,
            event = ?event,
            delta = delta,
            new_score = rep.score,
            "Reputation event recorded"
        );
    }

    /// Calculate the score delta for an event.
    fn score_event(&self, event: &ReputationEvent) -> f64 {
        match event {
            ReputationEvent::BandwidthRelayed { megabytes } => {
                megabytes * self.config.points_per_mb_relayed
            }
            ReputationEvent::StorageProvided { gigabyte_hours } => {
                gigabyte_hours * self.config.points_per_gbh_stored
            }
            ReputationEvent::ComputeProvided { kilo_tokens } => {
                kilo_tokens * self.config.points_per_kt_computed
            }
            ReputationEvent::UptimeContribution { hours } => {
                hours * self.config.points_per_hour_uptime
            }
            ReputationEvent::ValidContent => self.config.points_per_valid_content,
            ReputationEvent::InvalidData => self.config.penalty_invalid_data,
            ReputationEvent::RateLimitViolation => self.config.penalty_rate_limit,
            ReputationEvent::SpamDetected => self.config.penalty_spam,
            ReputationEvent::RelayFailure => self.config.penalty_relay_failure,
            ReputationEvent::TransferTimeout => self.config.penalty_transfer_timeout,
            ReputationEvent::ManualTrust { delta } => *delta,
        }
    }

    /// Get a peer's reputation.
    pub fn get_reputation(&self, peer_id: &str) -> Option<&PeerReputation> {
        self.peers.get(peer_id)
    }

    /// Check if a peer is trusted.
    pub fn is_trusted(&self, peer_id: &str) -> bool {
        self.peers.get(peer_id)
            .map(|r| r.is_trusted(self.config.trust_threshold))
            .unwrap_or(false)
    }

    /// Check if a peer is hostile.
    pub fn is_hostile(&self, peer_id: &str) -> bool {
        self.peers.get(peer_id)
            .map(|r| r.is_hostile(self.config.hostile_threshold))
            .unwrap_or(false)
    }

    /// Get the score for a peer (0.0 if unknown).
    pub fn score(&self, peer_id: &str) -> f64 {
        self.peers.get(peer_id)
            .map(|r| r.score)
            .unwrap_or(0.0)
    }

    /// List all peers sorted by reputation (highest first).
    pub fn leaderboard(&self) -> Vec<&PeerReputation> {
        let mut peers: Vec<&PeerReputation> = self.peers.values().collect();
        peers.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        peers
    }

    /// Count of known peers.
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Count of trusted peers.
    pub fn trusted_count(&self) -> usize {
        self.peers.values()
            .filter(|r| r.is_trusted(self.config.trust_threshold))
            .count()
    }

    /// Count of hostile peers.
    pub fn hostile_count(&self) -> usize {
        self.peers.values()
            .filter(|r| r.is_hostile(self.config.hostile_threshold))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_engine() -> ReputationEngine {
        ReputationEngine::new(ReputationConfig::default())
    }

    #[test]
    fn test_new_peer_has_neutral_score() {
        let engine = make_engine();
        assert_eq!(engine.score("unknown"), 0.0);
        assert!(!engine.is_trusted("unknown"));
        assert!(!engine.is_hostile("unknown"));
    }

    #[test]
    fn test_bandwidth_relay_increases_score() {
        let mut engine = make_engine();
        engine.record("alice", ReputationEvent::BandwidthRelayed { megabytes: 1000.0 });
        assert_eq!(engine.score("alice"), 10.0); // 1000 * 0.01
    }

    #[test]
    fn test_uptime_increases_score() {
        let mut engine = make_engine();
        engine.record("alice", ReputationEvent::UptimeContribution { hours: 24.0 });
        assert_eq!(engine.score("alice"), 12.0); // 24 * 0.5
    }

    #[test]
    fn test_spam_decreases_score() {
        let mut engine = make_engine();
        engine.record("spammer", ReputationEvent::SpamDetected);
        assert_eq!(engine.score("spammer"), -20.0);
    }

    #[test]
    fn test_trusted_threshold() {
        let mut engine = make_engine();
        engine.record("alice", ReputationEvent::UptimeContribution { hours: 20.0 });
        assert!(engine.is_trusted("alice")); // 10.0 >= threshold 10.0
    }

    #[test]
    fn test_hostile_threshold() {
        let mut engine = make_engine();
        engine.record("bad", ReputationEvent::SpamDetected);
        engine.record("bad", ReputationEvent::SpamDetected);
        engine.record("bad", ReputationEvent::InvalidData);
        // Score: -20 -20 -10 = -50
        assert!(engine.is_hostile("bad"));
    }

    #[test]
    fn test_manual_trust() {
        let mut engine = make_engine();
        engine.record("trusted_friend", ReputationEvent::ManualTrust { delta: 100.0 });
        assert_eq!(engine.score("trusted_friend"), 100.0);
    }

    #[test]
    fn test_mixed_events() {
        let mut engine = make_engine();
        engine.record("peer", ReputationEvent::UptimeContribution { hours: 48.0 }); // +24
        engine.record("peer", ReputationEvent::RateLimitViolation); // -5
        assert_eq!(engine.score("peer"), 19.0);

        let rep = engine.get_reputation("peer").unwrap();
        assert_eq!(rep.positive_events, 1);
        assert_eq!(rep.negative_events, 1);
    }

    #[test]
    fn test_leaderboard_sorted() {
        let mut engine = make_engine();
        engine.record("alice", ReputationEvent::UptimeContribution { hours: 20.0 }); // 10.0
        engine.record("bob", ReputationEvent::UptimeContribution { hours: 40.0 }); // 20.0
        engine.record("charlie", ReputationEvent::UptimeContribution { hours: 10.0 }); // 5.0

        let board = engine.leaderboard();
        assert_eq!(board[0].peer_id, "bob");
        assert_eq!(board[1].peer_id, "alice");
        assert_eq!(board[2].peer_id, "charlie");
    }

    #[test]
    fn test_peer_count() {
        let mut engine = make_engine();
        engine.record("a", ReputationEvent::ValidContent);
        engine.record("b", ReputationEvent::ValidContent);
        assert_eq!(engine.peer_count(), 2);
    }

    #[test]
    fn test_trusted_hostile_counts() {
        let mut engine = make_engine();
        engine.record("good", ReputationEvent::ManualTrust { delta: 100.0 });
        engine.record("bad", ReputationEvent::ManualTrust { delta: -100.0 });
        engine.record("neutral", ReputationEvent::ValidContent);

        assert_eq!(engine.trusted_count(), 1);
        assert_eq!(engine.hostile_count(), 1);
    }

    #[test]
    fn test_reputation_serializable() {
        let rep = PeerReputation::new("alice");
        let json = serde_json::to_string(&rep).unwrap();
        let recovered: PeerReputation = serde_json::from_str(&json).unwrap();
        assert_eq!(recovered.peer_id, "alice");
    }
}
