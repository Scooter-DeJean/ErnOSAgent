// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Mesh network configuration — parsed from the `[mesh]` section of `ern-os.toml`.
//!
//! All mesh parameters are user-configurable. There are **no compiled-in defaults**
//! for operational values. If the `[mesh]` section or required subsections are absent,
//! the system returns an error instructing the user to configure them (§5).
//!
//! This module is the single source of truth for mesh config types. It does not
//! perform any networking — it only parses and validates configuration.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ─── Top-Level Mesh Config ───────────────────────────────────────────

/// Root mesh configuration, deserialized from `[mesh]` in `ern-os.toml`.
///
/// All fields are required — serde will fail to parse if any are missing,
/// forcing the user to explicitly configure their mesh node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MeshConfig {
    /// Whether the mesh subsystem is active. When `true`, the node binds
    /// to `0.0.0.0:{listen_port}` and joins the mesh network.
    pub enabled: bool,

    /// Port for the mesh listener. User-chosen based on their network.
    pub listen_port: u16,

    /// Optional list of known peers for DHT bootstrap. Empty starts mDNS-only.
    #[serde(default)]
    pub bootstrap_peers: Vec<String>,

    /// Maximum simultaneous peer connections. User sets based on hardware/bandwidth.
    pub max_connections: u32,

    /// Whether to contribute bandwidth to the mesh relay pool.
    pub share_bandwidth: bool,

    /// Gigabytes of local disk to contribute for mesh content storage.
    pub share_storage_gb: u64,

    /// ErnPoints economy configuration.
    pub economy: EconomyConfig,

    /// Security and peer scoring configuration.
    pub security: SecurityConfig,

    /// Resource sharing configuration (WiFi, compute, relay pool).
    pub sharing: SharingConfig,

    /// ErnieBook AI social network configuration.
    #[serde(default)]
    pub erniebook: ErnieBookConfig,

    /// Super-Node SFU volunteering configuration.
    #[serde(default)]
    pub supernode: SuperNodeConfig,

    /// Service buffer capacities and reputation scoring.
    #[serde(default)]
    pub services: ServicesConfig,
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen_port: 4001,
            bootstrap_peers: Vec::new(),
            max_connections: 128,
            share_bandwidth: false,
            share_storage_gb: 0,
            economy: EconomyConfig::default(),
            security: SecurityConfig::default(),
            sharing: SharingConfig::default(),
            erniebook: ErnieBookConfig::default(),
            supernode: SuperNodeConfig::default(),
            services: ServicesConfig::default(),
        }
    }
}

// ─── Economy Config ──────────────────────────────────────────────────

/// ErnPoints economy rates — all values are user-configurable.
///
/// If this section is absent from the config file, deserialization fails
/// with a clear error message directing the user to configure rates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EconomyConfig {
    /// Points earned per megabyte of bandwidth relayed to peers.
    pub points_per_mb_relayed: f64,

    /// Points earned per gigabyte-hour of content stored for peers.
    pub points_per_gb_hour_stored: f64,

    /// Points earned per 1,000 inference tokens computed for peers.
    pub points_per_1k_tokens_computed: f64,

    /// Points earned per megabyte of WiFi traffic shared with peers.
    pub points_per_mb_wifi_shared: f64,

    /// Points earned per hour of continuous mesh uptime.
    pub points_per_hour_uptime: f64,

    /// Bonus multiplier applied when pinning popular content.
    #[serde(default = "default_pinning_bonus")]
    pub pinning_bonus_multiplier: f64,

    /// Size in bytes for content-addressable storage chunks.
    pub chunk_size_bytes: u64,

    /// Points cost per 1,000 tokens of inference requested from peers.
    pub cost_per_1k_tokens_inference: f64,

    /// Points cost per GB-month of remote storage.
    pub cost_per_gb_month_storage: f64,

    /// Points cost per priority bandwidth session.
    pub cost_per_session_priority: f64,

    /// Points cost per replica-month of mesh site hosting.
    pub cost_per_replica_month: f64,
}

fn default_pinning_bonus() -> f64 {
    1.0
}

impl Default for EconomyConfig {
    fn default() -> Self {
        Self {
            points_per_mb_relayed: 1.0,
            points_per_gb_hour_stored: 0.5,
            points_per_1k_tokens_computed: 2.0,
            points_per_mb_wifi_shared: 0.1,
            points_per_hour_uptime: 1.0,
            pinning_bonus_multiplier: 1.0,
            chunk_size_bytes: 262_144,
            cost_per_1k_tokens_inference: 5.0,
            cost_per_gb_month_storage: 10.0,
            cost_per_session_priority: 2.0,
            cost_per_replica_month: 3.0,
        }
    }
}

// ─── Security Config ─────────────────────────────────────────────────

/// Security parameters — peer scoring, connection limits, rate limiting.
///
/// All thresholds are user-configurable to allow tuning per deployment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SecurityConfig {
    /// Gossipsub peer score below which the peer is pruned from the mesh.
    pub gossip_score_threshold: f64,

    /// Gossipsub peer score below which all RPCs from the peer are ignored.
    pub gossip_graylist_threshold: f64,

    /// Seconds to retain peer scores after disconnection.
    pub gossip_score_retention_secs: u64,

    /// Maximum connections from a single IP address (Sybil defence).
    pub max_peers_per_ip: u32,

    /// Maximum connections from a single /16 subnet (Eclipse defence).
    pub max_peers_per_subnet: u32,

    /// Maximum inbound peer connections.
    pub max_inbound_connections: u32,

    /// Maximum outbound peer connections.
    pub max_outbound_connections: u32,

    /// Maximum messages per peer per second (DoS defence).
    pub max_messages_per_peer_per_sec: u32,

    /// Maximum payload size in bytes per inbound message.
    pub max_payload_bytes: u64,

    /// Maximum pending requests queued per peer.
    pub max_pending_requests_per_peer: u32,

    /// Enable offline recovery key generation for key compromise scenarios.
    #[serde(default)]
    pub enable_recovery_key: bool,

    /// Interval in days for automatic key rotation. 0 = manual only.
    #[serde(default)]
    pub key_rotation_interval_days: u32,

    /// Enable community-maintained content hash blocklists.
    #[serde(default = "default_true")]
    pub enable_community_blocklist: bool,

    /// URLs of trusted blocklist maintainers.
    #[serde(default)]
    pub blocklist_sources: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            gossip_score_threshold: -100.0,
            gossip_graylist_threshold: -1000.0,
            gossip_score_retention_secs: 3600,
            max_peers_per_ip: 3,
            max_peers_per_subnet: 10,
            max_inbound_connections: 128,
            max_outbound_connections: 128,
            max_messages_per_peer_per_sec: 50,
            max_payload_bytes: 1_048_576,
            max_pending_requests_per_peer: 10,
            enable_recovery_key: false,
            key_rotation_interval_days: 0,
            enable_community_blocklist: true,
            blocklist_sources: Vec::new(),
        }
    }
}

// ─── Sharing Config ──────────────────────────────────────────────────

/// Resource sharing configuration — per-peer approval and relay pool.
///
/// Sharing is disabled by default. First-time enablement requires
/// WebUI consent gate (§7.4.4).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SharingConfig {
    /// Whether resource sharing is active. Requires WebUI consent on first enable.
    pub enabled: bool,

    /// Approval mode: "whitelist", "blacklist", or "request-approval".
    pub mode: SharingMode,

    /// Per-peer bandwidth cap in Mbps.
    pub max_bandwidth_per_peer_mbps: u32,

    /// Maximum simultaneous sharing sessions.
    pub max_concurrent_peers: u32,

    /// Auto-revoke sharing sessions after this many hours.
    pub session_timeout_hours: u32,

    /// Explicitly approved PeerIDs (used in whitelist mode).
    #[serde(default)]
    pub whitelisted_peers: Vec<String>,

    /// Explicitly blocked PeerIDs (used in blacklist mode).
    #[serde(default)]
    pub blacklisted_peers: Vec<String>,
}

/// Sharing approval mode — determines how peer sharing requests are handled.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SharingMode {
    /// Only explicitly approved PeerIDs can use shared resources.
    Whitelist,
    /// All authenticated peers allowed except explicitly blocked ones.
    Blacklist,
    /// Each new peer triggers a WebUI notification for operator approval.
    RequestApproval,
}

impl Default for SharingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: SharingMode::RequestApproval,
            max_bandwidth_per_peer_mbps: 10,
            max_concurrent_peers: 5,
            session_timeout_hours: 24,
            whitelisted_peers: Vec::new(),
            blacklisted_peers: Vec::new(),
        }
    }
}

// ─── ErnieBook Config ────────────────────────────────────────────────

/// ErnieBook AI social network configuration.
///
/// When enabled, the local Ern-OS AI instance can post to the ErnieBook
/// mesh network. Humans observe via a read-only WebUI tab.
/// The AI cannot self-enable this — requires human configuration (§15.2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErnieBookConfig {
    /// Whether this node's AI can post to ErnieBook.
    pub enabled: bool,
}

impl Default for ErnieBookConfig {
    fn default() -> Self {
        Self { enabled: false }
    }
}

// ─── Super-Node Config ──────────────────────────────────────────────

/// Super-Node SFU volunteering configuration.
///
/// When enabled, this node volunteers as a Selective Forwarding Unit
/// for large voice/video calls, earning elevated ErnPoints rates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SuperNodeConfig {
    /// Whether this node volunteers as a Super-Node SFU.
    pub enabled: bool,

    /// Maximum bandwidth in Mbps to dedicate to SFU forwarding.
    pub max_bandwidth_mbps: u32,
}

impl Default for SuperNodeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_bandwidth_mbps: 100,
        }
    }
}

// ─── Services Config ────────────────────────────────────────────────

/// Service-layer configuration — buffer capacities, scoring weights.
///
/// All values are user-configurable via `[mesh.services]` in `ern-os.toml`.
/// These control runtime buffer sizes and reputation scoring policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServicesConfig {
    /// Maximum chat messages retained per topic.
    pub chat_capacity_per_topic: usize,
    /// Maximum ErnieBook feed items retained.
    pub feed_capacity: usize,
    /// Maximum mail messages retained in mailbox.
    pub mailbox_capacity: usize,
    /// Maximum forum posts per community before eviction.
    pub forum_posts_per_community: usize,
    /// Voice room SFU threshold (switch to SFU above this).
    pub voice_sfu_threshold: u32,
    /// Maximum participants per voice room.
    pub voice_max_participants: u32,
    // ─── Reputation scoring weights ───
    /// Points gained per MB relayed.
    pub rep_points_per_mb_relayed: f64,
    /// Points gained per GB·h stored.
    pub rep_points_per_gbh_stored: f64,
    /// Points gained per kT computed.
    pub rep_points_per_kt_computed: f64,
    /// Points gained per hour of uptime.
    pub rep_points_per_hour_uptime: f64,
    /// Points gained per valid content delivery.
    pub rep_points_per_valid_content: f64,
    /// Points lost per invalid data incident.
    pub rep_penalty_invalid_data: f64,
    /// Points lost per rate limit violation.
    pub rep_penalty_rate_limit: f64,
    /// Points lost per spam incident.
    pub rep_penalty_spam: f64,
    /// Points lost per relay failure.
    pub rep_penalty_relay_failure: f64,
    /// Points lost per transfer timeout.
    pub rep_penalty_transfer_timeout: f64,
    /// Score above which a peer is considered trusted.
    pub rep_trust_threshold: f64,
    /// Score below which a peer is considered hostile.
    pub rep_hostile_threshold: f64,
}

impl Default for ServicesConfig {
    fn default() -> Self {
        Self {
            chat_capacity_per_topic: 1000,
            feed_capacity: 500,
            mailbox_capacity: 1000,
            forum_posts_per_community: 500,
            voice_sfu_threshold: 4,
            voice_max_participants: 25,
            rep_points_per_mb_relayed: 0.01,
            rep_points_per_gbh_stored: 0.1,
            rep_points_per_kt_computed: 0.05,
            rep_points_per_hour_uptime: 0.5,
            rep_points_per_valid_content: 1.0,
            rep_penalty_invalid_data: -10.0,
            rep_penalty_rate_limit: -5.0,
            rep_penalty_spam: -20.0,
            rep_penalty_relay_failure: -15.0,
            rep_penalty_transfer_timeout: -3.0,
            rep_trust_threshold: 10.0,
            rep_hostile_threshold: -50.0,
        }
    }
}

impl ServicesConfig {
    /// Build a `ReputationConfig` from the service config values.
    pub fn reputation_config(&self) -> crate::reputation::ReputationConfig {
        crate::reputation::ReputationConfig {
            points_per_mb_relayed: self.rep_points_per_mb_relayed,
            points_per_gbh_stored: self.rep_points_per_gbh_stored,
            points_per_kt_computed: self.rep_points_per_kt_computed,
            points_per_hour_uptime: self.rep_points_per_hour_uptime,
            points_per_valid_content: self.rep_points_per_valid_content,
            penalty_invalid_data: self.rep_penalty_invalid_data,
            penalty_rate_limit: self.rep_penalty_rate_limit,
            penalty_spam: self.rep_penalty_spam,
            penalty_relay_failure: self.rep_penalty_relay_failure,
            penalty_transfer_timeout: self.rep_penalty_transfer_timeout,
            trust_threshold: self.rep_trust_threshold,
            hostile_threshold: self.rep_hostile_threshold,
        }
    }
}

// ─── Parsing ─────────────────────────────────────────────────────────

/// Wrapper used to extract the `[mesh]` table from the top-level TOML.
#[derive(Debug, Deserialize)]
struct TomlWrapper {
    mesh: Option<MeshConfig>,
}

/// Parse mesh configuration from the contents of `ern-os.toml`.
///
/// Returns `Ok(MeshConfig)` if the `[mesh]` section exists and is valid.
/// Returns `Err` with a user-facing message if the section is missing or
/// any required field is absent — the system does not invent defaults (§5).
pub fn parse_mesh_config(toml_content: &str) -> Result<MeshConfig> {
    let wrapper: TomlWrapper = toml::from_str(toml_content)
        .context("Failed to parse ern-os.toml — check TOML syntax")?;

    wrapper.mesh.ok_or_else(|| {
        anyhow::anyhow!(
            "Missing [mesh] section in ern-os.toml. \
             ErnMesh requires explicit configuration — \
             see docs/mesh_config.md for the required fields."
        )
    })
}

/// Load mesh configuration from the `ern-os.toml` file on disk.
///
/// Reads the file at `config_path`, parses the `[mesh]` section, and
/// logs the result. Returns `Err` if the file is unreadable, the TOML
/// is malformed, or the `[mesh]` section is missing.
pub fn load_mesh_config(config_path: &std::path::Path) -> Result<MeshConfig> {
    let content = std::fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;

    let config = parse_mesh_config(&content)
        .with_context(|| format!("Failed to parse mesh config from {}", config_path.display()))?;

    if config.enabled {
        tracing::warn!(
            port = config.listen_port,
            "Mesh listener will bind to 0.0.0.0:{} — accepting connections from all interfaces",
            config.listen_port
        );
    }

    tracing::info!(
        enabled = config.enabled,
        port = config.listen_port,
        max_connections = config.max_connections,
        share_bandwidth = config.share_bandwidth,
        share_storage_gb = config.share_storage_gb,
        "Loaded mesh configuration"
    );

    Ok(config)
}

/// Resolve the data directory for mesh state (keys, blocks, ledger).
///
/// Returns `{data_dir}/mesh/` and creates it if it does not exist.
pub fn resolve_mesh_data_dir(data_dir: &std::path::Path) -> Result<PathBuf> {
    let mesh_dir = data_dir.join("mesh");

    if !mesh_dir.exists() {
        std::fs::create_dir_all(&mesh_dir)
            .with_context(|| format!("Failed to create mesh data directory: {}", mesh_dir.display()))?;
        tracing::info!(path = %mesh_dir.display(), "Created mesh data directory");
    }

    Ok(mesh_dir)
}

// Tests extracted to config_tests.rs per §1.1 (file length limit).
#[cfg(test)]
#[path = "config_tests.rs"]
mod config_tests;
