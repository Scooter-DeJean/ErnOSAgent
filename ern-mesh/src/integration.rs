// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Integration tests — end-to-end scenarios across multiple mesh modules.
//!
//! These tests verify that the modules compose correctly:
//! - Identity → Node → Behaviour → Swarm
//! - Chat → MessageBus → Economy (via Accountant)
//! - Content store → Economy (pinning earns points)
//! - Relay pool → Economy (relay earns points)
//! - Encryption → Chat (E2E DM flow)
//!
//! All tests use ephemeral tmpdir state and in-process components.

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use tokio::sync::RwLock;

    // ── Identity → Node ──────────────────────────────────────────────

    #[test]
    fn test_identity_to_node_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let config = test_mesh_config();

        let node = crate::node::MeshNode::initialize(config, tmp.path())
            .expect("Node must initialise with fresh identity");

        // Identity was persisted
        assert!(tmp.path().join("mesh/identity.key").exists());

        // PeerId is valid base58
        let pid = node.peer_id_base58();
        assert!(pid.len() > 20, "PeerId base58 should be >20 chars");
    }

    #[test]
    fn test_node_deterministic_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let config = test_mesh_config();

        let pid1 = {
            let node1 = crate::node::MeshNode::initialize(config.clone(), tmp.path()).unwrap();
            node1.peer_id_base58()
        };

        let config2 = test_mesh_config();
        let node2 = crate::node::MeshNode::initialize(config2, tmp.path()).unwrap();
        let pid2 = node2.peer_id_base58();

        assert_eq!(pid1, pid2, "Same data dir must produce same PeerId");
    }

    // ── Chat → MessageBus ────────────────────────────────────────────

    #[tokio::test]
    async fn test_chat_to_message_bus_roundtrip() {
        let chat_log = Arc::new(RwLock::new(crate::chat::ChatLog::new(100)));
        let feed = Arc::new(RwLock::new(crate::erniebook::Feed::new(100)));
        let bus = crate::message_bus::MessageBus::new(chat_log.clone(), feed);

        // Create and prepare a chat message
        let msg = crate::chat::ChatMessage::new_public(
            crate::chat::TopicId::new("general"),
            "peer_a",
            "Integration test!",
            Some("Alice".into()),
        );

        let outgoing = bus.prepare_chat(&msg).unwrap();
        assert_eq!(outgoing.topic, "ernmesh/chat/general");

        // Simulate receiving it back through Gossipsub
        let incoming = crate::message_bus::IncomingMessage {
            topic: outgoing.topic,
            data: outgoing.data,
            source: "peer_a".to_string(),
        };

        bus.handle_incoming(incoming).await.unwrap();

        let log = chat_log.read().await;
        assert_eq!(log.total_messages(), 1);
    }

    // ── ErnieBook → MessageBus ───────────────────────────────────────

    #[tokio::test]
    async fn test_erniebook_to_message_bus_roundtrip() {
        let chat_log = Arc::new(RwLock::new(crate::chat::ChatLog::new(100)));
        let feed = Arc::new(RwLock::new(crate::erniebook::Feed::new(100)));
        let bus = crate::message_bus::MessageBus::new(chat_log, feed.clone());

        let post = crate::erniebook::Post::new(
            "peer_b",
            "Hello ErnieBook!",
            Some("Bob".into()),
            vec!["intro".into()],
        );

        let outgoing = bus.prepare_post(&post).unwrap();

        let incoming = crate::message_bus::IncomingMessage {
            topic: outgoing.topic,
            data: outgoing.data,
            source: "peer_b".to_string(),
        };

        bus.handle_incoming(incoming).await.unwrap();

        let f = feed.read().await;
        assert_eq!(f.count(), 1);
    }

    // ── Relay → Accountant (earn on relay) ────────────────────────────

    #[tokio::test]
    async fn test_relay_to_accountant_earns_points() {
        let config = test_economy_config();
        let ledger = Arc::new(RwLock::new(crate::economy::Ledger::new(config)));
        let accountant = crate::accounting::Accountant::new(ledger);

        // Simulate relay usage
        let earned = accountant.record_relay(50.0).await;
        assert_eq!(earned, 50.0); // 50 MB * 1.0 pts/MB

        let balance = accountant.balance().await;
        assert_eq!(balance, 50.0);
    }

    // ── Content store → Economy (storage earns points) ────────────────

    #[tokio::test]
    async fn test_content_store_with_accountant() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = crate::content_store::ContentStore::open(
            &tmp.path().join("content"),
        ).unwrap();

        let config = test_economy_config();
        let ledger = Arc::new(RwLock::new(crate::economy::Ledger::new(config)));
        let accountant = crate::accounting::Accountant::new(ledger);

        // Store content
        let data = b"important mesh content";
        let cid = store.store(data, None, None).unwrap();
        store.pin(&cid);

        // Simulate storage contribution (1 GB for 1 hour)
        let earned = accountant.record_storage(1.0).await;
        assert_eq!(earned, 0.5); // 1 GB·h * 0.5 pts/GB·h

        // Verify content is persisted and pinned
        let meta = store.metadata(&cid).unwrap();
        assert!(meta.pinned);
    }

    // ── E2E Encryption → Chat (encrypted DM flow) ────────────────────

    #[tokio::test]
    async fn test_encrypted_dm_flow() {
        // Two peers generate key pairs
        let alice_kp = crate::encryption::EphemeralKeyPair::generate();
        let bob_kp = crate::encryption::EphemeralKeyPair::generate();

        let alice_pub = *alice_kp.public_key();
        let bob_pub = *bob_kp.public_key();

        // Key exchange
        let alice_key = alice_kp.derive_session_key(&bob_pub).unwrap();
        let bob_key = bob_kp.derive_session_key(&alice_pub).unwrap();

        // Alice encrypts a DM
        let plaintext = b"Hey Bob, this is a secret message!";
        let encrypted = alice_key.encrypt(plaintext).unwrap();
        let encoded = hex::encode(&encrypted);

        // Alice creates an encrypted chat message
        let topic = crate::chat::TopicId::direct("alice_peer_id", "bob_peer_id");
        let msg = crate::chat::ChatMessage::new_encrypted(
            topic.clone(),
            "alice_peer_id",
            encoded.clone(),
            Some("Alice".into()),
        );

        assert!(msg.encrypted);
        assert!(msg.topic.is_dm());

        // Bob receives and decrypts
        let ciphertext = hex::decode(&msg.content).unwrap();
        let decrypted = bob_key.decrypt(&ciphertext).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    // ── Relay pool enforces limits ───────────────────────────────────

    #[test]
    fn test_relay_pool_integration() {
        let sharing_config = crate::config::SharingConfig {
            enabled: true,
            mode: crate::config::SharingMode::RequestApproval,
            max_bandwidth_per_peer_mbps: 10,
            max_concurrent_peers: 2,
            session_timeout_hours: 24,
            whitelisted_peers: Vec::new(),
            blacklisted_peers: Vec::new(),
        };

        let mut pool = crate::relay::RelayPool::new(sharing_config);

        // Grant two sessions (at limit)
        pool.request_session("peer_a", false).unwrap();
        pool.request_session("peer_b", true).unwrap();

        // Third peer must be denied
        let result = pool.request_session("peer_c", false);
        assert!(result.is_err());

        // Terminate one, then third peer can join
        pool.terminate_session("peer_a");
        pool.request_session("peer_c", false).unwrap();
    }

    // ── Capability + Protocol ────────────────────────────────────────

    #[test]
    fn test_capability_guards_protocol_messages() {
        let mut caps = crate::capability::CapabilityStore::new();

        // Peer has no capabilities — must not be able to publish
        assert!(
            !caps.check("peer_test", crate::capability::Capability::PublishTopics).is_granted(),
            "Peers must have zero capabilities by default"
        );

        // Grant publish capability
        caps.grant("peer_test", crate::capability::Capability::PublishTopics);
        assert!(caps.check("peer_test", crate::capability::Capability::PublishTopics).is_granted());

        // Revoke
        caps.revoke("peer_test", crate::capability::Capability::PublishTopics);
        assert!(!caps.check("peer_test", crate::capability::Capability::PublishTopics).is_granted());
    }

    // ── Dashboard disabled state ─────────────────────────────────────

    #[test]
    fn test_dashboard_disabled_state() {
        let dash = crate::dashboard::dashboard_disabled();
        assert!(!dash.enabled);
        assert_eq!(dash.status, "Not Configured");
        assert!(dash.security.e2e_available);
    }

    // ── Full earn/spend cycle ────────────────────────────────────────

    #[tokio::test]
    async fn test_full_earn_spend_cycle() {
        let config = test_economy_config();
        let ledger = Arc::new(RwLock::new(crate::economy::Ledger::new(config)));
        let acc = crate::accounting::Accountant::new(ledger);

        // Earn from multiple sources
        acc.record_relay(100.0).await;     // +100
        acc.record_compute(10.0).await;    // +20
        acc.record_uptime(24.0).await;     // +24
        acc.record_wifi(500.0).await;      // +50
        // Total earned: 194

        assert_eq!(acc.balance().await, 194.0);

        // Spend on inference
        acc.charge_inference(10.0).await.unwrap(); // -50
        assert_eq!(acc.balance().await, 144.0);

        // Try to overspend
        let result = acc.charge_inference(100.0).await; // needs 500
        assert!(result.is_err());
        assert_eq!(acc.balance().await, 144.0); // unchanged

        assert_eq!(acc.transaction_count().await, 5); // 4 earn + 1 spend
    }

    // ── Helpers ──────────────────────────────────────────────────────

    fn test_mesh_config() -> crate::config::MeshConfig {
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

    fn test_economy_config() -> crate::config::EconomyConfig {
        crate::config::EconomyConfig {
            points_per_mb_relayed: 1.0,
            points_per_gb_hour_stored: 0.5,
            points_per_1k_tokens_computed: 2.0,
            points_per_mb_wifi_shared: 0.1,
            points_per_hour_uptime: 1.0,
            chunk_size_bytes: 262144,
            cost_per_1k_tokens_inference: 5.0,
            cost_per_gb_month_storage: 10.0,
            cost_per_session_priority: 2.0,
            cost_per_replica_month: 3.0,
            pinning_bonus_multiplier: 1.0,
        }
    }
}
