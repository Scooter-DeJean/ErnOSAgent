// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Mesh social service handlers — contacts, groups, and offline queue.
//!
//! Each handler reads/writes from `MeshRuntime` service modules via
//! `state.mesh_runtime`. Returns 503 if mesh is not enabled.
//! All POST bodies are validated and size-limited (64KB via router layer).

use crate::web::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;

type R = Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)>;

fn mesh_disabled() -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({
        "error": "Mesh network is not enabled"
    })))
}

fn bad_request(msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::BAD_REQUEST, Json(serde_json::json!({
        "error": msg
    })))
}

fn validate_peer_id(id: &str) -> Result<(), &'static str> {
    if id.is_empty() || id.len() > 64 {
        return Err("Peer ID must be 1-64 characters");
    }
    Ok(())
}

/// Convert a SystemTime to a Unix timestamp integer.
fn ts(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ─── Contacts ───────────────────────────────────────────────

/// GET /api/mesh/contacts — list all contacts with online status.
pub async fn contacts_list(State(state): State<AppState>) -> R {
    let guard = state.mesh_runtime.read().await;
    let Some(ref rt) = *guard else { return Err(mesh_disabled()); };
    let book = rt.contacts.read().await;
    let contacts: Vec<serde_json::Value> = book.contacts().iter().map(|c| {
        serde_json::json!({
            "peer_id": c.peer_id,
            "display_name": c.display_name,
            "status": format!("{:?}", c.status),
            "added_at": ts(c.added_at),
            "last_seen": c.last_seen.map(ts),
            "blocked": c.blocked,
        })
    }).collect();
    let total = contacts.len();
    Ok(Json(serde_json::json!({ "contacts": contacts, "total": total })))
}

/// POST /api/mesh/contacts/add — send a contact request.
pub async fn contact_add(
    State(state): State<AppState>, Json(body): Json<serde_json::Value>,
) -> R {
    let guard = state.mesh_runtime.read().await;
    let Some(ref rt) = *guard else { return Err(mesh_disabled()); };
    let peer_id = body["peer_id"].as_str().ok_or_else(|| bad_request("Missing peer_id"))?;
    let name = body["display_name"].as_str().unwrap_or("Unknown");
    validate_peer_id(peer_id).map_err(|e| bad_request(e))?;
    let our_id = rt.peer_id();
    let mut book = rt.contacts.write().await;
    book.send_request(&our_id, peer_id, name).map_err(|e| bad_request(e))?;
    Ok(Json(serde_json::json!({ "ok": true, "message": "Contact request sent" })))
}

/// POST /api/mesh/contacts/accept/{peer_id} — accept a pending request.
pub async fn contact_accept(
    State(state): State<AppState>, Path(peer_id): Path<String>,
) -> R {
    let guard = state.mesh_runtime.read().await;
    let Some(ref rt) = *guard else { return Err(mesh_disabled()); };
    let mut book = rt.contacts.write().await;
    book.accept_request(&peer_id).map_err(|e| bad_request(e))?;
    Ok(Json(serde_json::json!({ "ok": true, "message": "Contact accepted" })))
}

/// POST /api/mesh/contacts/block/{peer_id} — block a peer.
pub async fn contact_block(
    State(state): State<AppState>, Path(peer_id): Path<String>,
) -> R {
    let guard = state.mesh_runtime.read().await;
    let Some(ref rt) = *guard else { return Err(mesh_disabled()); };
    let mut book = rt.contacts.write().await;
    book.block(&peer_id);
    Ok(Json(serde_json::json!({ "ok": true, "message": "Contact blocked" })))
}

/// DELETE /api/mesh/contacts/{peer_id} — remove a contact.
pub async fn contact_remove(
    State(state): State<AppState>, Path(peer_id): Path<String>,
) -> R {
    let guard = state.mesh_runtime.read().await;
    let Some(ref rt) = *guard else { return Err(mesh_disabled()); };
    let mut book = rt.contacts.write().await;
    let removed = book.remove(&peer_id);
    Ok(Json(serde_json::json!({ "ok": true, "removed": removed })))
}

/// GET /api/mesh/contacts/requests — list pending inbound/outbound requests.
pub async fn contact_requests(State(state): State<AppState>) -> R {
    let guard = state.mesh_runtime.read().await;
    let Some(ref rt) = *guard else { return Err(mesh_disabled()); };
    let book = rt.contacts.read().await;
    let inbound: Vec<serde_json::Value> = book.inbound_requests().iter().map(|r| {
        serde_json::json!({
            "from": r.from, "display_name": r.display_name, "timestamp": ts(r.timestamp),
        })
    }).collect();
    let outbound: Vec<serde_json::Value> = book.outbound_requests().iter().map(|r| {
        serde_json::json!({
            "to": r.to, "display_name": r.display_name, "timestamp": ts(r.timestamp),
        })
    }).collect();
    Ok(Json(serde_json::json!({ "inbound": inbound, "outbound": outbound })))
}

// ─── Groups ─────────────────────────────────────────────────

/// GET /api/mesh/groups — list all groups.
pub async fn groups_list(State(state): State<AppState>) -> R {
    let guard = state.mesh_runtime.read().await;
    let Some(ref rt) = *guard else { return Err(mesh_disabled()); };
    let store = rt.groups.read().await;
    let groups: Vec<serde_json::Value> = store.groups().iter().map(|g| {
        serde_json::json!({
            "id": g.id, "name": g.name, "emoji": g.emoji,
            "member_count": g.member_count(),
            "members": g.members, "admins": g.admins,
            "created_by": g.created_by, "created_at": ts(g.created_at),
        })
    }).collect();
    let total = groups.len();
    let invites = store.pending_invites().len();
    Ok(Json(serde_json::json!({
        "groups": groups, "total": total, "pending_invites": invites,
    })))
}

/// POST /api/mesh/groups/create — create a new group.
pub async fn group_create(
    State(state): State<AppState>, Json(body): Json<serde_json::Value>,
) -> R {
    let guard = state.mesh_runtime.read().await;
    let Some(ref rt) = *guard else { return Err(mesh_disabled()); };
    let name = body["name"].as_str().ok_or_else(|| bad_request("Missing name"))?;
    let emoji = body["emoji"].as_str().unwrap_or("👥");
    let members: Vec<String> = body["members"].as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let our_id = rt.peer_id();
    let mut store = rt.groups.write().await;
    let group = store.create(name, emoji, &our_id, &members)
        .map_err(|e| bad_request(e))?;
    let gid = group.id.clone();
    let gname = group.name.clone();
    Ok(Json(serde_json::json!({ "ok": true, "group_id": gid, "name": gname })))
}

/// POST /api/mesh/groups/{id}/invite — invite a member.
pub async fn group_invite(
    State(state): State<AppState>, Path(group_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> R {
    let guard = state.mesh_runtime.read().await;
    let Some(ref rt) = *guard else { return Err(mesh_disabled()); };
    let peer_id = body["peer_id"].as_str().ok_or_else(|| bad_request("Missing peer_id"))?;
    let our_id = rt.peer_id();
    let mut store = rt.groups.write().await;
    store.invite_member(&group_id, &our_id, peer_id)
        .map_err(|e| bad_request(e))?;
    Ok(Json(serde_json::json!({ "ok": true, "message": "Invite sent" })))
}

/// POST /api/mesh/groups/{id}/leave — leave a group.
pub async fn group_leave(
    State(state): State<AppState>, Path(group_id): Path<String>,
) -> R {
    let guard = state.mesh_runtime.read().await;
    let Some(ref rt) = *guard else { return Err(mesh_disabled()); };
    let our_id = rt.peer_id();
    let mut store = rt.groups.write().await;
    store.leave(&group_id, &our_id).map_err(|e| bad_request(e))?;
    Ok(Json(serde_json::json!({ "ok": true, "message": "Left group" })))
}

/// POST /api/mesh/groups/{id}/kick/{peer_id} — admin kicks a member.
pub async fn group_kick(
    State(state): State<AppState>, Path((group_id, peer_id)): Path<(String, String)>,
) -> R {
    let guard = state.mesh_runtime.read().await;
    let Some(ref rt) = *guard else { return Err(mesh_disabled()); };
    let our_id = rt.peer_id();
    let mut store = rt.groups.write().await;
    store.kick_member(&group_id, &peer_id, &our_id)
        .map_err(|e| bad_request(e))?;
    Ok(Json(serde_json::json!({ "ok": true, "message": "Member kicked" })))
}

/// GET /api/mesh/groups/{id}/messages — get group message history.
pub async fn group_messages(
    State(state): State<AppState>, Path(group_id): Path<String>,
) -> R {
    let guard = state.mesh_runtime.read().await;
    let Some(ref rt) = *guard else { return Err(mesh_disabled()); };
    let store = rt.groups.read().await;
    let msgs: Vec<serde_json::Value> = store.messages(&group_id).iter().map(|m| {
        serde_json::json!({
            "id": m.id, "author": m.author, "display_name": m.display_name,
            "content": m.content, "timestamp": ts(m.timestamp),
        })
    }).collect();
    let count = msgs.len();
    Ok(Json(serde_json::json!({ "messages": msgs, "count": count })))
}

/// POST /api/mesh/groups/{id}/send — send a message to a group.
pub async fn group_send(
    State(state): State<AppState>, Path(group_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> R {
    let guard = state.mesh_runtime.read().await;
    let Some(ref rt) = *guard else { return Err(mesh_disabled()); };
    let content = body["content"].as_str()
        .ok_or_else(|| bad_request("Missing content"))?;
    if content.is_empty() || content.len() > 4096 {
        return Err(bad_request("Content must be 1-4096 characters"));
    }
    let display_name = body["display_name"].as_str().unwrap_or("Anonymous");
    let our_id = rt.peer_id();
    let msg = ern_mesh::groups::GroupMessage {
        id: format!("{:x}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()),
        group_id: group_id.clone(),
        author: our_id, display_name: display_name.to_string(),
        content: content.to_string(), timestamp: std::time::SystemTime::now(),
    };
    let mut store = rt.groups.write().await;
    store.add_message(msg).map_err(|e| bad_request(e))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ─── Offline Queue ──────────────────────────────────────────

/// GET /api/mesh/offline_queue — queue statistics.
pub async fn offline_queue_stats(State(state): State<AppState>) -> R {
    let guard = state.mesh_runtime.read().await;
    let Some(ref rt) = *guard else { return Err(mesh_disabled()); };
    let q = rt.offline_queue.read().await;
    let stats_list: Vec<serde_json::Value> = q.stats().iter().map(|(peer, s)| {
        serde_json::json!({
            "peer_id": peer, "messages": s.message_count, "bytes": s.total_bytes,
        })
    }).collect();
    Ok(Json(serde_json::json!({
        "total_queued": q.total_count(),
        "peer_count": q.peer_count(),
        "stats": stats_list,
    })))
}
