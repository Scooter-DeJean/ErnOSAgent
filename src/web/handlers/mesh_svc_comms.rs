// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Mesh communication service handlers — chat, mail, voice, forum, feed.
//!
//! Each handler reads/writes from `MeshRuntime` service modules via
//! `state.mesh_runtime`. Returns 503 if mesh is not enabled.
//! All POST bodies are validated and size-limited (64KB via router layer).

use crate::web::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use ern_mesh::swarm_handle::SwarmCommand;

type R = Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)>;

/// Publish bytes to a Gossipsub topic via the Swarm command channel.
async fn publish(runtime: &ern_mesh::runtime::MeshRuntime, topic: &str, data: Vec<u8>) {
    let swarm_tx_arc = runtime.swarm_tx();
    let tx_guard = swarm_tx_arc.read().await;
    if let Some(ref tx) = *tx_guard {
        let _ = tx.send(SwarmCommand::Publish {
            topic: topic.to_string(), data,
        }).await;
    }
}

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

fn validate_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() || name.len() > 64 {
        return Err("Name must be 1-64 characters");
    }
    if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err("Name must be alphanumeric, hyphens, or underscores");
    }
    Ok(())
}

fn validate_content(content: &str) -> Result<(), &'static str> {
    if content.is_empty() { return Err("Content cannot be empty"); }
    if content.len() > 4096 { return Err("Content exceeds 4096 character limit"); }
    Ok(())
}

fn ts_to_string(ts: std::time::SystemTime) -> String {
    ts.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}

// ─── Chat ───────────────────────────────────────────────────

/// GET /api/mesh/chat/topics
pub async fn chat_topics(State(state): State<AppState>) -> R {
    tracing::debug!(service = "chat", "mesh: listing topics");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let log = runtime.chat_log.read().await;
    let topics: Vec<serde_json::Value> = log.active_topics().iter().map(|t| {
        let msgs = log.messages(t);
        serde_json::json!({
            "id": t.as_str(), "name": t.to_string(),
            "message_count": msgs.len(), "is_dm": t.is_dm(),
        })
    }).collect();

    Ok(Json(serde_json::json!({ "topics": topics })))
}

/// GET /api/mesh/chat/messages/{topic}
pub async fn chat_messages(
    State(state): State<AppState>, Path(topic): Path<String>,
) -> R {
    tracing::debug!(service = "chat", %topic, "mesh: loading messages");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let log = runtime.chat_log.read().await;
    let tid = ern_mesh::chat::TopicId::new(&topic);
    let msgs: Vec<serde_json::Value> = log.messages(&tid).iter().map(|m| {
        serde_json::json!({
            "sender": m.sender, "content": m.content,
            "display_name": m.display_name,
            "encrypted": m.encrypted, "timestamp": ts_to_string(m.timestamp),
        })
    }).collect();

    Ok(Json(serde_json::json!({ "messages": msgs })))
}

/// POST /api/mesh/chat/send
pub async fn chat_send(
    State(state): State<AppState>,
    Json(body): Json<ern_mesh::api::SendChatRequest>,
) -> R {
    tracing::debug!(service = "chat", topic = %body.topic, "mesh: sending message");
    if let Err(e) = validate_name(&body.topic) { return Err(bad_request(e)); }
    if let Err(e) = validate_content(&body.content) { return Err(bad_request(e)); }

    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let tid = ern_mesh::chat::TopicId::new(&body.topic);
    let peer_id = runtime.peer_id();
    let msg = ern_mesh::chat::ChatMessage::new_public(
        tid, &peer_id, &body.content, body.display_name.clone(),
    );

    // Publish to Gossipsub before local append
    if let Ok(bytes) = serde_json::to_vec(&msg) {
        publish(runtime, msg.topic.as_str(), bytes).await;
    }
    runtime.chat_log.write().await.append(msg);

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ─── Mail ───────────────────────────────────────────────────

/// GET /api/mesh/mail/inbox
pub async fn mail_inbox(State(state): State<AppState>) -> R {
    tracing::debug!(service = "mail", "mesh: listing inbox");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let mb = runtime.mailbox.read().await;
    let messages: Vec<serde_json::Value> = mb.inbox().iter().map(|m| {
        serde_json::json!({
            "message_id": m.message_id, "from": m.from, "subject": m.subject,
            "body": m.body, "read": m.read, "timestamp": ts_to_string(m.sent_at),
        })
    }).collect();

    Ok(Json(serde_json::json!({
        "messages": messages, "unread": mb.unread_count(),
    })))
}

/// GET /api/mesh/mail/sent
pub async fn mail_sent(State(state): State<AppState>) -> R {
    tracing::debug!(service = "mail", "mesh: listing sent");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let mb = runtime.mailbox.read().await;
    let messages: Vec<serde_json::Value> = mb.sent().iter().map(|m| {
        serde_json::json!({
            "message_id": m.message_id, "from": m.from, "subject": m.subject,
            "body": m.body, "timestamp": ts_to_string(m.sent_at),
        })
    }).collect();

    Ok(Json(serde_json::json!({ "messages": messages })))
}

/// GET /api/mesh/mail/message/{id} — reads and marks as read.
pub async fn mail_read(
    State(state): State<AppState>, Path(id): Path<String>,
) -> R {
    tracing::debug!(service = "mail", %id, "mesh: reading message");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let mut mb = runtime.mailbox.write().await;
    if let Some(m) = mb.get_message_mut(&id) {
        m.mark_read();
        let resp = serde_json::json!({
            "message_id": m.message_id, "from": m.from, "subject": m.subject,
            "body": m.body, "read": m.read, "timestamp": ts_to_string(m.sent_at),
        });
        Ok(Json(resp))
    } else {
        Err((StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": "Message not found"
        }))))
    }
}

/// POST /api/mesh/mail/send
pub async fn mail_send(
    State(state): State<AppState>,
    Json(body): Json<ern_mesh::api::SendMailRequest>,
) -> R {
    tracing::debug!(service = "mail", to = %body.to, "mesh: sending mail");
    if let Err(e) = validate_content(&body.body) { return Err(bad_request(e)); }

    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let peer_id = runtime.peer_id();
    let msg = ern_mesh::mesh_mail::MailMessage::new(
        &peer_id, &body.to, &body.subject, &body.body,
    );

    // Publish to recipient's mail topic
    if let Ok(bytes) = serde_json::to_vec(&msg) {
        let topic = format!("ernmesh/mail/{}", body.to);
        publish(runtime, &topic, bytes).await;
    }
    runtime.mailbox.write().await.record_sent(msg);

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /api/mesh/mail/message/{id}
pub async fn mail_delete(
    State(state): State<AppState>, Path(id): Path<String>,
) -> R {
    tracing::debug!(service = "mail", %id, "mesh: deleting message");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let deleted = runtime.mailbox.write().await.delete_message(&id);
    if deleted {
        Ok(Json(serde_json::json!({ "ok": true })))
    } else {
        Err((StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": "Message not found"
        }))))
    }
}

// ─── Voice ──────────────────────────────────────────────────

/// GET /api/mesh/voice/rooms
pub async fn voice_rooms(State(state): State<AppState>) -> R {
    tracing::debug!(service = "voice", "mesh: listing rooms");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let vm = runtime.voice.read().await;
    let rooms: Vec<serde_json::Value> = vm.list_rooms().iter().map(|rid| {
        if let Some(room) = vm.get_room(rid) {
            serde_json::json!({
                "room_id": rid, "name": room.name,
                "creator": room.creator,
                "participants": room.participant_count(),
                "max_participants": room.max_participants,
                "needs_sfu": room.needs_sfu(),
            })
        } else {
            serde_json::json!({ "room_id": rid })
        }
    }).collect();

    Ok(Json(serde_json::json!({ "rooms": rooms })))
}

#[derive(serde::Deserialize)]
pub struct CreateRoomRequest {
    pub name: String,
    pub max_participants: Option<u32>,
}

/// POST /api/mesh/voice/create
pub async fn voice_create(
    State(state): State<AppState>,
    Json(body): Json<CreateRoomRequest>,
) -> R {
    tracing::debug!(service = "voice", name = %body.name, "mesh: creating room");
    if let Err(e) = validate_name(&body.name) { return Err(bad_request(e)); }

    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let peer_id = runtime.peer_id();
    let max = body.max_participants.unwrap_or(10);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let room_id = format!("room-{}", ts);
    let mut vm = runtime.voice.write().await;
    vm.create_room(&room_id, &peer_id, &body.name, max, 4);
    Ok(Json(serde_json::json!({
        "ok": true, "room_id": room_id,
    })))
}

/// POST /api/mesh/voice/join/{id}
pub async fn voice_join(
    State(state): State<AppState>, Path(id): Path<String>,
) -> R {
    tracing::debug!(service = "voice", %id, "mesh: joining room");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let peer_id = runtime.peer_id();
    let mut vm = runtime.voice.write().await;
    if let Some(room) = vm.get_room_mut(&id) {
        match room.join(&peer_id) {
            Ok(()) => Ok(Json(serde_json::json!({ "ok": true }))),
            Err(e) => Err(bad_request(&e.to_string())),
        }
    } else {
        Err((StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": "Room not found"
        }))))
    }
}

/// POST /api/mesh/voice/leave/{id}
pub async fn voice_leave(
    State(state): State<AppState>, Path(id): Path<String>,
) -> R {
    tracing::debug!(service = "voice", %id, "mesh: leaving room");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let peer_id = runtime.peer_id();
    let mut vm = runtime.voice.write().await;
    if let Some(room) = vm.get_room_mut(&id) {
        room.leave(&peer_id);
        Ok(Json(serde_json::json!({ "ok": true })))
    } else {
        Err((StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": "Room not found"
        }))))
    }
}

// ─── Forum ──────────────────────────────────────────────────

/// GET /api/mesh/forum/communities
pub async fn forum_communities(State(state): State<AppState>) -> R {
    tracing::debug!(service = "forum", "mesh: listing communities");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let fs = runtime.forums.read().await;
    let communities: Vec<serde_json::Value> = fs.list_communities().iter().map(|name| {
        if let Some(c) = fs.get_community(name) {
            serde_json::json!({
                "name": c.name, "description": c.description,
                "creator": c.creator, "rules": c.rules,
            })
        } else {
            serde_json::json!({ "name": name })
        }
    }).collect();

    Ok(Json(serde_json::json!({ "communities": communities })))
}

#[derive(serde::Deserialize)]
pub struct CreateCommunityRequest {
    pub name: String,
    pub description: String,
}

/// POST /api/mesh/forum/communities
pub async fn forum_create_community(
    State(state): State<AppState>,
    Json(body): Json<CreateCommunityRequest>,
) -> R {
    tracing::debug!(service = "forum", name = %body.name, "mesh: creating community");
    if let Err(e) = validate_name(&body.name) { return Err(bad_request(e)); }
    if let Err(e) = validate_content(&body.description) { return Err(bad_request(e)); }

    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let peer_id = runtime.peer_id();
    let community = ern_mesh::forum::Community::new(
        &body.name, &body.description, &peer_id,
    );
    let mut fs = runtime.forums.write().await;
    match fs.create_community(community) {
        Ok(()) => Ok(Json(serde_json::json!({ "ok": true }))),
        Err(e) => Err(bad_request(&e.to_string())),
    }
}

/// GET /api/mesh/forum/threads/{community}
pub async fn forum_threads(
    State(state): State<AppState>, Path(community): Path<String>,
) -> R {
    tracing::debug!(service = "forum", %community, "mesh: listing threads");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let fs = runtime.forums.read().await;
    let threads: Vec<serde_json::Value> = fs.threads_in(&community).iter().map(|p| {
        serde_json::json!({
            "post_id": p.post_id, "author": p.author,
            "title": p.title, "body": p.body,
            "upvotes": p.upvotes, "downvotes": p.downvotes,
            "score": p.score(), "timestamp": ts_to_string(p.created_at),
        })
    }).collect();

    Ok(Json(serde_json::json!({ "threads": threads })))
}

#[derive(serde::Deserialize)]
pub struct ForumPostRequest {
    pub community: String,
    pub title: Option<String>,
    pub body: String,
    pub parent_id: Option<String>,
}

/// POST /api/mesh/forum/post
pub async fn forum_post(
    State(state): State<AppState>,
    Json(body): Json<ForumPostRequest>,
) -> R {
    tracing::debug!(service = "forum", community = %body.community, "mesh: posting");
    if let Err(e) = validate_content(&body.body) { return Err(bad_request(e)); }

    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let peer_id = runtime.peer_id();
    let post = if let Some(ref parent) = body.parent_id {
        ern_mesh::forum::ForumPost::new_reply(
            &body.community, &peer_id, parent, &body.body,
        )
    } else {
        let title = body.title.as_deref().unwrap_or("Untitled");
        ern_mesh::forum::ForumPost::new_thread(
            &body.community, &peer_id, title, &body.body,
        )
    };

    let mut fs = runtime.forums.write().await;
    match fs.add_post(post) {
        Ok(()) => Ok(Json(serde_json::json!({ "ok": true }))),
        Err(e) => Err(bad_request(&e.to_string())),
    }
}

// ─── Feed (ErnieBook) ───────────────────────────────────────

/// GET /api/mesh/feed
pub async fn feed_list(State(state): State<AppState>) -> R {
    tracing::debug!(service = "feed", "mesh: listing feed");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let f = runtime.feed.read().await;
    let posts: Vec<serde_json::Value> = f.recent_posts(50).iter().map(|p| {
        serde_json::json!({
            "author": p.author, "content": p.content,
            "display_name": p.display_name, "tags": p.tags,
            "timestamp": ts_to_string(p.timestamp),
        })
    }).collect();

    Ok(Json(serde_json::json!({ "posts": posts })))
}

/// POST /api/mesh/feed
pub async fn feed_create(
    State(state): State<AppState>,
    Json(body): Json<ern_mesh::api::CreatePostRequest>,
) -> R {
    tracing::debug!(service = "feed", "mesh: creating post");
    if let Err(e) = validate_content(&body.content) { return Err(bad_request(e)); }

    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let peer_id = runtime.peer_id();
    let post = ern_mesh::erniebook::Post::new(
        &peer_id, &body.content, body.display_name.clone(), body.tags.clone(),
    );

    // Publish to erniebook topic
    if let Ok(bytes) = serde_json::to_vec(&post) {
        publish(runtime, ern_mesh::erniebook::ERNIEBOOK_TOPIC, bytes).await;
    }
    runtime.feed.write().await.add(post);

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
#[path = "mesh_svc_comms_tests.rs"]
mod tests;
