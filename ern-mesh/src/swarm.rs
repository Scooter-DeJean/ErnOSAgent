// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Swarm lifecycle — builds and runs the libp2p Swarm event loop.
//!
//! The Swarm is the core runtime of the mesh network. It combines:
//! - The transport stack (QUIC + TCP/Noise/Yamux) from `transport.rs`
//! - The composed behaviour (Gossipsub + Kademlia + mDNS + Identify + Ping)
//!   from `behaviour.rs`
//! - The node's Ed25519 identity from `identity.rs`
//!
//! The Swarm event loop runs in a background tokio task. Events are processed
//! by [`handle_swarm_event`] which updates the [`PeerRegistry`] and logs
//! all discovery/connectivity changes.
//!
//! The public API is [`build_and_spawn_swarm`], which creates the Swarm,
//! binds listeners, connects to bootstrap peers, and spawns the event loop.

use anyhow::{Context, Result};
use libp2p::futures::StreamExt;
use libp2p::{Multiaddr, Swarm, SwarmBuilder};
use libp2p::swarm::SwarmEvent;
use libp2p_identity::Keypair;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::behaviour::{MeshBehaviour, MeshBehaviourEvent};
use crate::config::MeshConfig;
use crate::discovery::{DiscoveryMethod, PeerRegistry};
use crate::message_bus::{IncomingMessage, MessageBus};
use crate::swarm_handle::{SwarmCommand, SwarmRx, SwarmTx};

/// Build the libp2p Swarm with the full transport + behaviour stack.
///
/// Uses `SwarmBuilder` with:
/// - Existing Ed25519 identity (from `identity.rs`)
/// - Tokio async runtime
/// - TCP + Noise + Yamux (encrypted, multiplexed TCP)
/// - QUIC (built-in TLS 1.3 encryption)
/// - Composed `MeshBehaviour`
pub fn build_swarm(
    keypair: Keypair,
    config: &MeshConfig,
) -> Result<Swarm<MeshBehaviour>> {
    let behaviour = crate::behaviour::build_behaviour(&keypair, config)
        .context("Failed to build mesh behaviour")?;

    let swarm = SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            libp2p::tcp::Config::default().nodelay(true),
            libp2p::noise::Config::new,
            libp2p::yamux::Config::default,
        )
        .map_err(|e| anyhow::anyhow!("Failed to configure TCP transport: {}", e))?
        .with_quic()
        .with_behaviour(|_| Ok(behaviour))
        .map_err(|e| anyhow::anyhow!("Failed to attach behaviour: {}", e))?
        .with_swarm_config(|cfg| {
            cfg.with_idle_connection_timeout(std::time::Duration::from_secs(60))
        })
        .build();

    tracing::info!("Swarm built with TCP/Noise/Yamux + QUIC transports");

    Ok(swarm)
}

/// Build, bind listeners, connect bootstrap, and spawn the event loop.
///
/// Returns `(JoinHandle, SwarmTx)`. The `SwarmTx` lets handlers
/// send publish/subscribe commands to the running Swarm.
pub async fn build_and_spawn_swarm(
    keypair: Keypair,
    config: &MeshConfig,
    peers: Arc<RwLock<PeerRegistry>>,
    message_bus: Arc<MessageBus>,
    mailbox: Arc<RwLock<crate::mesh_mail::Mailbox>>,
    forums: Arc<RwLock<crate::forum::ForumStore>>,
    voice: Arc<RwLock<crate::voice::VoiceManager>>,
    bootstrap_addrs: &[Multiaddr],
) -> Result<(tokio::task::JoinHandle<()>, SwarmTx)> {
    let mut swarm = build_swarm(keypair, config)?;

    // Bind QUIC listener
    let quic_addr: Multiaddr = format!("/ip4/0.0.0.0/udp/{}/quic-v1", config.listen_port)
        .parse()
        .context("Invalid QUIC listen address")?;
    swarm.listen_on(quic_addr.clone())
        .context("Failed to bind QUIC listener")?;
    tracing::info!(addr = %quic_addr, "QUIC listener bound");

    // Bind TCP listener
    let tcp_addr: Multiaddr = format!("/ip4/0.0.0.0/tcp/{}", config.listen_port)
        .parse()
        .context("Invalid TCP listen address")?;
    swarm.listen_on(tcp_addr.clone())
        .context("Failed to bind TCP listener")?;
    tracing::info!(addr = %tcp_addr, "TCP listener bound");

    // Dial bootstrap peers
    for addr in bootstrap_addrs {
        match swarm.dial(addr.clone()) {
            Ok(_) => tracing::info!(addr = %addr, "Dialing bootstrap peer"),
            Err(e) => tracing::warn!(addr = %addr, error = %e, "Failed to dial bootstrap peer"),
        }
    }

    let (tx, rx) = crate::swarm_handle::command_channel();

    // Spawn event loop
    let handle = tokio::spawn(async move {
        run_event_loop(swarm, peers, message_bus, mailbox, forums, voice, rx).await;
    });

    tracing::info!("Swarm event loop spawned");

    Ok((handle, tx))
}

/// The main Swarm event loop — processes libp2p events + handler commands.
///
/// Runs indefinitely until the task is aborted. Uses `tokio::select!`
/// to handle both Swarm events and SwarmCommand messages from handlers.
async fn run_event_loop(
    mut swarm: Swarm<MeshBehaviour>,
    peers: Arc<RwLock<PeerRegistry>>,
    message_bus: Arc<MessageBus>,
    mailbox: Arc<RwLock<crate::mesh_mail::Mailbox>>,
    forums: Arc<RwLock<crate::forum::ForumStore>>,
    voice: Arc<RwLock<crate::voice::VoiceManager>>,
    mut rx: SwarmRx,
) {
    tracing::info!("Swarm event loop started (with command channel)");

    loop {
        tokio::select! {
            event = swarm.select_next_some() => {
                handle_swarm_event(
                    &mut swarm, &peers, &message_bus,
                    &mailbox, &forums, &voice, event,
                ).await;
            }
            Some(cmd) = rx.recv() => {
                handle_swarm_command(&mut swarm, cmd);
            }
        }
    }
}

/// Handle a SwarmCommand from HTTP handlers.
fn handle_swarm_command(
    swarm: &mut Swarm<MeshBehaviour>,
    cmd: SwarmCommand,
) {
    match cmd {
        SwarmCommand::Publish { topic, data } => {
            let gs_topic = libp2p::gossipsub::IdentTopic::new(&topic);
            match swarm.behaviour_mut().gossipsub.publish(gs_topic, data) {
                Ok(mid) => tracing::debug!(%topic, %mid, "Published to Gossipsub"),
                Err(e) => tracing::warn!(%topic, error = %e, "Gossipsub publish failed"),
            }
        }
        SwarmCommand::Subscribe { topic } => {
            let gs_topic = libp2p::gossipsub::IdentTopic::new(&topic);
            match swarm.behaviour_mut().gossipsub.subscribe(&gs_topic) {
                Ok(_) => tracing::info!(%topic, "Subscribed to Gossipsub topic"),
                Err(e) => tracing::warn!(%topic, error = %e, "Subscribe failed"),
            }
        }
        SwarmCommand::Unsubscribe { topic } => {
            let gs_topic = libp2p::gossipsub::IdentTopic::new(&topic);
            if swarm.behaviour_mut().gossipsub.unsubscribe(&gs_topic) {
                tracing::info!(%topic, "Unsubscribed from Gossipsub topic");
            } else {
                tracing::warn!(%topic, "Unsubscribe failed (not subscribed)");
            }
        }
    }
}

/// Handle a single Swarm event.
async fn handle_swarm_event(
    swarm: &mut Swarm<MeshBehaviour>,
    peers: &Arc<RwLock<PeerRegistry>>,
    message_bus: &Arc<MessageBus>,
    mailbox: &Arc<RwLock<crate::mesh_mail::Mailbox>>,
    forums: &Arc<RwLock<crate::forum::ForumStore>>,
    voice: &Arc<RwLock<crate::voice::VoiceManager>>,
    event: SwarmEvent<MeshBehaviourEvent>,
) {
    match event {
        SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
            tracing::info!(peer_id = %peer_id, endpoint = ?endpoint, "Peer connected");
        }
        SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
            tracing::info!(peer_id = %peer_id, cause = ?cause, "Peer disconnected");
            peers.write().await.remove_peer(&peer_id);
        }
        SwarmEvent::NewListenAddr { address, .. } => {
            tracing::info!(addr = %address, "Listening on");
        }
        SwarmEvent::ListenerError { error, .. } => {
            tracing::error!(error = %error, "Listener error");
        }
        SwarmEvent::Behaviour(behaviour_event) => {
            handle_behaviour_event(
                swarm, peers, message_bus, mailbox, forums, voice,
                behaviour_event,
            ).await;
        }
        other => {
            tracing::debug!(event = ?other, "Swarm event");
        }
    }
}

/// Handle a behaviour-level event from one of the sub-protocols.
async fn handle_behaviour_event(
    swarm: &mut Swarm<MeshBehaviour>,
    peers: &Arc<RwLock<PeerRegistry>>,
    message_bus: &Arc<MessageBus>,
    mailbox: &Arc<RwLock<crate::mesh_mail::Mailbox>>,
    forums: &Arc<RwLock<crate::forum::ForumStore>>,
    voice: &Arc<RwLock<crate::voice::VoiceManager>>,
    event: MeshBehaviourEvent,
) {
    match event {
        MeshBehaviourEvent::Mdns(mdns_event) => {
            handle_mdns_event(swarm, peers, mdns_event).await;
        }
        MeshBehaviourEvent::Gossipsub(gs_event) => {
            handle_gossipsub_event(message_bus, mailbox, forums, voice, gs_event).await;
        }
        MeshBehaviourEvent::Kademlia(kad_event) => {
            tracing::debug!(event = ?kad_event, "Kademlia event");
        }
        MeshBehaviourEvent::Identify(identify_event) => {
            handle_identify_event(swarm, identify_event);
        }
        MeshBehaviourEvent::Ping(ping_event) => {
            handle_ping_event(ping_event);
        }
    }
}

/// Handle mDNS events — add discovered peers to Kademlia and registry.
async fn handle_mdns_event(
    swarm: &mut Swarm<MeshBehaviour>,
    peers: &Arc<RwLock<PeerRegistry>>,
    event: libp2p::mdns::Event,
) {
    use libp2p::mdns::Event;

    match event {
        Event::Discovered(list) => {
            for (peer_id, addr) in list {
                tracing::info!(
                    peer_id = %peer_id,
                    addr = %addr,
                    "mDNS: discovered local peer"
                );

                // Add to Kademlia routing table
                swarm.behaviour_mut().kademlia.add_address(&peer_id, addr.clone());

                // Add to peer registry
                peers.write().await.add_peer(
                    peer_id,
                    vec![addr],
                    DiscoveryMethod::Mdns,
                );
            }
        }
        Event::Expired(list) => {
            for (peer_id, addr) in list {
                tracing::debug!(
                    peer_id = %peer_id,
                    addr = %addr,
                    "mDNS: peer expired"
                );
            }
        }
    }
}

/// Handle Gossipsub events — dispatch incoming messages to service modules.
async fn handle_gossipsub_event(
    message_bus: &Arc<MessageBus>,
    mailbox: &Arc<RwLock<crate::mesh_mail::Mailbox>>,
    forums: &Arc<RwLock<crate::forum::ForumStore>>,
    voice: &Arc<RwLock<crate::voice::VoiceManager>>,
    event: libp2p::gossipsub::Event,
) {
    use libp2p::gossipsub::Event;

    match event {
        Event::Message { propagation_source, message, message_id, .. } => {
            let topic = message.topic.as_str().to_string();
            let source = propagation_source.to_base58();
            tracing::info!(%source, %topic, %message_id, "Gossipsub: dispatching");

            // Chat + ErnieBook — routed by MessageBus
            if topic.starts_with("ernmesh/chat/") || topic.starts_with("ernmesh/dm/")
                || topic == crate::erniebook::ERNIEBOOK_TOPIC
            {
                let incoming = IncomingMessage {
                    topic, data: message.data, source,
                };
                if let Err(e) = message_bus.handle_incoming(incoming).await {
                    tracing::warn!(error = %e, "Gossipsub dispatch failed");
                }
                return;
            }

            // Mail — direct to mailbox
            if topic.starts_with("ernmesh/mail/") {
                match serde_json::from_slice::<crate::mesh_mail::MailMessage>(&message.data) {
                    Ok(msg) => mailbox.write().await.receive(msg),
                    Err(e) => tracing::warn!(error = %e, "Bad mail message"),
                }
                return;
            }

            // Forum — direct to forum store
            if topic.starts_with("ernmesh/forum/") {
                match serde_json::from_slice::<crate::forum::ForumPost>(&message.data) {
                    Ok(post) => { let _ = forums.write().await.add_post(post); }
                    Err(e) => tracing::warn!(error = %e, "Bad forum post"),
                }
                return;
            }

            // Voice signalling
            if topic.starts_with("ernmesh/voice/") {
                match serde_json::from_slice::<crate::voice::VoiceSignal>(&message.data) {
                    Ok(signal) => dispatch_voice_signal(voice, signal).await,
                    Err(e) => tracing::warn!(error = %e, "Bad voice signal"),
                }
                return;
            }

            tracing::debug!(%topic, "Unknown Gossipsub topic — dropped");
        }
        Event::Subscribed { peer_id, topic } => {
            tracing::info!(peer_id = %peer_id, topic = %topic, "Peer subscribed");
        }
        Event::Unsubscribed { peer_id, topic } => {
            tracing::debug!(peer_id = %peer_id, topic = %topic, "Peer unsubscribed");
        }
        other => {
            tracing::debug!(event = ?other, "Gossipsub event");
        }
    }
}

/// Apply a voice signalling message to the local voice manager.
async fn dispatch_voice_signal(
    voice: &Arc<RwLock<crate::voice::VoiceManager>>,
    signal: crate::voice::VoiceSignal,
) {
    let mut vm = voice.write().await;
    match signal {
        crate::voice::VoiceSignal::Join { room_id, peer_id } => {
            if let Some(room) = vm.get_room_mut(&room_id) {
                let _ = room.join(&peer_id);
            }
        }
        crate::voice::VoiceSignal::Leave { room_id, peer_id } => {
            if let Some(room) = vm.get_room_mut(&room_id) {
                room.leave(&peer_id);
            }
        }
        _ => { tracing::debug!(signal = ?signal, "Voice signal (unhandled)"); }
    }
}

/// Handle Identify events — add identified peer addresses to Kademlia.
fn handle_identify_event(
    swarm: &mut Swarm<MeshBehaviour>,
    event: libp2p::identify::Event,
) {
    use libp2p::identify::Event;

    match event {
        Event::Received { peer_id, info, .. } => {
            tracing::info!(
                peer_id = %peer_id,
                protocol_version = %info.protocol_version,
                agent_version = %info.agent_version,
                protocols = ?info.protocols,
                addrs = ?info.listen_addrs,
                "Identify: received peer info"
            );

            // Add all advertised addresses to Kademlia
            for addr in &info.listen_addrs {
                swarm.behaviour_mut().kademlia.add_address(&peer_id, addr.clone());
            }
        }
        Event::Sent { peer_id, .. } => {
            tracing::debug!(peer_id = %peer_id, "Identify: sent info");
        }
        other => {
            tracing::debug!(event = ?other, "Identify event");
        }
    }
}

/// Handle Ping events — log RTT for connected peers.
fn handle_ping_event(event: libp2p::ping::Event) {
    match event.result {
        Ok(rtt) => {
            tracing::debug!(
                peer_id = %event.peer,
                rtt_ms = rtt.as_millis() as u64,
                "Ping: pong received"
            );
        }
        Err(e) => {
            tracing::warn!(
                peer_id = %event.peer,
                error = %e,
                "Ping: failed"
            );
        }
    }
}

// Tests extracted to swarm_tests.rs per §1.1 (file length limit).
#[cfg(test)]
#[path = "swarm_tests.rs"]
mod tests;

