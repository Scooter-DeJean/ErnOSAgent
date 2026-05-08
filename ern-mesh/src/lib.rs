// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! ErnMesh — Decentralised mesh network layer for Ern-OS.
//!
//! This crate provides the P2P mesh networking stack: identity management,
//! peer discovery, encrypted transport, content-addressable storage,
//! the ErnPoints contribution economy, and mesh service modules.
//!
//! All public types and functions are re-exported from this root module.

pub mod accounting;
pub mod api;
pub mod behaviour;
pub mod capability;
pub mod chat;
pub mod config;
pub mod contacts;
pub mod content_store;
pub mod dashboard;
pub mod discovery;
pub mod economy;
pub mod encryption;
pub mod erniebook;
pub mod file_transfer;
pub mod forum;
pub mod groups;
pub mod identity;
pub mod integration;
pub mod mesh_mail;
pub mod mesh_site;
pub mod message_bus;
pub mod node;
pub mod offline_queue;
pub mod persistence;
pub mod protocol;
pub mod relay;
pub mod reputation;
pub mod runtime;
pub mod session;
pub mod swarm;
pub mod swarm_handle;
pub mod transport;
pub mod voice;
