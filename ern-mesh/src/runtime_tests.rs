// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Tests for mesh runtime — initialisation, lifecycle, and cross-module integration.

#[cfg(test)]
mod tests {
    use crate::config::MeshConfig;
    use crate::runtime::*;

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
    fn test_runtime_initialize() {
        let tmp = tempfile::tempdir().unwrap();
        let config = test_config();
        let runtime = MeshRuntime::initialize(config, tmp.path())
            .expect("Runtime must initialize");

        assert!(!runtime.peer_id().is_empty());
    }

    #[tokio::test]
    async fn test_runtime_start_stop() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = MeshRuntime::initialize(test_config(), tmp.path()).unwrap();

        runtime.start().await.unwrap();
        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_dashboard_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = MeshRuntime::initialize(test_config(), tmp.path()).unwrap();

        let snap = runtime.dashboard_snapshot().await;

        assert_eq!(snap.status, "Configured");
        assert_eq!(snap.connected_peers, 0);
        assert_eq!(snap.balance, 0.0);
        assert_eq!(snap.active_sessions, 0);
        assert_eq!(snap.hosted_sites, 0);
        assert_eq!(snap.unread_mail, 0);
        assert_eq!(snap.voice_rooms, 0);
        assert_eq!(snap.forum_communities, 0);
        assert_eq!(snap.chat_messages, 0);
        assert_eq!(snap.feed_posts, 0);
    }

    #[tokio::test]
    async fn test_runtime_chat_integration() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = MeshRuntime::initialize(test_config(), tmp.path()).unwrap();

        // Send a chat message through the runtime
        let msg = crate::chat::ChatMessage::new_public(
            crate::chat::TopicId::new("general"),
            &runtime.peer_id(),
            "Hello from runtime!",
            None,
        );

        let outgoing = runtime.message_bus.prepare_chat(&msg).unwrap();
        let incoming = crate::message_bus::IncomingMessage {
            topic: outgoing.topic,
            data: outgoing.data,
            source: runtime.peer_id(),
        };

        runtime.message_bus.handle_incoming(incoming).await.unwrap();

        let snap = runtime.dashboard_snapshot().await;
        assert_eq!(snap.chat_messages, 1);
    }

    #[tokio::test]
    async fn test_runtime_economy_integration() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = MeshRuntime::initialize(test_config(), tmp.path()).unwrap();

        // Earn points through the runtime
        runtime.accountant.record_relay(100.0).await;
        runtime.accountant.record_uptime(24.0).await;

        let snap = runtime.dashboard_snapshot().await;
        assert_eq!(snap.balance, 124.0); // 100 + 24
        assert_eq!(snap.transactions, 2);
    }

    #[tokio::test]
    async fn test_runtime_mail_integration() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = MeshRuntime::initialize(test_config(), tmp.path()).unwrap();

        let mail = crate::mesh_mail::MailMessage::new(
            "alice", &runtime.peer_id(), "Hello", "Test mail",
        );
        runtime.mailbox.write().await.receive(mail);

        let snap = runtime.dashboard_snapshot().await;
        assert_eq!(snap.unread_mail, 1);
        assert_eq!(snap.inbox_count, 1);
    }

    #[tokio::test]
    async fn test_runtime_voice_integration() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = MeshRuntime::initialize(test_config(), tmp.path()).unwrap();

        {
            let mut voice = runtime.voice.write().await;
            voice.create_room("room-1", &runtime.peer_id(), "General", 10, 4);
            voice.get_room_mut("room-1").unwrap().join("alice").unwrap();
        }

        let snap = runtime.dashboard_snapshot().await;
        assert_eq!(snap.voice_rooms, 1);
        assert_eq!(snap.voice_participants, 1);
    }

    #[tokio::test]
    async fn test_runtime_forum_integration() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = MeshRuntime::initialize(test_config(), tmp.path()).unwrap();

        {
            let mut forums = runtime.forums.write().await;
            forums.create_community(
                crate::forum::Community::new("general", "General discussion", &runtime.peer_id()),
            ).unwrap();
            forums.add_post(
                crate::forum::ForumPost::new_thread("general", &runtime.peer_id(), "Hello", "First post!"),
            ).unwrap();
        }

        let snap = runtime.dashboard_snapshot().await;
        assert_eq!(snap.forum_communities, 1);
        assert_eq!(snap.forum_posts, 1);
    }

    #[tokio::test]
    async fn test_runtime_reputation_integration() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = MeshRuntime::initialize(test_config(), tmp.path()).unwrap();

        {
            let mut rep = runtime.reputation.write().await;
            rep.record("alice", crate::reputation::ReputationEvent::ManualTrust { delta: 100.0 });
            rep.record("bad_actor", crate::reputation::ReputationEvent::SpamDetected);
        }

        let snap = runtime.dashboard_snapshot().await;
        assert_eq!(snap.known_peers_reputation, 2);
        assert_eq!(snap.trusted_peers, 1);
    }

    #[tokio::test]
    async fn test_runtime_state_persistence() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = MeshRuntime::initialize(test_config(), tmp.path()).unwrap();

        // Save some state
        runtime.state_dir.save("test_key", &42u64).unwrap();
        let loaded: Option<u64> = runtime.state_dir.load("test_key").unwrap();
        assert_eq!(loaded, Some(42));
    }

    #[test]
    fn test_runtime_snapshot_serializable() {
        let snap = RuntimeSnapshot {
            peer_id: "test".into(),
            status: "Running".into(),
            connected_peers: 5,
            listen_port: 4001,
            balance: 100.0,
            transactions: 10,
            active_sessions: 2,
            hosted_sites: 1,
            active_transfers: 0,
            unread_mail: 3,
            inbox_count: 10,
            voice_rooms: 1,
            voice_participants: 4,
            forum_communities: 2,
            forum_posts: 15,
            known_peers_reputation: 20,
            trusted_peers: 5,
            hostile_peers: 1,
            chat_messages: 50,
            feed_posts: 8,
            relay_sessions: 0,
        };

        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("\"balance\":100.0"));
        assert!(json.contains("\"unread_mail\":3"));
    }
}
