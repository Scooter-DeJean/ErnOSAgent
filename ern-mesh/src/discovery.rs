// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Peer discovery — Kademlia DHT bootstrap and mDNS local discovery.
//!
//! ErnMesh uses a hybrid discovery strategy per §7.1:
//!
//! - **mDNS**: Discovers peers on the local network (same LAN/room/house).
//!   Zero-configuration, works without internet connectivity.
//! - **Kademlia DHT**: Discovers peers globally via distributed hash table.
//!   Bootstrap peers (Founders' Nodes) are pre-configured in `ern-os.toml`.
//!
//! Both discovery methods run simultaneously. Discovered peers are fed into
//! the Kademlia routing table for future lookups.
//!
//! This module provides discovery configuration and peer tracking.
//! The actual Kademlia/mDNS behaviour instances are created by `node.rs`
//! (PR 7) when building the libp2p `Swarm`.

use libp2p_identity::PeerId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;

/// Configuration for peer discovery.
///
/// Derived from `MeshConfig.bootstrap_peers` and internal defaults.
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    /// Parsed bootstrap peer multiaddrs for Kademlia seeding.
    pub bootstrap_addrs: Vec<libp2p::Multiaddr>,

    /// Whether mDNS is enabled for local network discovery.
    pub mdns_enabled: bool,

    /// Kademlia protocol name — must match across all ErnMesh nodes.
    pub kademlia_protocol: String,
}

impl DiscoveryConfig {
    /// Create discovery config from the mesh configuration.
    ///
    /// Parses bootstrap peer addresses and sets defaults for discovery.
    pub fn from_mesh_config(config: &crate::config::MeshConfig) -> Self {
        let bootstrap_addrs = crate::transport::parse_bootstrap_addrs(config);

        tracing::info!(
            bootstrap_count = bootstrap_addrs.len(),
            mdns = true,
            "Discovery configuration ready"
        );

        Self {
            bootstrap_addrs,
            mdns_enabled: true,
            kademlia_protocol: "/ernmesh/kad/1.0.0".to_string(),
        }
    }
}

/// Discovery method — how a peer was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiscoveryMethod {
    /// Found via mDNS on the local network.
    Mdns,
    /// Found via Kademlia DHT lookup.
    Kademlia,
    /// Explicitly configured as a bootstrap peer.
    Bootstrap,
    /// Received via peer exchange (another peer told us about them).
    PeerExchange,
}

impl std::fmt::Display for DiscoveryMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoveryMethod::Mdns => write!(f, "mDNS"),
            DiscoveryMethod::Kademlia => write!(f, "Kademlia"),
            DiscoveryMethod::Bootstrap => write!(f, "Bootstrap"),
            DiscoveryMethod::PeerExchange => write!(f, "PeerExchange"),
        }
    }
}

/// Information about a discovered peer.
#[derive(Debug, Clone)]
pub struct DiscoveredPeer {
    /// The peer's unique identifier.
    pub peer_id: PeerId,

    /// Known multiaddrs for reaching this peer.
    pub addrs: Vec<libp2p::Multiaddr>,

    /// How this peer was discovered.
    pub method: DiscoveryMethod,

    /// When this peer was first discovered.
    pub discovered_at: Instant,

    /// When this peer was last seen (refreshed on any activity).
    pub last_seen: Instant,
}

/// Tracks all discovered peers and their metadata.
///
/// Used by the node to maintain a view of the mesh topology.
/// Thread-safety note: wrap in `RwLock` if shared across tasks.
#[derive(Debug, Default)]
pub struct PeerRegistry {
    /// Maps PeerId → discovered peer info.
    peers: HashMap<PeerId, DiscoveredPeer>,
}

impl PeerRegistry {
    /// Create a new, empty peer registry.
    pub fn new() -> Self {
        Self {
            peers: HashMap::new(),
        }
    }

    /// Register a newly discovered peer.
    ///
    /// If the peer is already known, updates its addresses and last-seen time.
    pub fn add_peer(
        &mut self,
        peer_id: PeerId,
        addrs: Vec<libp2p::Multiaddr>,
        method: DiscoveryMethod,
    ) {
        let now = Instant::now();

        if let Some(existing) = self.peers.get_mut(&peer_id) {
            // Merge new addresses
            for addr in &addrs {
                if !existing.addrs.contains(addr) {
                    existing.addrs.push(addr.clone());
                }
            }
            existing.last_seen = now;

            tracing::debug!(
                peer_id = %peer_id,
                method = %method,
                addrs = existing.addrs.len(),
                "Updated existing peer"
            );
        } else {
            tracing::info!(
                peer_id = %peer_id,
                method = %method,
                addrs = addrs.len(),
                "Discovered new peer"
            );

            self.peers.insert(peer_id, DiscoveredPeer {
                peer_id,
                addrs,
                method,
                discovered_at: now,
                last_seen: now,
            });
        }
    }

    /// Remove a peer from the registry (e.g., on disconnect).
    pub fn remove_peer(&mut self, peer_id: &PeerId) -> bool {
        if self.peers.remove(peer_id).is_some() {
            tracing::info!(peer_id = %peer_id, "Removed peer from registry");
            true
        } else {
            false
        }
    }

    /// Mark a peer as recently seen (refresh last_seen timestamp).
    pub fn touch(&mut self, peer_id: &PeerId) {
        if let Some(peer) = self.peers.get_mut(peer_id) {
            peer.last_seen = Instant::now();
        }
    }

    /// Look up a peer by PeerId.
    pub fn get(&self, peer_id: &PeerId) -> Option<&DiscoveredPeer> {
        self.peers.get(peer_id)
    }

    /// Returns the total number of known peers.
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Returns all known PeerIds.
    pub fn peer_ids(&self) -> Vec<PeerId> {
        self.peers.keys().copied().collect()
    }

    /// Returns peers discovered via a specific method.
    pub fn peers_by_method(&self, method: DiscoveryMethod) -> Vec<&DiscoveredPeer> {
        self.peers
            .values()
            .filter(|p| p.method == method)
            .collect()
    }

    /// Returns `true` if the peer is known.
    pub fn contains(&self, peer_id: &PeerId) -> bool {
        self.peers.contains_key(peer_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a random PeerId for testing.
    fn random_peer_id() -> PeerId {
        PeerId::random()
    }

    fn test_addr() -> libp2p::Multiaddr {
        "/ip4/127.0.0.1/tcp/4001".parse().unwrap()
    }

    #[test]
    fn test_new_registry_is_empty() {
        let registry = PeerRegistry::new();
        assert_eq!(registry.peer_count(), 0);
        assert!(registry.peer_ids().is_empty());
    }

    #[test]
    fn test_add_peer() {
        let mut registry = PeerRegistry::new();
        let pid = random_peer_id();

        registry.add_peer(pid, vec![test_addr()], DiscoveryMethod::Mdns);

        assert_eq!(registry.peer_count(), 1);
        assert!(registry.contains(&pid));
    }

    #[test]
    fn test_add_peer_preserves_method() {
        let mut registry = PeerRegistry::new();
        let pid = random_peer_id();

        registry.add_peer(pid, vec![test_addr()], DiscoveryMethod::Kademlia);

        let peer = registry.get(&pid).unwrap();
        assert_eq!(peer.method, DiscoveryMethod::Kademlia);
    }

    #[test]
    fn test_add_existing_peer_merges_addrs() {
        let mut registry = PeerRegistry::new();
        let pid = random_peer_id();
        let addr1: libp2p::Multiaddr = "/ip4/1.2.3.4/tcp/4001".parse().unwrap();
        let addr2: libp2p::Multiaddr = "/ip4/5.6.7.8/tcp/4001".parse().unwrap();

        registry.add_peer(pid, vec![addr1.clone()], DiscoveryMethod::Mdns);
        registry.add_peer(pid, vec![addr2.clone()], DiscoveryMethod::Kademlia);

        let peer = registry.get(&pid).unwrap();
        assert_eq!(peer.addrs.len(), 2);
        assert!(peer.addrs.contains(&addr1));
        assert!(peer.addrs.contains(&addr2));
    }

    #[test]
    fn test_add_existing_peer_no_duplicate_addrs() {
        let mut registry = PeerRegistry::new();
        let pid = random_peer_id();
        let addr = test_addr();

        registry.add_peer(pid, vec![addr.clone()], DiscoveryMethod::Mdns);
        registry.add_peer(pid, vec![addr], DiscoveryMethod::Mdns);

        let peer = registry.get(&pid).unwrap();
        assert_eq!(peer.addrs.len(), 1, "Duplicate addresses must not be added");
    }

    #[test]
    fn test_remove_peer() {
        let mut registry = PeerRegistry::new();
        let pid = random_peer_id();

        registry.add_peer(pid, vec![test_addr()], DiscoveryMethod::Mdns);
        assert!(registry.remove_peer(&pid));
        assert!(!registry.contains(&pid));
        assert_eq!(registry.peer_count(), 0);
    }

    #[test]
    fn test_remove_unknown_peer_returns_false() {
        let mut registry = PeerRegistry::new();
        assert!(!registry.remove_peer(&random_peer_id()));
    }

    #[test]
    fn test_get_unknown_peer_returns_none() {
        let registry = PeerRegistry::new();
        assert!(registry.get(&random_peer_id()).is_none());
    }

    #[test]
    fn test_peers_by_method() {
        let mut registry = PeerRegistry::new();

        registry.add_peer(random_peer_id(), vec![test_addr()], DiscoveryMethod::Mdns);
        registry.add_peer(random_peer_id(), vec![test_addr()], DiscoveryMethod::Mdns);
        registry.add_peer(random_peer_id(), vec![test_addr()], DiscoveryMethod::Kademlia);

        let mdns_peers = registry.peers_by_method(DiscoveryMethod::Mdns);
        assert_eq!(mdns_peers.len(), 2);

        let kad_peers = registry.peers_by_method(DiscoveryMethod::Kademlia);
        assert_eq!(kad_peers.len(), 1);

        let bootstrap_peers = registry.peers_by_method(DiscoveryMethod::Bootstrap);
        assert!(bootstrap_peers.is_empty());
    }

    #[test]
    fn test_peer_ids_returns_all() {
        let mut registry = PeerRegistry::new();
        let pid_a = random_peer_id();
        let pid_b = random_peer_id();

        registry.add_peer(pid_a, vec![test_addr()], DiscoveryMethod::Mdns);
        registry.add_peer(pid_b, vec![test_addr()], DiscoveryMethod::Kademlia);

        let ids = registry.peer_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&pid_a));
        assert!(ids.contains(&pid_b));
    }

    #[test]
    fn test_touch_updates_last_seen() {
        let mut registry = PeerRegistry::new();
        let pid = random_peer_id();

        registry.add_peer(pid, vec![test_addr()], DiscoveryMethod::Mdns);
        let first_seen = registry.get(&pid).unwrap().last_seen;

        // Small sleep to ensure timestamp changes
        std::thread::sleep(std::time::Duration::from_millis(10));
        registry.touch(&pid);

        let after_touch = registry.get(&pid).unwrap().last_seen;
        assert!(after_touch > first_seen, "touch must update last_seen");
    }

    #[test]
    fn test_discovery_method_display() {
        assert_eq!(format!("{}", DiscoveryMethod::Mdns), "mDNS");
        assert_eq!(format!("{}", DiscoveryMethod::Kademlia), "Kademlia");
        assert_eq!(format!("{}", DiscoveryMethod::Bootstrap), "Bootstrap");
        assert_eq!(format!("{}", DiscoveryMethod::PeerExchange), "PeerExchange");
    }

    #[test]
    fn test_discovery_config_from_mesh_config() {
        let toml = r#"
[mesh]
enabled = true
listen_port = 4001
max_connections = 128
share_bandwidth = true
share_storage_gb = 50
bootstrap_peers = ["/ip4/1.2.3.4/tcp/4001"]

[mesh.economy]
points_per_mb_relayed = 1.0
points_per_gb_hour_stored = 0.5
points_per_1k_tokens_computed = 2.0
points_per_mb_wifi_shared = 0.1
points_per_hour_uptime = 1.0
chunk_size_bytes = 262144
cost_per_1k_tokens_inference = 5.0
cost_per_gb_month_storage = 10.0
cost_per_session_priority = 2.0
cost_per_replica_month = 3.0

[mesh.security]
gossip_score_threshold = -100.0
gossip_graylist_threshold = -1000.0
gossip_score_retention_secs = 3600
max_peers_per_ip = 3
max_peers_per_subnet = 10
max_inbound_connections = 128
max_outbound_connections = 128
max_messages_per_peer_per_sec = 50
max_payload_bytes = 1048576
max_pending_requests_per_peer = 10

[mesh.sharing]
enabled = false
mode = "request-approval"
max_bandwidth_per_peer_mbps = 10
max_concurrent_peers = 5
session_timeout_hours = 24
        "#;

        let mesh_config = crate::config::parse_mesh_config(toml).unwrap();
        let disco = DiscoveryConfig::from_mesh_config(&mesh_config);

        assert_eq!(disco.bootstrap_addrs.len(), 1);
        assert!(disco.mdns_enabled);
        assert_eq!(disco.kademlia_protocol, "/ernmesh/kad/1.0.0");
    }

    #[test]
    fn test_discovery_method_serialize_roundtrip() {
        let method = DiscoveryMethod::PeerExchange;
        let json = serde_json::to_string(&method).unwrap();
        let recovered: DiscoveryMethod = serde_json::from_str(&json).unwrap();
        assert_eq!(method, recovered);
    }
}
