// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Mesh node — Swarm orchestration, lifecycle management, and event handling.
//!
//! This module ties together all lower-level modules into a running mesh node:
//!
//! - **Identity** (PR 2): loads or generates the Ed25519 keypair
//! - **Transport** (PR 3): configures QUIC + TCP/Noise/Yamux
//! - **Protocol** (PR 4): signs/verifies/replays-protects messages
//! - **Capability** (PR 5): enforces per-peer access control
//! - **Discovery** (PR 6): tracks discovered peers
//!
//! The [`MeshNode`] struct holds all mesh state and provides the public API
//! for starting, stopping, and querying the mesh network.
//!
//! The actual libp2p `Swarm` event loop runs in a background tokio task.
//! The `MeshNode` communicates with it via channels.

use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::capability::CapabilityStore;
use crate::config::MeshConfig;
use crate::discovery::{DiscoveryConfig, PeerRegistry};
use crate::identity::NodeIdentity;
use crate::protocol::SequenceTracker;
use crate::transport::TransportConfig;

/// Status of the mesh node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    /// Node is configured but not yet started.
    Configured,
    /// Node is starting up (binding listeners, connecting bootstrap peers).
    Starting,
    /// Node is running and connected to the mesh.
    Running,
    /// Node is shutting down gracefully.
    ShuttingDown,
    /// Node has stopped.
    Stopped,
    /// Node encountered an error and is not operational.
    Error,
}

impl std::fmt::Display for NodeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeStatus::Configured => write!(f, "Configured"),
            NodeStatus::Starting => write!(f, "Starting"),
            NodeStatus::Running => write!(f, "Running"),
            NodeStatus::ShuttingDown => write!(f, "Shutting Down"),
            NodeStatus::Stopped => write!(f, "Stopped"),
            NodeStatus::Error => write!(f, "Error"),
        }
    }
}

/// A snapshot of the mesh node's current state — returned by the status API.
///
/// This is a read-only view safe to serialize for the WebUI.
#[derive(Debug, Clone)]
pub struct NodeSnapshot {
    /// Current node status.
    pub status: NodeStatus,
    /// This node's PeerId (base58).
    pub peer_id: String,
    /// Number of connected peers.
    pub connected_peers: usize,
    /// Whether bandwidth sharing is active.
    pub sharing_active: bool,
    /// Whether ErnieBook posting is enabled.
    pub erniebook_enabled: bool,
    /// Listen port.
    pub listen_port: u16,
}

/// The mesh node — holds all state and provides the public API.
///
/// Created via [`MeshNode::initialize`]. The node must be explicitly started
/// with [`MeshNode::start`] — it does not auto-start on creation.
pub struct MeshNode {
    /// The node's cryptographic identity.
    identity: NodeIdentity,

    /// Mesh configuration (from `ern-os.toml`).
    config: MeshConfig,

    /// Transport configuration (QUIC + TCP).
    transport_config: TransportConfig,

    /// Discovery configuration (Kademlia + mDNS).
    discovery_config: DiscoveryConfig,

    /// Per-peer capability grants.
    capabilities: Arc<RwLock<CapabilityStore>>,

    /// Discovered peer registry.
    peers: Arc<RwLock<PeerRegistry>>,

    /// Message sequence tracker for replay protection.
    sequence_tracker: Arc<RwLock<SequenceTracker>>,

    /// Current outbound sequence number (monotonically increasing).
    outbound_sequence: Arc<std::sync::atomic::AtomicU64>,

    /// Current node status.
    status: Arc<RwLock<NodeStatus>>,

    /// Background Swarm task handle (set after start).
    swarm_handle: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,

    /// Command channel sender for the Swarm (set after start).
    swarm_tx: Arc<RwLock<Option<crate::swarm_handle::SwarmTx>>>,
}

impl MeshNode {
    /// Initialize a mesh node from configuration.
    ///
    /// This performs all setup except actually joining the network:
    /// - Loads or generates the Ed25519 identity
    /// - Configures transport (QUIC + TCP/Noise/Yamux)
    /// - Configures discovery (Kademlia + mDNS)
    /// - Initializes empty capability store and peer registry
    ///
    /// Call [`MeshNode::start`] to begin networking.
    pub fn initialize(config: MeshConfig, data_dir: &Path) -> Result<Self> {
        tracing::info!("Initializing mesh node");

        // Resolve mesh data directory
        let mesh_data_dir = crate::config::resolve_mesh_data_dir(data_dir)
            .context("Failed to resolve mesh data directory")?;

        // Load or generate identity
        let identity = crate::identity::load_or_generate(&mesh_data_dir)
            .context("Failed to initialize mesh identity")?;

        tracing::info!(
            peer_id = %identity.peer_id(),
            "Mesh identity ready"
        );

        // Configure transport
        let transport_config = crate::transport::build_transport_config(&config)
            .context("Failed to configure mesh transport")?;

        // Configure discovery
        let discovery_config = DiscoveryConfig::from_mesh_config(&config);

        tracing::info!(
            peer_id = %identity.peer_id(),
            listen_port = config.listen_port,
            bootstrap_peers = discovery_config.bootstrap_addrs.len(),
            share_bandwidth = config.share_bandwidth,
            share_storage_gb = config.share_storage_gb,
            "Mesh node initialized — ready to start"
        );

        Ok(Self {
            identity,
            config,
            transport_config,
            discovery_config,
            capabilities: Arc::new(RwLock::new(CapabilityStore::new())),
            peers: Arc::new(RwLock::new(PeerRegistry::new())),
            sequence_tracker: Arc::new(RwLock::new(SequenceTracker::new())),
            outbound_sequence: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            status: Arc::new(RwLock::new(NodeStatus::Configured)),
            swarm_handle: Arc::new(RwLock::new(None)),
            swarm_tx: Arc::new(RwLock::new(None)),
        })
    }

    /// Returns the node's PeerId.
    pub fn peer_id(&self) -> &libp2p_identity::PeerId {
        self.identity.peer_id()
    }

    /// Returns the node's PeerId as base58 string.
    pub fn peer_id_base58(&self) -> String {
        self.identity.peer_id_base58()
    }

    /// Returns a reference to the mesh config.
    pub fn config(&self) -> &MeshConfig {
        &self.config
    }

    /// Returns a reference to the transport config.
    pub fn transport_config(&self) -> &TransportConfig {
        &self.transport_config
    }

    /// Returns a reference to the discovery config.
    pub fn discovery_config(&self) -> &DiscoveryConfig {
        &self.discovery_config
    }

    /// Returns the shared capability store.
    pub fn capabilities(&self) -> Arc<RwLock<CapabilityStore>> {
        Arc::clone(&self.capabilities)
    }

    /// Returns the shared peer registry.
    pub fn peers(&self) -> Arc<RwLock<PeerRegistry>> {
        Arc::clone(&self.peers)
    }

    /// Returns the shared sequence tracker for replay protection.
    pub fn sequence_tracker(&self) -> Arc<RwLock<SequenceTracker>> {
        Arc::clone(&self.sequence_tracker)
    }

    /// Get the next outbound sequence number (atomically incremented).
    pub fn next_sequence(&self) -> u64 {
        self.outbound_sequence
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    /// Sign a message payload and create a signed envelope.
    pub fn sign_message(
        &self,
        payload: crate::protocol::MessagePayload,
    ) -> Result<crate::protocol::SignedEnvelope> {
        let seq = self.next_sequence();
        crate::protocol::create_signed_envelope(
            self.identity.keypair(),
            self.identity.peer_id(),
            seq,
            payload,
        )
    }

    /// Get a snapshot of the current node state (for WebUI).
    pub async fn snapshot(&self) -> NodeSnapshot {
        let status = *self.status.read().await;
        let peer_count = self.peers.read().await.peer_count();

        NodeSnapshot {
            status,
            peer_id: self.peer_id_base58(),
            connected_peers: peer_count,
            sharing_active: self.config.share_bandwidth,
            erniebook_enabled: self.config.erniebook.enabled,
            listen_port: self.config.listen_port,
        }
    }

    /// Set the node status.
    pub async fn set_status(&self, status: NodeStatus) {
        let mut current = self.status.write().await;
        let prev = *current;
        *current = status;

        tracing::info!(
            from = %prev,
            to = %status,
            "Mesh node status changed"
        );
    }

    /// Start the mesh node — bind listeners and connect to bootstrap peers.
    ///
    /// This spawns a background tokio task that runs the libp2p Swarm event
    /// loop. The node transitions from `Configured` → `Starting` → `Running`.
    ///
    /// Requires shared references to the service modules for Gossipsub
    /// message dispatch.
    pub async fn start_with_services(
        &self,
        message_bus: Arc<crate::message_bus::MessageBus>,
        mailbox: Arc<RwLock<crate::mesh_mail::Mailbox>>,
        forums: Arc<RwLock<crate::forum::ForumStore>>,
        voice: Arc<RwLock<crate::voice::VoiceManager>>,
    ) -> Result<()> {
        self.set_status(NodeStatus::Starting).await;

        tracing::info!(
            peer_id = %self.peer_id(),
            quic_addr = %self.transport_config.quic_listen_addr(),
            tcp_addr = %self.transport_config.tcp_listen_addr(),
            bootstrap_peers = self.discovery_config.bootstrap_addrs.len(),
            "Starting mesh node — spawning Swarm"
        );

        let (handle, tx) = crate::swarm::build_and_spawn_swarm(
            self.identity.keypair().clone(),
            &self.config,
            Arc::clone(&self.peers),
            message_bus,
            mailbox,
            forums,
            voice,
            &self.discovery_config.bootstrap_addrs,
        ).await?;

        // Subscribe to default topics
        let peer_id = self.peer_id_base58();
        let default_topics = vec![
            crate::erniebook::ERNIEBOOK_TOPIC.to_string(),
            format!("ernmesh/mail/{}", peer_id),
        ];
        for topic in default_topics {
            let _ = tx.send(crate::swarm_handle::SwarmCommand::Subscribe {
                topic,
            }).await;
        }

        *self.swarm_handle.write().await = Some(handle);
        *self.swarm_tx.write().await = Some(tx);

        self.set_status(NodeStatus::Running).await;

        tracing::info!(peer_id = %self.peer_id(), "Mesh node started — Swarm running");

        Ok(())
    }

    /// Legacy start (for tests) — does NOT spawn Swarm.
    pub async fn start(&self) -> Result<()> {
        self.set_status(NodeStatus::Starting).await;
        self.set_status(NodeStatus::Running).await;
        Ok(())
    }

    /// Stop the mesh node gracefully.
    pub async fn stop(&self) -> Result<()> {
        self.set_status(NodeStatus::ShuttingDown).await;

        tracing::info!(peer_id = %self.peer_id(), "Stopping mesh node");

        // Abort the Swarm event loop task
        if let Some(handle) = self.swarm_handle.write().await.take() {
            handle.abort();
            tracing::info!("Swarm event loop aborted");
        }
        *self.swarm_tx.write().await = None;

        self.set_status(NodeStatus::Stopped).await;

        tracing::info!(peer_id = %self.peer_id(), "Mesh node stopped");

        Ok(())
    }

    /// Get the SwarmTx (if started), for publishing from handlers.
    pub fn swarm_tx(&self) -> Arc<RwLock<Option<crate::swarm_handle::SwarmTx>>> {
        Arc::clone(&self.swarm_tx)
    }
}

// Tests extracted to node_tests.rs per §1.1 (file length limit).
#[cfg(test)]
#[path = "node_tests.rs"]
mod node_tests;
