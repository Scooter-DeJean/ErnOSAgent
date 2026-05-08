// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Tests for mesh configuration parsing and validation.

#[cfg(test)]
mod tests {
    use crate::config::*;

    /// Valid config with all required fields — must parse successfully.
    fn valid_toml() -> String {
        r#"
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
        "#
        .to_string()
    }

    #[test]
    fn test_parse_valid_config() {
        let config = parse_mesh_config(&valid_toml())
            .expect("Valid config must parse");

        assert!(config.enabled);
        assert_eq!(config.listen_port, 4001);
        assert_eq!(config.max_connections, 128);
        assert!(config.share_bandwidth);
        assert_eq!(config.share_storage_gb, 50);
    }

    #[test]
    fn test_parse_economy_values() {
        let config = parse_mesh_config(&valid_toml()).unwrap();

        assert!((config.economy.points_per_mb_relayed - 1.0).abs() < f64::EPSILON);
        assert!((config.economy.points_per_gb_hour_stored - 0.5).abs() < f64::EPSILON);
        assert!((config.economy.points_per_1k_tokens_computed - 2.0).abs() < f64::EPSILON);
        assert_eq!(config.economy.chunk_size_bytes, 262144);
        assert!((config.economy.cost_per_1k_tokens_inference - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_security_values() {
        let config = parse_mesh_config(&valid_toml()).unwrap();

        assert!((config.security.gossip_score_threshold - (-100.0)).abs() < f64::EPSILON);
        assert!((config.security.gossip_graylist_threshold - (-1000.0)).abs() < f64::EPSILON);
        assert_eq!(config.security.gossip_score_retention_secs, 3600);
        assert_eq!(config.security.max_peers_per_ip, 3);
        assert_eq!(config.security.max_peers_per_subnet, 10);
        assert_eq!(config.security.max_payload_bytes, 1048576);
    }

    #[test]
    fn test_parse_sharing_defaults() {
        let config = parse_mesh_config(&valid_toml()).unwrap();

        assert!(!config.sharing.enabled);
        assert_eq!(config.sharing.mode, SharingMode::RequestApproval);
        assert_eq!(config.sharing.max_concurrent_peers, 5);
        assert!(config.sharing.whitelisted_peers.is_empty());
    }

    #[test]
    fn test_missing_mesh_section_returns_error() {
        let toml = r#"
[general]
active_provider = "llamacpp"
        "#;

        let result = parse_mesh_config(toml);
        assert!(result.is_err());

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Missing [mesh] section"),
            "Error must direct user to configure mesh: got '{}'",
            err_msg
        );
    }

    #[test]
    fn test_missing_economy_section_returns_parse_error() {
        let toml = r#"
[mesh]
enabled = true
listen_port = 4001
max_connections = 128
share_bandwidth = true
share_storage_gb = 50

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

        let result = parse_mesh_config(toml);
        assert!(
            result.is_err(),
            "Missing [mesh.economy] must cause parse failure"
        );
    }

    #[test]
    fn test_missing_security_section_returns_parse_error() {
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

[mesh.sharing]
enabled = false
mode = "request-approval"
max_bandwidth_per_peer_mbps = 10
max_concurrent_peers = 5
session_timeout_hours = 24
        "#;

        let result = parse_mesh_config(toml);
        assert!(
            result.is_err(),
            "Missing [mesh.security] must cause parse failure"
        );
    }

    #[test]
    fn test_erniebook_defaults_to_disabled() {
        let config = parse_mesh_config(&valid_toml()).unwrap();
        assert!(!config.erniebook.enabled, "ErnieBook must default to disabled (§15.2)");
    }

    #[test]
    fn test_supernode_defaults_to_disabled() {
        let config = parse_mesh_config(&valid_toml()).unwrap();
        assert!(!config.supernode.enabled, "Super-Node must default to disabled");
    }

    #[test]
    fn test_erniebook_explicit_enable() {
        let toml = format!(
            r#"{}
[mesh.erniebook]
enabled = true
"#,
            valid_toml()
        );

        let config = parse_mesh_config(&toml).unwrap();
        assert!(config.erniebook.enabled);
    }

    #[test]
    fn test_sharing_mode_whitelist() {
        let toml = valid_toml().replace(
            r#"mode = "request-approval""#,
            r#"mode = "whitelist""#,
        );
        let config = parse_mesh_config(&toml).unwrap();
        assert_eq!(config.sharing.mode, SharingMode::Whitelist);
    }

    #[test]
    fn test_sharing_mode_blacklist() {
        let toml = valid_toml().replace(
            r#"mode = "request-approval""#,
            r#"mode = "blacklist""#,
        );
        let config = parse_mesh_config(&toml).unwrap();
        assert_eq!(config.sharing.mode, SharingMode::Blacklist);
    }

    #[test]
    fn test_bootstrap_peers_optional() {
        let config = parse_mesh_config(&valid_toml()).unwrap();
        assert!(
            config.bootstrap_peers.is_empty(),
            "bootstrap_peers must default to empty when absent"
        );
    }

    #[test]
    fn test_bootstrap_peers_explicit() {
        let toml = valid_toml().replace(
            "bootstrap_peers = []",
            "",
        ).replace(
            "listen_port = 4001",
            "listen_port = 4001\nbootstrap_peers = [\"/ip4/1.2.3.4/tcp/4001\", \"/ip4/5.6.7.8/tcp/4001\"]",
        );
        let config = parse_mesh_config(&toml).unwrap();
        assert_eq!(config.bootstrap_peers.len(), 2);
    }

    #[test]
    fn test_toml_roundtrip_preserves_values() {
        let original = parse_mesh_config(&valid_toml()).unwrap();
        let serialized = toml::to_string_pretty(&original)
            .expect("Serialization must succeed");
        let roundtripped = toml::from_str::<MeshConfig>(&serialized)
            .expect("Deserialization of serialized config must succeed");
        assert_eq!(original, roundtripped);
    }

    #[test]
    fn test_resolve_mesh_data_dir_creates_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let mesh_dir = resolve_mesh_data_dir(tmp.path())
            .expect("Must create mesh data directory");

        assert!(mesh_dir.exists(), "Mesh data directory must exist after resolve");
        assert_eq!(mesh_dir, tmp.path().join("mesh"));
    }

    #[test]
    fn test_resolve_mesh_data_dir_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let first = resolve_mesh_data_dir(tmp.path()).unwrap();
        let second = resolve_mesh_data_dir(tmp.path()).unwrap();
        assert_eq!(first, second, "resolve_mesh_data_dir must be idempotent");
    }

    #[test]
    fn test_pinning_bonus_multiplier_default() {
        let config = parse_mesh_config(&valid_toml()).unwrap();
        assert!(
            (config.economy.pinning_bonus_multiplier - 1.0).abs() < f64::EPSILON,
            "Pinning bonus must default to 1.0 (no bonus) when not specified"
        );
    }

    #[test]
    fn test_security_recovery_key_defaults_disabled() {
        let config = parse_mesh_config(&valid_toml()).unwrap();
        assert!(
            !config.security.enable_recovery_key,
            "Recovery key must default to disabled"
        );
    }

    #[test]
    fn test_security_blocklist_defaults_enabled() {
        let config = parse_mesh_config(&valid_toml()).unwrap();
        assert!(
            config.security.enable_community_blocklist,
            "Community blocklist must default to enabled"
        );
    }

    #[test]
    fn test_invalid_toml_syntax_returns_error() {
        let result = parse_mesh_config("this is not valid toml {{{}}");
        assert!(result.is_err(), "Invalid TOML must return error");
    }
}
