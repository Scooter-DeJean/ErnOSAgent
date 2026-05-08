// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Tests for mesh node lifecycle, identity, signing, and snapshot.

#[cfg(test)]
mod tests {
    use crate::config::MeshConfig;
    use crate::node::*;

    /// Parse the standard test config TOML.
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
    fn test_initialize_creates_node() {
        let tmp = tempfile::tempdir().unwrap();
        let config = test_config();

        let node = MeshNode::initialize(config, tmp.path())
            .expect("Must initialize mesh node");

        assert!(!node.peer_id_base58().is_empty());
    }

    #[test]
    fn test_initialize_creates_identity_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let config = test_config();

        let _node = MeshNode::initialize(config, tmp.path()).unwrap();

        let key_path = tmp.path().join("mesh").join("identity.key");
        assert!(key_path.exists(), "Identity key must be persisted");
    }

    #[test]
    fn test_initialize_same_dir_same_peer_id() {
        let tmp = tempfile::tempdir().unwrap();

        let node_a = MeshNode::initialize(test_config(), tmp.path()).unwrap();
        let node_b = MeshNode::initialize(test_config(), tmp.path()).unwrap();

        assert_eq!(
            node_a.peer_id_base58(),
            node_b.peer_id_base58(),
            "Same data dir must produce same PeerId"
        );
    }

    #[test]
    fn test_sequence_numbers_increase() {
        let tmp = tempfile::tempdir().unwrap();
        let node = MeshNode::initialize(test_config(), tmp.path()).unwrap();

        let seq1 = node.next_sequence();
        let seq2 = node.next_sequence();
        let seq3 = node.next_sequence();

        assert!(seq2 > seq1);
        assert!(seq3 > seq2);
    }

    #[test]
    fn test_sign_message_produces_valid_envelope() {
        let tmp = tempfile::tempdir().unwrap();
        let node = MeshNode::initialize(test_config(), tmp.path()).unwrap();

        let envelope = node.sign_message(
            crate::protocol::MessagePayload::Ping { nonce: 42 },
        ).expect("Must sign message");

        assert_eq!(envelope.body.sender, node.peer_id_base58());
        assert!(!envelope.signature.is_empty());
    }

    #[test]
    fn test_sign_message_increments_sequence() {
        let tmp = tempfile::tempdir().unwrap();
        let node = MeshNode::initialize(test_config(), tmp.path()).unwrap();

        let env1 = node.sign_message(
            crate::protocol::MessagePayload::Ping { nonce: 1 },
        ).unwrap();
        let env2 = node.sign_message(
            crate::protocol::MessagePayload::Ping { nonce: 2 },
        ).unwrap();

        assert!(
            env2.body.sequence > env1.body.sequence,
            "Sequence must increase between messages"
        );
    }

    #[tokio::test]
    async fn test_snapshot_initial_state() {
        let tmp = tempfile::tempdir().unwrap();
        let node = MeshNode::initialize(test_config(), tmp.path()).unwrap();

        let snap = node.snapshot().await;
        assert_eq!(snap.status, NodeStatus::Configured);
        assert_eq!(snap.connected_peers, 0);
        assert_eq!(snap.listen_port, 4001);
        assert!(snap.sharing_active);
        assert!(!snap.erniebook_enabled);
    }

    #[tokio::test]
    async fn test_start_transitions_to_running() {
        let tmp = tempfile::tempdir().unwrap();
        let node = MeshNode::initialize(test_config(), tmp.path()).unwrap();

        node.start().await.expect("Must start");

        let snap = node.snapshot().await;
        assert_eq!(snap.status, NodeStatus::Running);
    }

    #[tokio::test]
    async fn test_stop_transitions_to_stopped() {
        let tmp = tempfile::tempdir().unwrap();
        let node = MeshNode::initialize(test_config(), tmp.path()).unwrap();

        node.start().await.unwrap();
        node.stop().await.expect("Must stop");

        let snap = node.snapshot().await;
        assert_eq!(snap.status, NodeStatus::Stopped);
    }

    #[tokio::test]
    async fn test_capabilities_accessible_from_node() {
        let tmp = tempfile::tempdir().unwrap();
        let node = MeshNode::initialize(test_config(), tmp.path()).unwrap();

        let caps = node.capabilities();
        let mut caps_guard = caps.write().await;
        caps_guard.grant("peer_x", crate::capability::Capability::ReadContent);

        assert!(
            caps_guard.check("peer_x", crate::capability::Capability::ReadContent).is_granted()
        );
    }

    #[tokio::test]
    async fn test_peer_registry_accessible_from_node() {
        let tmp = tempfile::tempdir().unwrap();
        let node = MeshNode::initialize(test_config(), tmp.path()).unwrap();

        let peers = node.peers();
        let mut peers_guard = peers.write().await;
        peers_guard.add_peer(
            libp2p_identity::PeerId::random(),
            vec!["/ip4/1.2.3.4/tcp/4001".parse().unwrap()],
            crate::discovery::DiscoveryMethod::Mdns,
        );

        assert_eq!(peers_guard.peer_count(), 1);
    }

    #[test]
    fn test_node_status_display() {
        assert_eq!(format!("{}", NodeStatus::Configured), "Configured");
        assert_eq!(format!("{}", NodeStatus::Running), "Running");
        assert_eq!(format!("{}", NodeStatus::Stopped), "Stopped");
        assert_eq!(format!("{}", NodeStatus::Error), "Error");
    }
}
