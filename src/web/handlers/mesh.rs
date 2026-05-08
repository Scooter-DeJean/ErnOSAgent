//! Mesh network handler — REST API for the WebUI mesh dashboard.
//!
//! Serves real data from all 15 mesh service modules via MeshRuntime.
//! Also provides a toggle endpoint so non-technical users can
//! enable/disable the mesh without editing ern-os.toml.

use crate::web::state::AppState;
use axum::{extract::State, Json};

/// GET /api/mesh/status — mesh runtime status snapshot for the WebUI.
pub async fn mesh_status(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else {
        return Json(serde_json::json!({
            "enabled": false,
            "status": "Not Configured",
            "message": "Enable the mesh network from the Mesh tab toggle.",
        }));
    };

    let snap = runtime.node().snapshot().await;

    Json(serde_json::json!({
        "enabled": true,
        "status": snap.status.to_string(),
        "peer_id": snap.peer_id,
        "listen_port": snap.listen_port,
        "connected_peers": snap.connected_peers,
        "sharing_active": snap.sharing_active,
        "erniebook_enabled": snap.erniebook_enabled,
    }))
}

/// GET /api/mesh/dashboard — full service dashboard with all module stats.
pub async fn mesh_dashboard(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else {
        return Json(serde_json::json!({
            "enabled": false,
        }));
    };

    let snap = runtime.dashboard_snapshot().await;

    Json(serde_json::json!({
        "enabled": true,
        "peer_id": snap.peer_id,
        "status": snap.status,
        "connected_peers": snap.connected_peers,
        "listen_port": snap.listen_port,
        "balance": snap.balance,
        "transactions": snap.transactions,
        "active_sessions": snap.active_sessions,
        "hosted_sites": snap.hosted_sites,
        "active_transfers": snap.active_transfers,
        "unread_mail": snap.unread_mail,
        "inbox_count": snap.inbox_count,
        "voice_rooms": snap.voice_rooms,
        "voice_participants": snap.voice_participants,
        "forum_communities": snap.forum_communities,
        "forum_posts": snap.forum_posts,
        "known_peers_reputation": snap.known_peers_reputation,
        "trusted_peers": snap.trusted_peers,
        "hostile_peers": snap.hostile_peers,
        "chat_messages": snap.chat_messages,
        "feed_posts": snap.feed_posts,
        "relay_sessions": snap.relay_sessions,
    }))
}

/// GET /api/mesh/peers — list all discovered peers.
pub async fn mesh_peers(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else {
        return Json(serde_json::json!({
            "peers": [],
            "total": 0,
        }));
    };

    let peers = runtime.node().peers();
    let peers_guard = peers.read().await;
    let peer_list: Vec<serde_json::Value> = peers_guard.peer_ids().iter().map(|pid| {
        if let Some(peer) = peers_guard.get(pid) {
            serde_json::json!({
                "peer_id": pid.to_base58(),
                "method": peer.method.to_string(),
                "addrs": peer.addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
            })
        } else {
            serde_json::json!({
                "peer_id": pid.to_base58(),
            })
        }
    }).collect();

    let total = peer_list.len();
    Json(serde_json::json!({
        "peers": peer_list,
        "total": total,
    }))
}

/// GET /api/mesh/capabilities — list all capability grants.
pub async fn mesh_capabilities(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else {
        return Json(serde_json::json!({
            "capabilities": {},
        }));
    };

    let caps = runtime.node().capabilities();
    let caps_guard = caps.read().await;

    let peer_count = caps_guard.peer_count();

    Json(serde_json::json!({
        "peer_count": peer_count,
        "available_capabilities": ern_mesh::capability::Capability::all()
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>(),
    }))
}

/// PUT /api/mesh/toggle — enable or disable the mesh network at runtime.
///
/// When enabling: initializes MeshRuntime from config (creates default
/// config if none exists). When disabling: drops the runtime.
/// Non-technical users use this instead of editing ern-os.toml.
pub async fn mesh_toggle(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let mut guard = state.mesh_runtime.write().await;

    if guard.is_some() {
        // Disable: stop networking then drop the runtime
        if let Some(ref runtime) = *guard {
            if let Err(e) = runtime.stop().await {
                tracing::warn!(error = %e, "Error stopping mesh runtime");
            }
        }
        *guard = None;
        tracing::info!("Mesh network disabled via WebUI toggle");
        Json(serde_json::json!({
            "ok": true,
            "enabled": false,
            "message": "Mesh network disabled.",
        }))
    } else {
        // Enable: try to initialize from config (or create default)
        let config = {
            let cfg = state.config.clone();
            cfg.mesh.clone().unwrap_or_else(|| {
                ern_mesh::config::MeshConfig::default()
            })
        };
        let data_dir = state.config.general.data_dir.clone();
        let data_path = std::path::Path::new(&data_dir);

        match ern_mesh::runtime::MeshRuntime::initialize(config, data_path) {
            Ok(runtime) => {
                let peer_id = runtime.peer_id();

                // Start Swarm networking
                if let Err(e) = runtime.start().await {
                    tracing::error!(error = %e, "Failed to start mesh Swarm");
                    return Json(serde_json::json!({
                        "ok": false,
                        "enabled": false,
                        "message": format!("Mesh initialized but Swarm failed: {}", e),
                    }));
                }

                tracing::info!(
                    %peer_id,
                    "Mesh network enabled via WebUI toggle — Swarm running"
                );
                *guard = Some(runtime);
                Json(serde_json::json!({
                    "ok": true,
                    "enabled": true,
                    "peer_id": peer_id,
                    "message": "Mesh network enabled.",
                }))
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to enable mesh network");
                Json(serde_json::json!({
                    "ok": false,
                    "enabled": false,
                    "message": format!("Failed to enable mesh: {}", e),
                }))
            }
        }
    }
}
