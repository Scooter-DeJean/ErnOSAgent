// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Tests for swarm module — extracted per §1.1 (file length limit).

use super::*;

fn test_keypair() -> Keypair {
    Keypair::generate_ed25519()
}

fn test_config() -> MeshConfig {
    let toml = r#"
[mesh]
enabled = true
listen_port = 0
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
fn test_build_swarm_succeeds() {
    let kp = test_keypair();
    let config = test_config();
    let swarm = build_swarm(kp, &config);
    assert!(swarm.is_ok(), "Must build swarm");
}

#[test]
fn test_build_swarm_with_different_ports() {
    let kp = test_keypair();
    let mut config = test_config();
    config.listen_port = 9999;
    let swarm = build_swarm(kp, &config);
    assert!(swarm.is_ok());
}
