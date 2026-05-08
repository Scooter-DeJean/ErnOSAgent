// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Transport layer configuration — parallel QUIC + TCP/Noise/Yamux stack.
//!
//! ErnMesh runs two transports in parallel, negotiated automatically by libp2p:
//!
//! - **QUIC**: Modern, UDP-based transport with built-in TLS 1.3 encryption
//!   and multiplexing. Preferred for most connections.
//! - **TCP + Noise + Yamux**: Fallback-free alternative transport for networks
//!   that block UDP. Noise provides encryption, Yamux provides multiplexing.
//!
//! Both transports run simultaneously — libp2p selects the best available
//! path per peer. Neither is a "fallback" — both are first-class. If no
//! transport path works, the connection fails loud with a clear error (§2.4).
//!
//! This module configures the transport stack. The actual `Swarm` is
//! assembled in `node.rs` (PR 7), which calls [`build_transport_config`]
//! to get the transport layer configuration.

use anyhow::{Context, Result};
use libp2p::quic;
use libp2p::tcp;

use crate::config::MeshConfig;

/// Transport configuration derived from the mesh config.
///
/// Holds the configured TCP and QUIC parameters ready for Swarm construction.
/// Created via [`build_transport_config`] and consumed by the node builder.
pub struct TransportConfig {
    /// TCP transport configuration.
    tcp: tcp::Config,

    /// QUIC transport configuration.
    quic: quic::Config,

    /// The port to listen on — applies to both QUIC (UDP) and TCP.
    listen_port: u16,
}

impl std::fmt::Debug for TransportConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransportConfig")
            .field("listen_port", &self.listen_port)
            .field("tcp", &"tcp::Config { .. }")
            .field("quic", &"quic::Config { .. }")
            .finish()
    }
}

impl TransportConfig {
    /// Returns a reference to the TCP configuration.
    pub fn tcp(&self) -> &tcp::Config {
        &self.tcp
    }

    /// Returns a reference to the QUIC configuration.
    pub fn quic(&self) -> &quic::Config {
        &self.quic
    }

    /// Returns the configured listen port.
    pub fn listen_port(&self) -> u16 {
        self.listen_port
    }

    /// Returns the QUIC multiaddr string for listener binding.
    ///
    /// Format: `/ip4/0.0.0.0/udp/{port}/quic-v1`
    pub fn quic_listen_addr(&self) -> String {
        format!("/ip4/0.0.0.0/udp/{}/quic-v1", self.listen_port)
    }

    /// Returns the TCP multiaddr string for listener binding.
    ///
    /// Format: `/ip4/0.0.0.0/tcp/{port}`
    pub fn tcp_listen_addr(&self) -> String {
        format!("/ip4/0.0.0.0/tcp/{}", self.listen_port)
    }
}

/// Build transport configuration from the mesh config.
///
/// Configures both QUIC and TCP transports based on the user's
/// `[mesh]` settings. Both transports will listen on the same port
/// number (QUIC on UDP, TCP on TCP).
///
/// The resulting [`TransportConfig`] is consumed by the node builder
/// in `node.rs` when constructing the libp2p `Swarm`.
pub fn build_transport_config(config: &MeshConfig) -> Result<TransportConfig> {
    let tcp_config = build_tcp_config();
    let quic_config = build_quic_config();

    tracing::info!(
        listen_port = config.listen_port,
        "Configured parallel transport stack: QUIC (UDP/{port}) + TCP/Noise/Yamux (TCP/{port})",
        port = config.listen_port,
    );

    Ok(TransportConfig {
        tcp: tcp_config,
        quic: quic_config,
        listen_port: config.listen_port,
    })
}

/// Build TCP transport configuration.
///
/// Uses default TCP settings — libp2p handles socket options.
/// Encryption is handled by Noise (configured at Swarm build time).
/// Multiplexing is handled by Yamux (configured at Swarm build time).
fn build_tcp_config() -> tcp::Config {
    let config = tcp::Config::default()
        .nodelay(true);

    tracing::debug!("TCP transport configured with nodelay=true");

    config
}

/// Build QUIC transport configuration.
///
/// QUIC provides built-in TLS 1.3 encryption and multiplexing — no separate
/// Noise or Yamux layer needed. This is the preferred transport for direct
/// peer connections.
fn build_quic_config() -> quic::Config {
    let config = quic::Config::new(&libp2p_identity::Keypair::generate_ed25519());

    tracing::debug!("QUIC transport configured");

    config
}

/// Parse a multiaddr string into a libp2p `Multiaddr`.
///
/// Used for parsing bootstrap peer addresses and listen addresses
/// from configuration strings.
pub fn parse_multiaddr(addr: &str) -> Result<libp2p::Multiaddr> {
    addr.parse::<libp2p::Multiaddr>()
        .with_context(|| format!(
            "Invalid multiaddr: '{}'. \
             Expected format: /ip4/1.2.3.4/tcp/4001 or /ip4/1.2.3.4/udp/4001/quic-v1",
            addr
        ))
}

/// Parse all bootstrap peer addresses from config.
///
/// Returns a `Vec` of parsed `Multiaddr` values. Logs a warning for
/// any addresses that fail to parse (but does not fail the entire operation).
pub fn parse_bootstrap_addrs(config: &MeshConfig) -> Vec<libp2p::Multiaddr> {
    let mut addrs = Vec::with_capacity(config.bootstrap_peers.len());

    for peer_str in &config.bootstrap_peers {
        match parse_multiaddr(peer_str) {
            Ok(addr) => {
                tracing::debug!(addr = %addr, "Parsed bootstrap peer address");
                addrs.push(addr);
            }
            Err(e) => {
                tracing::warn!(
                    addr = %peer_str,
                    error = %e,
                    "Skipping invalid bootstrap peer address"
                );
            }
        }
    }

    tracing::info!(
        total = config.bootstrap_peers.len(),
        valid = addrs.len(),
        "Parsed bootstrap peer addresses"
    );

    addrs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;

    /// Create a minimal valid MeshConfig for testing.
    fn test_config() -> MeshConfig {
        let toml = r#"
[mesh]
enabled = true
listen_port = 4001
max_connections = 128
share_bandwidth = true
share_storage_gb = 50

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
        crate::config::parse_mesh_config(toml).unwrap()
    }

    #[test]
    fn test_build_transport_config_succeeds() {
        let config = test_config();
        let transport = build_transport_config(&config)
            .expect("Must build transport config");

        assert_eq!(transport.listen_port(), 4001);
    }

    #[test]
    fn test_quic_listen_addr_format() {
        let config = test_config();
        let transport = build_transport_config(&config).unwrap();

        assert_eq!(
            transport.quic_listen_addr(),
            "/ip4/0.0.0.0/udp/4001/quic-v1"
        );
    }

    #[test]
    fn test_tcp_listen_addr_format() {
        let config = test_config();
        let transport = build_transport_config(&config).unwrap();

        assert_eq!(
            transport.tcp_listen_addr(),
            "/ip4/0.0.0.0/tcp/4001"
        );
    }

    #[test]
    fn test_parse_valid_tcp_multiaddr() {
        let addr = parse_multiaddr("/ip4/127.0.0.1/tcp/4001")
            .expect("Valid TCP multiaddr must parse");

        let addr_str = addr.to_string();
        assert!(addr_str.contains("127.0.0.1"));
        assert!(addr_str.contains("4001"));
    }

    #[test]
    fn test_parse_valid_quic_multiaddr() {
        let addr = parse_multiaddr("/ip4/1.2.3.4/udp/4001/quic-v1")
            .expect("Valid QUIC multiaddr must parse");

        let addr_str = addr.to_string();
        assert!(addr_str.contains("1.2.3.4"));
        assert!(addr_str.contains("quic-v1"));
    }

    #[test]
    fn test_parse_invalid_multiaddr_returns_error() {
        let result = parse_multiaddr("not-a-multiaddr");
        assert!(result.is_err(), "Invalid multiaddr must return error");

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Invalid multiaddr"),
            "Error must say 'Invalid multiaddr': got '{}'",
            err_msg
        );
    }

    #[test]
    fn test_parse_bootstrap_addrs_all_valid() {
        let mut config = test_config();
        config.bootstrap_peers = vec![
            "/ip4/1.2.3.4/tcp/4001".to_string(),
            "/ip4/5.6.7.8/udp/4001/quic-v1".to_string(),
        ];

        let addrs = parse_bootstrap_addrs(&config);
        assert_eq!(addrs.len(), 2, "Both valid addresses must parse");
    }

    #[test]
    fn test_parse_bootstrap_addrs_skips_invalid() {
        let mut config = test_config();
        config.bootstrap_peers = vec![
            "/ip4/1.2.3.4/tcp/4001".to_string(),
            "garbage-address".to_string(),
            "/ip4/5.6.7.8/udp/4001/quic-v1".to_string(),
        ];

        let addrs = parse_bootstrap_addrs(&config);
        assert_eq!(
            addrs.len(), 2,
            "Invalid addresses must be skipped, valid ones kept"
        );
    }

    #[test]
    fn test_parse_bootstrap_addrs_empty_config() {
        let config = test_config();
        let addrs = parse_bootstrap_addrs(&config);
        assert!(addrs.is_empty(), "Empty bootstrap_peers must return empty vec");
    }

    #[test]
    fn test_different_ports_produce_different_listen_addrs() {
        let mut config_a = test_config();
        config_a.listen_port = 4001;
        let transport_a = build_transport_config(&config_a).unwrap();

        let mut config_b = test_config();
        config_b.listen_port = 9999;
        let transport_b = build_transport_config(&config_b).unwrap();

        assert_ne!(
            transport_a.quic_listen_addr(),
            transport_b.quic_listen_addr(),
            "Different ports must produce different listen addresses"
        );
        assert_ne!(
            transport_a.tcp_listen_addr(),
            transport_b.tcp_listen_addr(),
        );
    }
}
