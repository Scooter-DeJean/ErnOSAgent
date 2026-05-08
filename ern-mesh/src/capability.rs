// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Capability-based access control — per-peer permission grants and enforcement.
//!
//! ErnMesh uses a capability model per §14.4: every peer starts with **zero**
//! capabilities and must be explicitly granted permissions. Capabilities are:
//!
//! - **Scoped**: each capability targets a specific resource or action.
//! - **Revocable**: can be revoked at any time by the node operator.
//! - **Non-transitive**: a peer cannot delegate its capabilities to others.
//!
//! The node operator manages capabilities through the WebUI or `ern-os.toml`.
//! The [`CapabilityStore`] enforces access checks at runtime before any
//! resource is shared with a peer.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A specific capability that can be granted to a peer.
///
/// Each variant represents a distinct action or resource a peer may access.
/// Default for all peers: **none**. Capabilities must be explicitly granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Relay bandwidth through this node.
    RelayBandwidth,

    /// Read content from this node's content-addressable store.
    ReadContent,

    /// Pin (store) content on this node.
    PinContent,

    /// Request inference compute from this node.
    RequestCompute,

    /// Access shared WiFi resources.
    UseWifi,

    /// Query mesh metadata (peer list, topic list, etc.).
    QueryMetadata,

    /// Publish to Gossipsub topics hosted on this node.
    PublishTopics,

    /// Request direct file transfers from this node.
    FileTransfer,
}

impl Capability {
    /// Returns all defined capabilities.
    pub fn all() -> &'static [Capability] {
        &[
            Capability::RelayBandwidth,
            Capability::ReadContent,
            Capability::PinContent,
            Capability::RequestCompute,
            Capability::UseWifi,
            Capability::QueryMetadata,
            Capability::PublishTopics,
            Capability::FileTransfer,
        ]
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Capability::RelayBandwidth => write!(f, "relay_bandwidth"),
            Capability::ReadContent => write!(f, "read_content"),
            Capability::PinContent => write!(f, "pin_content"),
            Capability::RequestCompute => write!(f, "request_compute"),
            Capability::UseWifi => write!(f, "use_wifi"),
            Capability::QueryMetadata => write!(f, "query_metadata"),
            Capability::PublishTopics => write!(f, "publish_topics"),
            Capability::FileTransfer => write!(f, "file_transfer"),
        }
    }
}

/// Result of a capability check.
#[derive(Debug, PartialEq, Eq)]
pub enum AccessDecision {
    /// The peer has the requested capability — proceed.
    Granted,
    /// The peer does not have the requested capability — deny and log.
    Denied {
        /// The peer that was denied.
        peer: String,
        /// The capability that was missing.
        capability: Capability,
    },
}

impl AccessDecision {
    /// Returns `true` if access is granted.
    pub fn is_granted(&self) -> bool {
        matches!(self, AccessDecision::Granted)
    }
}

/// Runtime capability store — tracks per-peer capability grants.
///
/// All peers start with zero capabilities. Capabilities are granted
/// explicitly by the node operator and can be revoked at any time.
///
/// Thread-safety note: this struct is not `Sync` — wrap in a `Mutex`
/// or `RwLock` if shared across async tasks.
#[derive(Debug, Default)]
pub struct CapabilityStore {
    /// Maps PeerId (base58) → set of granted capabilities.
    grants: HashMap<String, HashSet<Capability>>,
}

impl CapabilityStore {
    /// Create a new, empty capability store (all peers start with nothing).
    pub fn new() -> Self {
        Self {
            grants: HashMap::new(),
        }
    }

    /// Grant a capability to a peer.
    ///
    /// Idempotent — granting an already-held capability is a no-op.
    pub fn grant(&mut self, peer_id: &str, capability: Capability) {
        self.grants
            .entry(peer_id.to_string())
            .or_default()
            .insert(capability);

        tracing::info!(
            peer_id = peer_id,
            capability = %capability,
            "Granted capability to peer"
        );
    }

    /// Grant multiple capabilities to a peer at once.
    pub fn grant_many(&mut self, peer_id: &str, capabilities: &[Capability]) {
        for cap in capabilities {
            self.grant(peer_id, *cap);
        }
    }

    /// Revoke a capability from a peer.
    ///
    /// Idempotent — revoking an un-held capability is a no-op.
    pub fn revoke(&mut self, peer_id: &str, capability: Capability) {
        if let Some(caps) = self.grants.get_mut(peer_id) {
            caps.remove(&capability);

            tracing::info!(
                peer_id = peer_id,
                capability = %capability,
                "Revoked capability from peer"
            );

            // Clean up empty entries
            if caps.is_empty() {
                self.grants.remove(peer_id);
            }
        }
    }

    /// Revoke all capabilities from a peer.
    pub fn revoke_all(&mut self, peer_id: &str) {
        if self.grants.remove(peer_id).is_some() {
            tracing::info!(
                peer_id = peer_id,
                "Revoked ALL capabilities from peer"
            );
        }
    }

    /// Check if a peer has a specific capability.
    ///
    /// Returns an [`AccessDecision`] — either `Granted` or `Denied`
    /// with diagnostic information. Always logs denied access at `warn` level.
    pub fn check(&self, peer_id: &str, capability: Capability) -> AccessDecision {
        let has_cap = self.grants
            .get(peer_id)
            .map(|caps| caps.contains(&capability))
            .unwrap_or(false);

        if has_cap {
            AccessDecision::Granted
        } else {
            tracing::warn!(
                peer_id = peer_id,
                capability = %capability,
                "Access DENIED — peer lacks required capability"
            );
            AccessDecision::Denied {
                peer: peer_id.to_string(),
                capability,
            }
        }
    }

    /// Returns all capabilities currently held by a peer.
    pub fn capabilities_for(&self, peer_id: &str) -> Vec<Capability> {
        self.grants
            .get(peer_id)
            .map(|caps| caps.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Returns the number of peers with at least one capability.
    pub fn peer_count(&self) -> usize {
        self.grants.len()
    }

    /// Returns `true` if the peer has any capabilities at all.
    pub fn is_known(&self, peer_id: &str) -> bool {
        self.grants.contains_key(peer_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_store_has_no_peers() {
        let store = CapabilityStore::new();
        assert_eq!(store.peer_count(), 0);
    }

    #[test]
    fn test_default_peer_has_no_capabilities() {
        let store = CapabilityStore::new();
        let decision = store.check("peer_a", Capability::RelayBandwidth);
        assert!(!decision.is_granted());

        match decision {
            AccessDecision::Denied { peer, capability } => {
                assert_eq!(peer, "peer_a");
                assert_eq!(capability, Capability::RelayBandwidth);
            }
            _ => panic!("Expected Denied"),
        }
    }

    #[test]
    fn test_grant_and_check() {
        let mut store = CapabilityStore::new();
        store.grant("peer_a", Capability::ReadContent);

        assert!(store.check("peer_a", Capability::ReadContent).is_granted());
    }

    #[test]
    fn test_grant_does_not_affect_other_capabilities() {
        let mut store = CapabilityStore::new();
        store.grant("peer_a", Capability::ReadContent);

        assert!(!store.check("peer_a", Capability::PinContent).is_granted());
        assert!(!store.check("peer_a", Capability::UseWifi).is_granted());
    }

    #[test]
    fn test_grant_idempotent() {
        let mut store = CapabilityStore::new();
        store.grant("peer_a", Capability::ReadContent);
        store.grant("peer_a", Capability::ReadContent);

        let caps = store.capabilities_for("peer_a");
        assert_eq!(caps.len(), 1);
    }

    #[test]
    fn test_grant_many() {
        let mut store = CapabilityStore::new();
        store.grant_many("peer_a", &[
            Capability::ReadContent,
            Capability::PinContent,
            Capability::QueryMetadata,
        ]);

        assert!(store.check("peer_a", Capability::ReadContent).is_granted());
        assert!(store.check("peer_a", Capability::PinContent).is_granted());
        assert!(store.check("peer_a", Capability::QueryMetadata).is_granted());
        assert!(!store.check("peer_a", Capability::UseWifi).is_granted());
    }

    #[test]
    fn test_revoke_removes_capability() {
        let mut store = CapabilityStore::new();
        store.grant("peer_a", Capability::ReadContent);
        store.grant("peer_a", Capability::PinContent);

        store.revoke("peer_a", Capability::ReadContent);

        assert!(!store.check("peer_a", Capability::ReadContent).is_granted());
        assert!(store.check("peer_a", Capability::PinContent).is_granted());
    }

    #[test]
    fn test_revoke_idempotent() {
        let mut store = CapabilityStore::new();
        store.revoke("peer_a", Capability::ReadContent); // no-op
        assert_eq!(store.peer_count(), 0);
    }

    #[test]
    fn test_revoke_all() {
        let mut store = CapabilityStore::new();
        store.grant_many("peer_a", &[
            Capability::ReadContent,
            Capability::PinContent,
            Capability::UseWifi,
        ]);

        store.revoke_all("peer_a");

        assert!(!store.is_known("peer_a"));
        assert_eq!(store.peer_count(), 0);
    }

    #[test]
    fn test_revoke_last_capability_cleans_up_peer() {
        let mut store = CapabilityStore::new();
        store.grant("peer_a", Capability::ReadContent);
        store.revoke("peer_a", Capability::ReadContent);

        assert!(!store.is_known("peer_a"));
        assert_eq!(store.peer_count(), 0);
    }

    #[test]
    fn test_capabilities_independent_per_peer() {
        let mut store = CapabilityStore::new();
        store.grant("peer_a", Capability::ReadContent);
        store.grant("peer_b", Capability::UseWifi);

        assert!(store.check("peer_a", Capability::ReadContent).is_granted());
        assert!(!store.check("peer_a", Capability::UseWifi).is_granted());

        assert!(!store.check("peer_b", Capability::ReadContent).is_granted());
        assert!(store.check("peer_b", Capability::UseWifi).is_granted());

        assert_eq!(store.peer_count(), 2);
    }

    #[test]
    fn test_capabilities_for_returns_all_granted() {
        let mut store = CapabilityStore::new();
        store.grant("peer_a", Capability::ReadContent);
        store.grant("peer_a", Capability::PinContent);

        let caps = store.capabilities_for("peer_a");
        assert_eq!(caps.len(), 2);
        assert!(caps.contains(&Capability::ReadContent));
        assert!(caps.contains(&Capability::PinContent));
    }

    #[test]
    fn test_capabilities_for_unknown_peer_returns_empty() {
        let store = CapabilityStore::new();
        let caps = store.capabilities_for("unknown");
        assert!(caps.is_empty());
    }

    #[test]
    fn test_capability_all_covers_every_variant() {
        let all = Capability::all();
        assert_eq!(all.len(), 8, "All defined capabilities must be listed");
    }

    #[test]
    fn test_capability_display() {
        assert_eq!(format!("{}", Capability::RelayBandwidth), "relay_bandwidth");
        assert_eq!(format!("{}", Capability::UseWifi), "use_wifi");
        assert_eq!(format!("{}", Capability::FileTransfer), "file_transfer");
    }

    #[test]
    fn test_capability_serialize_roundtrip() {
        let cap = Capability::RequestCompute;
        let json = serde_json::to_string(&cap).unwrap();
        let recovered: Capability = serde_json::from_str(&json).unwrap();
        assert_eq!(cap, recovered);
    }

    #[test]
    fn test_access_decision_denied_contains_diagnostics() {
        let decision = AccessDecision::Denied {
            peer: "peer_x".to_string(),
            capability: Capability::PinContent,
        };

        assert!(!decision.is_granted());
        match decision {
            AccessDecision::Denied { peer, capability } => {
                assert_eq!(peer, "peer_x");
                assert_eq!(capability, Capability::PinContent);
            }
            _ => unreachable!(),
        }
    }
}
