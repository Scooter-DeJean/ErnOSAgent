// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Mesh dashboard — aggregated mesh status for the WebUI.
//!
//! Provides a single endpoint that returns the complete mesh state
//! in one API call, optimised for the WebUI dashboard panel.
//!
//! The dashboard combines data from:
//! - MeshNode (PeerId, status, port)
//! - PeerRegistry (connected peers)
//! - CapabilityStore (granted capabilities)
//! - Economy Ledger (balance, recent transactions)
//! - RelayPool (active sessions)
//! - ChatLog (message counts)
//! - ContentStore (storage usage)
//! - ErnieBook Feed (post count)

use serde::Serialize;

/// Complete mesh dashboard state — serialised as JSON for the WebUI.
#[derive(Debug, Serialize)]
pub struct MeshDashboard {
    /// Whether the mesh is configured and active.
    pub enabled: bool,
    /// Current node status (e.g., "Running", "Stopped").
    pub status: String,
    /// This node's PeerId (base58).
    pub peer_id: String,
    /// Listen port.
    pub listen_port: u16,

    /// Network section.
    pub network: NetworkStats,
    /// Economy section.
    pub economy: EconomyStats,
    /// Services section.
    pub services: ServiceStats,
    /// Security section.
    pub security: SecurityStats,
}

/// Network connectivity statistics.
#[derive(Debug, Serialize)]
pub struct NetworkStats {
    /// Number of connected peers.
    pub connected_peers: usize,
    /// Number of discovered (known) peers.
    pub discovered_peers: usize,
    /// Active relay sessions.
    pub active_relays: usize,
    /// Total bandwidth relayed (MB).
    pub total_mb_relayed: f64,
}

/// Economy statistics.
#[derive(Debug, Serialize)]
pub struct EconomyStats {
    /// Current ErnPoints balance.
    pub balance: f64,
    /// Total transactions (lifetime).
    pub total_transactions: usize,
    /// Total points earned (lifetime).
    pub total_earned: f64,
    /// Total points spent (lifetime).
    pub total_spent: f64,
}

/// Service statistics.
#[derive(Debug, Serialize)]
pub struct ServiceStats {
    /// Total chat messages in local log.
    pub chat_messages: usize,
    /// Active chat topics.
    pub chat_topics: usize,
    /// ErnieBook posts in local feed.
    pub erniebook_posts: usize,
    /// Content items in local store.
    pub content_items: usize,
    /// Total content storage used (bytes).
    pub content_bytes: u64,
    /// Pinned content items.
    pub pinned_items: usize,
}

/// Security overview.
#[derive(Debug, Serialize)]
pub struct SecurityStats {
    /// Number of peers with capability grants.
    pub peers_with_capabilities: usize,
    /// Whether sharing is enabled.
    pub sharing_enabled: bool,
    /// Sharing mode (whitelist/blacklist/request-approval).
    pub sharing_mode: String,
    /// Whether E2E encryption is available.
    pub e2e_available: bool,
}

/// Build a dashboard from a disabled/unconfigured mesh state.
pub fn dashboard_disabled() -> MeshDashboard {
    MeshDashboard {
        enabled: false,
        status: "Not Configured".to_string(),
        peer_id: String::new(),
        listen_port: 0,
        network: NetworkStats {
            connected_peers: 0,
            discovered_peers: 0,
            active_relays: 0,
            total_mb_relayed: 0.0,
        },
        economy: EconomyStats {
            balance: 0.0,
            total_transactions: 0,
            total_earned: 0.0,
            total_spent: 0.0,
        },
        services: ServiceStats {
            chat_messages: 0,
            chat_topics: 0,
            erniebook_posts: 0,
            content_items: 0,
            content_bytes: 0,
            pinned_items: 0,
        },
        security: SecurityStats {
            peers_with_capabilities: 0,
            sharing_enabled: false,
            sharing_mode: "disabled".to_string(),
            e2e_available: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dashboard_disabled() {
        let dash = dashboard_disabled();
        assert!(!dash.enabled);
        assert_eq!(dash.status, "Not Configured");
        assert_eq!(dash.peer_id, "");
        assert_eq!(dash.network.connected_peers, 0);
        assert_eq!(dash.economy.balance, 0.0);
    }

    #[test]
    fn test_dashboard_serializes_to_json() {
        let dash = dashboard_disabled();
        let json = serde_json::to_string(&dash).unwrap();
        assert!(json.contains("\"enabled\":false"));
        assert!(json.contains("\"status\":\"Not Configured\""));
        assert!(json.contains("\"e2e_available\":true"));
    }

    #[test]
    fn test_network_stats_default() {
        let stats = NetworkStats {
            connected_peers: 5,
            discovered_peers: 12,
            active_relays: 2,
            total_mb_relayed: 1024.5,
        };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("\"connected_peers\":5"));
    }

    #[test]
    fn test_economy_stats() {
        let stats = EconomyStats {
            balance: 42.5,
            total_transactions: 100,
            total_earned: 200.0,
            total_spent: 157.5,
        };
        assert_eq!(stats.balance, 42.5);
        assert_eq!(stats.total_transactions, 100);
    }

    #[test]
    fn test_service_stats() {
        let stats = ServiceStats {
            chat_messages: 500,
            chat_topics: 3,
            erniebook_posts: 42,
            content_items: 15,
            content_bytes: 1073741824,
            pinned_items: 5,
        };
        assert_eq!(stats.content_bytes, 1073741824);
        assert_eq!(stats.pinned_items, 5);
    }

    #[test]
    fn test_security_stats() {
        let stats = SecurityStats {
            peers_with_capabilities: 3,
            sharing_enabled: true,
            sharing_mode: "request-approval".to_string(),
            e2e_available: true,
        };
        assert!(stats.sharing_enabled);
        assert!(stats.e2e_available);
    }
}
