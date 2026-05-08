// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Mesh runtime — the master orchestrator that wires all modules together.
//!
//! The runtime owns all application-layer state and provides a unified API
//! for the WebUI, Android bridge, and CLI to interact with the mesh:
//!
//! ```text
//! ┌─────────────────────── MeshRuntime ───────────────────────┐
//! │                                                           │
//! │  MeshNode (identity, transport, discovery, capability)    │
//! │  ├── ChatLog (chat.rs)                                    │
//! │  ├── Feed (erniebook.rs)                                  │
//! │  ├── MessageBus (message_bus.rs)                           │
//! │  ├── Ledger + Accountant (economy.rs + accounting.rs)     │
//! │  ├── ContentStore (content_store.rs)                       │
//! │  ├── RelayPool (relay.rs)                                 │
//! │  ├── SessionManager (session.rs)                          │
//! │  ├── SiteRegistry (mesh_site.rs)                          │
//! │  ├── TransferManager (file_transfer.rs)                   │
//! │  ├── Mailbox (mesh_mail.rs)                               │
//! │  ├── VoiceManager (voice.rs)                              │
//! │  ├── ForumStore (forum.rs)                                │
//! │  ├── ReputationEngine (reputation.rs)                     │
//! │  └── StateDir (persistence.rs)                            │
//! │                                                           │
//! └───────────────────────────────────────────────────────────┘
//! ```
//!
//! All state is behind `Arc<RwLock<T>>` for concurrent access from
//! the Swarm event loop, WebUI HTTP handlers, and background tasks.

use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::accounting::Accountant;
use crate::chat::ChatLog;
use crate::config::MeshConfig;
use crate::contacts::ContactBook;
use crate::content_store::ContentStore;
use crate::economy::Ledger;
use crate::erniebook::Feed;
use crate::file_transfer::TransferManager;
use crate::forum::ForumStore;
use crate::groups::GroupStore;
use crate::mesh_mail::Mailbox;
use crate::mesh_site::SiteRegistry;
use crate::message_bus::MessageBus;
use crate::node::MeshNode;
use crate::offline_queue::OfflineQueue;
use crate::persistence::StateDir;
use crate::relay::RelayPool;
use crate::reputation::ReputationEngine;
use crate::session::SessionManager;
use crate::voice::VoiceManager;

/// The mesh runtime — owns all module state and provides the unified API.
pub struct MeshRuntime {
    /// The core mesh node (identity, transport, capability).
    node: MeshNode,

    /// Chat message log.
    pub chat_log: Arc<RwLock<ChatLog>>,

    /// ErnieBook social feed.
    pub feed: Arc<RwLock<Feed>>,

    /// Message bus (Gossipsub → Chat/ErnieBook dispatch).
    pub message_bus: Arc<MessageBus>,

    /// Economy ledger.
    pub ledger: Arc<RwLock<Ledger>>,

    /// Economy accountant (earn/spend triggers).
    pub accountant: Arc<Accountant>,

    /// Content-addressable storage.
    pub content_store: Arc<RwLock<ContentStore>>,

    /// Relay pool (bandwidth/WiFi sharing).
    pub relay_pool: Arc<RwLock<RelayPool>>,

    /// E2E session manager.
    pub sessions: Arc<RwLock<SessionManager>>,

    /// Mesh site registry.
    pub sites: Arc<RwLock<SiteRegistry>>,

    /// File transfer manager.
    pub transfers: Arc<RwLock<TransferManager>>,

    /// Mailbox.
    pub mailbox: Arc<RwLock<Mailbox>>,

    /// Voice channel manager.
    pub voice: Arc<RwLock<VoiceManager>>,

    /// Forum store.
    pub forums: Arc<RwLock<ForumStore>>,

    /// Peer reputation engine.
    pub reputation: Arc<RwLock<ReputationEngine>>,

    /// Contact book — peer contacts with presence tracking.
    pub contacts: Arc<RwLock<ContactBook>>,

    /// Group store — multi-member group chat.
    pub groups: Arc<RwLock<GroupStore>>,

    /// Offline message queue — store-and-forward for disconnected peers.
    pub offline_queue: Arc<RwLock<OfflineQueue>>,

    /// State persistence.
    pub state_dir: Arc<StateDir>,
}

impl MeshRuntime {
    /// Create a new mesh runtime from configuration.
    ///
    /// This initializes all modules with their default (empty) state.
    /// Call [`load_state`] to restore persisted state, then [`start`] to
    /// begin networking.
    pub fn initialize(config: MeshConfig, data_dir: &Path) -> Result<Self> {
        tracing::info!("Initializing mesh runtime — wiring all modules");

        // Core node (identity, transport, discovery, capability)
        let node = MeshNode::initialize(config.clone(), data_dir)
            .context("Failed to initialize mesh node")?;

        // Chat + ErnieBook — capacities from [mesh.services] config
        let svc = &config.services;
        let chat_log = Arc::new(RwLock::new(ChatLog::new(svc.chat_capacity_per_topic)));
        let feed = Arc::new(RwLock::new(Feed::new(svc.feed_capacity)));

        // Message bus
        let message_bus = Arc::new(MessageBus::new(
            Arc::clone(&chat_log),
            Arc::clone(&feed),
        ));

        // Economy
        let ledger = Arc::new(RwLock::new(Ledger::new(config.economy.clone())));
        let accountant = Arc::new(Accountant::new(Arc::clone(&ledger)));

        // Content store
        let content_dir = data_dir.join("mesh").join("content");
        let content_store = Arc::new(RwLock::new(
            ContentStore::open(&content_dir)
                .context("Failed to open content store")?,
        ));

        // Relay pool
        let relay_pool = Arc::new(RwLock::new(
            RelayPool::new(config.sharing.clone()),
        ));

        // Sessions, sites, transfers, mailbox, voice, forums
        let sessions = Arc::new(RwLock::new(SessionManager::new()));
        let sites = Arc::new(RwLock::new(SiteRegistry::new()));
        let transfers = Arc::new(RwLock::new(TransferManager::new()));
        let mailbox = Arc::new(RwLock::new(Mailbox::new(svc.mailbox_capacity)));
        let voice = Arc::new(RwLock::new(VoiceManager::new()));
        let forums = Arc::new(RwLock::new(ForumStore::new(svc.forum_posts_per_community)));

        // Contacts, groups, offline queue
        let contacts = Arc::new(RwLock::new(ContactBook::default_limits()));
        let groups = Arc::new(RwLock::new(GroupStore::default_limits()));
        let offline_queue = Arc::new(RwLock::new(OfflineQueue::default_limits()));

        // Reputation engine — weights from [mesh.services] config
        let reputation = Arc::new(RwLock::new(
            ReputationEngine::new(svc.reputation_config()),
        ));

        // State persistence
        let state_dir = Arc::new(
            StateDir::open(data_dir)
                .context("Failed to open state directory")?,
        );

        tracing::info!(
            peer_id = %node.peer_id_base58(),
            modules = 18,
            "Mesh runtime initialized — all modules wired"
        );

        Ok(Self {
            node,
            chat_log,
            feed,
            message_bus,
            ledger,
            accountant,
            content_store,
            relay_pool,
            sessions,
            sites,
            transfers,
            mailbox,
            voice,
            forums,
            reputation,
            contacts,
            groups,
            offline_queue,
            state_dir,
        })
    }

    /// Access the core mesh node.
    pub fn node(&self) -> &MeshNode {
        &self.node
    }

    /// Get the PeerId (base58 string).
    pub fn peer_id(&self) -> String {
        self.node.peer_id_base58()
    }

    /// Start the mesh — begin networking and background tasks.
    pub async fn start(&self) -> Result<()> {
        tracing::info!("Starting mesh runtime — spawning Swarm with service dispatch");
        self.node.start_with_services(
            Arc::clone(&self.message_bus),
            Arc::clone(&self.mailbox),
            Arc::clone(&self.forums),
            Arc::clone(&self.voice),
        ).await?;
        tracing::info!("Mesh runtime started — all services active");
        Ok(())
    }

    /// Stop the mesh — shutdown networking and persist state.
    pub async fn stop(&self) -> Result<()> {
        tracing::info!("Stopping mesh runtime");
        self.node.stop().await?;
        tracing::info!("Mesh runtime stopped");
        Ok(())
    }

    /// Get the SwarmTx for publishing from HTTP handlers.
    pub fn swarm_tx(&self) -> Arc<tokio::sync::RwLock<Option<crate::swarm_handle::SwarmTx>>> {
        self.node.swarm_tx()
    }

    /// Get a comprehensive dashboard snapshot of all modules.
    pub async fn dashboard_snapshot(&self) -> RuntimeSnapshot {
        let node_snap = self.node.snapshot().await;
        let ledger = self.ledger.read().await;
        let sessions = self.sessions.read().await;
        let sites = self.sites.read().await;
        let transfers = self.transfers.read().await;
        let mailbox = self.mailbox.read().await;
        let voice = self.voice.read().await;
        let forums = self.forums.read().await;
        let reputation = self.reputation.read().await;
        let chat_log = self.chat_log.read().await;
        let feed = self.feed.read().await;
        let relay = self.relay_pool.read().await;

        RuntimeSnapshot {
            peer_id: node_snap.peer_id,
            status: format!("{}", node_snap.status),
            connected_peers: node_snap.connected_peers,
            listen_port: node_snap.listen_port,
            balance: ledger.balance(),
            transactions: ledger.transaction_count(),
            active_sessions: sessions.active_session_count(),
            hosted_sites: sites.count(),
            active_transfers: transfers.active_count(),
            unread_mail: mailbox.unread_count(),
            inbox_count: mailbox.inbox_count(),
            voice_rooms: voice.room_count(),
            voice_participants: voice.total_participants(),
            forum_communities: forums.community_count(),
            forum_posts: forums.total_post_count(),
            known_peers_reputation: reputation.peer_count(),
            trusted_peers: reputation.trusted_count(),
            hostile_peers: reputation.hostile_count(),
            chat_messages: chat_log.total_messages(),
            feed_posts: feed.count(),
            relay_sessions: relay.active_session_count(),
        }
    }
}

/// A snapshot of the entire runtime state — for the WebUI dashboard.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RuntimeSnapshot {
    // Core
    pub peer_id: String,
    pub status: String,
    pub connected_peers: usize,
    pub listen_port: u16,

    // Economy
    pub balance: f64,
    pub transactions: usize,

    // Sessions
    pub active_sessions: usize,

    // Sites
    pub hosted_sites: usize,

    // Transfers
    pub active_transfers: usize,

    // Mail
    pub unread_mail: usize,
    pub inbox_count: usize,

    // Voice
    pub voice_rooms: usize,
    pub voice_participants: usize,

    // Forums
    pub forum_communities: usize,
    pub forum_posts: usize,

    // Reputation
    pub known_peers_reputation: usize,
    pub trusted_peers: usize,
    pub hostile_peers: usize,

    // Chat
    pub chat_messages: usize,

    // ErnieBook
    pub feed_posts: usize,

    // Relay
    pub relay_sessions: usize,
}

// Tests extracted to runtime_tests.rs per §1.1 (file length limit).
#[cfg(test)]
#[path = "runtime_tests.rs"]
mod runtime_tests;
