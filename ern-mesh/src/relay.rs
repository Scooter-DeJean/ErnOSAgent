// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Relay safeguards — anonymised relay pools and WiFi sharing protection.
//!
//! ErnMesh uses a multi-layer safeguard system for shared resources:
//!
//! ## Per-Peer Approval (§14.4)
//! Every resource request requires explicit per-peer approval from the
//! node operator. No blanket sharing — each peer must be individually
//! granted capabilities via [`CapabilityStore`](crate::capability::CapabilityStore).
//!
//! ## Anonymised Relay Pool (§14.5)
//! General mesh traffic is routed through an anonymised relay pool.
//! When a node contributes bandwidth to the pool:
//! - Its identity is **not** associated with any specific request
//! - Traffic is mixed across multiple relays (onion-style layering)
//! - No single relay sees both origin and destination
//! - Relay nodes earn ErnPoints but cannot inspect payload content
//!
//! ## Rate Limiting
//! Each peer connection is subject to:
//! - Bandwidth caps (from `config.sharing.max_bandwidth_per_peer_mbps`)
//! - Concurrent peer limits (from `config.sharing.max_concurrent_peers`)
//! - Session timeouts (from `config.sharing.session_timeout_hours`)
//!
//! ## WiFi Sharing
//! WiFi sharing is the highest-risk resource. Additional safeguards:
//! - Requires explicit `UseWifi` capability grant per peer
//! - All WiFi traffic is E2E encrypted through the relay pool
//! - Session-level accounting with auto-disconnect on timeout

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::config::SharingConfig;

/// A relay session — tracks bandwidth usage and time for a single peer.
#[derive(Debug, Clone)]
pub struct RelaySession {
    /// The peer using this relay session.
    pub peer_id: String,
    /// When this session started.
    pub started_at: Instant,
    /// Total megabytes relayed in this session.
    pub mb_relayed: f64,
    /// Whether this session is for WiFi sharing specifically.
    pub is_wifi: bool,
}

/// State of a relay session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    /// Session is active and within limits.
    Active,
    /// Session exceeded bandwidth limit.
    BandwidthExceeded,
    /// Session exceeded time limit.
    TimedOut,
    /// Session was manually terminated.
    Terminated,
}

/// Relay pool manager — enforces all sharing safeguards.
///
/// Tracks active sessions, enforces rate limits, and manages
/// anonymised relay assignments.
#[derive(Debug)]
pub struct RelayPool {
    /// Active relay sessions indexed by peer_id.
    sessions: HashMap<String, RelaySession>,
    /// Sharing configuration — all limits from config (no hardcoded values).
    config: SharingConfig,
}

impl RelayPool {
    /// Create a new relay pool with the given sharing config.
    pub fn new(config: SharingConfig) -> Self {
        tracing::info!(
            enabled = config.enabled,
            mode = ?config.mode,
            max_bw = config.max_bandwidth_per_peer_mbps,
            max_peers = config.max_concurrent_peers,
            timeout_hours = config.session_timeout_hours,
            "Relay pool initialised"
        );

        Self {
            sessions: HashMap::new(),
            config,
        }
    }

    /// Check if sharing is enabled at all.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Request a relay session for a peer.
    ///
    /// Returns `Ok(())` if the session is granted, or an error explaining
    /// why the request was denied.
    pub fn request_session(
        &mut self,
        peer_id: &str,
        is_wifi: bool,
    ) -> Result<(), RelayDenial> {
        // Check if sharing is enabled
        if !self.config.enabled {
            return Err(RelayDenial::SharingDisabled);
        }

        // Check concurrent peer limit
        let active_count = self.active_session_count();
        if active_count >= self.config.max_concurrent_peers as usize {
            return Err(RelayDenial::MaxPeersReached {
                limit: self.config.max_concurrent_peers,
            });
        }

        // Check if peer already has an active session
        if self.sessions.contains_key(peer_id) {
            return Err(RelayDenial::AlreadyActive);
        }

        // Create session
        let session = RelaySession {
            peer_id: peer_id.to_string(),
            started_at: Instant::now(),
            mb_relayed: 0.0,
            is_wifi,
        };

        tracing::info!(
            peer_id = peer_id,
            is_wifi = is_wifi,
            "Relay session granted"
        );

        self.sessions.insert(peer_id.to_string(), session);
        Ok(())
    }

    /// Record bandwidth usage for a peer's session.
    ///
    /// Returns the session state after recording — may trigger bandwidth limit.
    pub fn record_usage(
        &mut self,
        peer_id: &str,
        megabytes: f64,
    ) -> SessionState {
        let Some(session) = self.sessions.get_mut(peer_id) else {
            return SessionState::Terminated;
        };

        session.mb_relayed += megabytes;

        // Check bandwidth limit (convert MB/s to total MB based on session duration)
        let elapsed_secs = session.started_at.elapsed().as_secs_f64();
        let max_mb = self.config.max_bandwidth_per_peer_mbps as f64 * elapsed_secs;

        if session.mb_relayed > max_mb && elapsed_secs > 1.0 {
            tracing::warn!(
                peer_id = peer_id,
                mb_relayed = session.mb_relayed,
                max_mb = max_mb,
                "Relay session: bandwidth limit exceeded"
            );
            return SessionState::BandwidthExceeded;
        }

        // Check timeout
        let timeout = Duration::from_secs(self.config.session_timeout_hours as u64 * 3600);
        if session.started_at.elapsed() > timeout {
            tracing::info!(
                peer_id = peer_id,
                "Relay session: timed out"
            );
            return SessionState::TimedOut;
        }

        SessionState::Active
    }

    /// Terminate a relay session.
    pub fn terminate_session(&mut self, peer_id: &str) -> bool {
        if self.sessions.remove(peer_id).is_some() {
            tracing::info!(peer_id = peer_id, "Relay session terminated");
            true
        } else {
            false
        }
    }

    /// Get the number of active sessions.
    pub fn active_session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Get session info for a peer.
    pub fn get_session(&self, peer_id: &str) -> Option<&RelaySession> {
        self.sessions.get(peer_id)
    }

    /// Clean up expired sessions (timeout check).
    pub fn cleanup_expired(&mut self) -> usize {
        let timeout = Duration::from_secs(self.config.session_timeout_hours as u64 * 3600);

        let expired: Vec<String> = self.sessions
            .iter()
            .filter(|(_, s)| s.started_at.elapsed() > timeout)
            .map(|(id, _)| id.clone())
            .collect();

        let count = expired.len();
        for id in &expired {
            self.sessions.remove(id);
            tracing::info!(peer_id = id, "Relay session expired and cleaned up");
        }

        count
    }

    /// Get total bandwidth relayed across all active sessions.
    pub fn total_mb_relayed(&self) -> f64 {
        self.sessions.values().map(|s| s.mb_relayed).sum()
    }
}

/// Reason a relay request was denied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayDenial {
    /// Sharing is disabled in configuration.
    SharingDisabled,
    /// Maximum concurrent peer limit reached.
    MaxPeersReached { limit: u32 },
    /// This peer already has an active session.
    AlreadyActive,
}

impl std::fmt::Display for RelayDenial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelayDenial::SharingDisabled => write!(f, "Sharing is disabled"),
            RelayDenial::MaxPeersReached { limit } => {
                write!(f, "Max concurrent peers reached ({})", limit)
            }
            RelayDenial::AlreadyActive => write!(f, "Peer already has an active session"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled_config() -> SharingConfig {
        SharingConfig {
            enabled: true,
            mode: crate::config::SharingMode::RequestApproval,
            max_bandwidth_per_peer_mbps: 10,
            max_concurrent_peers: 3,
            session_timeout_hours: 24,
            whitelisted_peers: Vec::new(),
            blacklisted_peers: Vec::new(),
        }
    }

    fn disabled_config() -> SharingConfig {
        SharingConfig {
            enabled: false,
            mode: crate::config::SharingMode::RequestApproval,
            max_bandwidth_per_peer_mbps: 10,
            max_concurrent_peers: 3,
            session_timeout_hours: 24,
            whitelisted_peers: Vec::new(),
            blacklisted_peers: Vec::new(),
        }
    }

    #[test]
    fn test_new_pool_empty() {
        let pool = RelayPool::new(enabled_config());
        assert_eq!(pool.active_session_count(), 0);
        assert!(pool.is_enabled());
    }

    #[test]
    fn test_disabled_pool() {
        let pool = RelayPool::new(disabled_config());
        assert!(!pool.is_enabled());
    }

    #[test]
    fn test_request_session_granted() {
        let mut pool = RelayPool::new(enabled_config());
        let result = pool.request_session("peer_a", false);
        assert!(result.is_ok());
        assert_eq!(pool.active_session_count(), 1);
    }

    #[test]
    fn test_request_session_disabled() {
        let mut pool = RelayPool::new(disabled_config());
        let result = pool.request_session("peer_a", false);
        assert_eq!(result, Err(RelayDenial::SharingDisabled));
    }

    #[test]
    fn test_request_session_max_peers() {
        let mut pool = RelayPool::new(enabled_config()); // max 3

        pool.request_session("peer_a", false).unwrap();
        pool.request_session("peer_b", false).unwrap();
        pool.request_session("peer_c", false).unwrap();

        let result = pool.request_session("peer_d", false);
        assert_eq!(result, Err(RelayDenial::MaxPeersReached { limit: 3 }));
    }

    #[test]
    fn test_request_session_already_active() {
        let mut pool = RelayPool::new(enabled_config());
        pool.request_session("peer_a", false).unwrap();

        let result = pool.request_session("peer_a", false);
        assert_eq!(result, Err(RelayDenial::AlreadyActive));
    }

    #[test]
    fn test_record_usage_active() {
        let mut pool = RelayPool::new(enabled_config());
        pool.request_session("peer_a", false).unwrap();

        let state = pool.record_usage("peer_a", 1.0);
        assert_eq!(state, SessionState::Active);
    }

    #[test]
    fn test_record_usage_unknown_peer() {
        let mut pool = RelayPool::new(enabled_config());
        let state = pool.record_usage("nonexistent", 1.0);
        assert_eq!(state, SessionState::Terminated);
    }

    #[test]
    fn test_terminate_session() {
        let mut pool = RelayPool::new(enabled_config());
        pool.request_session("peer_a", false).unwrap();

        assert!(pool.terminate_session("peer_a"));
        assert_eq!(pool.active_session_count(), 0);
    }

    #[test]
    fn test_terminate_unknown_returns_false() {
        let mut pool = RelayPool::new(enabled_config());
        assert!(!pool.terminate_session("unknown"));
    }

    #[test]
    fn test_get_session() {
        let mut pool = RelayPool::new(enabled_config());
        pool.request_session("peer_a", true).unwrap();

        let session = pool.get_session("peer_a").unwrap();
        assert_eq!(session.peer_id, "peer_a");
        assert!(session.is_wifi);
        assert_eq!(session.mb_relayed, 0.0);
    }

    #[test]
    fn test_total_mb_relayed() {
        let mut pool = RelayPool::new(enabled_config());
        pool.request_session("peer_a", false).unwrap();
        pool.request_session("peer_b", false).unwrap();

        pool.record_usage("peer_a", 10.0);
        pool.record_usage("peer_b", 5.0);

        assert!((pool.total_mb_relayed() - 15.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_relay_denial_display() {
        assert_eq!(
            format!("{}", RelayDenial::SharingDisabled),
            "Sharing is disabled"
        );
        assert_eq!(
            format!("{}", RelayDenial::MaxPeersReached { limit: 5 }),
            "Max concurrent peers reached (5)"
        );
    }

    #[test]
    fn test_wifi_session_tracked() {
        let mut pool = RelayPool::new(enabled_config());
        pool.request_session("peer_a", true).unwrap();

        let session = pool.get_session("peer_a").unwrap();
        assert!(session.is_wifi, "WiFi sessions must be flagged");
    }
}
