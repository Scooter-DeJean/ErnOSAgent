// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Mesh infrastructure service handlers — economy, reputation, sites, relay, transfers.
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

fn validate_peer_id(id: &str) -> Result<(), &'static str> {
    if id.is_empty() || id.len() > 128 {
        return Err("Invalid peer ID length");
    }
    Ok(())
}

fn ts_to_string(ts: std::time::SystemTime) -> String {
    ts.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}

// ─── Economy ────────────────────────────────────────────────

/// GET /api/mesh/economy/balance
pub async fn economy_balance(State(state): State<AppState>) -> R {
    tracing::debug!(service = "economy", "mesh: fetching balance");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let ledger = runtime.ledger.read().await;
    Ok(Json(serde_json::json!({
        "balance": ledger.balance(),
        "transaction_count": ledger.transaction_count(),
    })))
}

/// GET /api/mesh/economy/transactions
pub async fn economy_transactions(State(state): State<AppState>) -> R {
    tracing::debug!(service = "economy", "mesh: listing transactions");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let ledger = runtime.ledger.read().await;
    let txs: Vec<serde_json::Value> = ledger.recent_transactions(50).iter().map(|tx| {
        let (direction, reason) = match &tx.kind {
            ern_mesh::economy::TransactionKind::Earn(r) => ("earn", format!("{:?}", r)),
            ern_mesh::economy::TransactionKind::Spend(r) => ("spend", format!("{:?}", r)),
        };
        serde_json::json!({
            "id": tx.id, "direction": direction, "reason": reason,
            "amount": tx.amount, "balance_after": tx.balance_after,
            "timestamp": ts_to_string(tx.timestamp),
        })
    }).collect();

    Ok(Json(serde_json::json!({ "transactions": txs })))
}

// ─── Reputation ─────────────────────────────────────────────

/// GET /api/mesh/reputation
pub async fn reputation_list(State(state): State<AppState>) -> R {
    tracing::debug!(service = "reputation", "mesh: listing peers");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let rep = runtime.reputation.read().await;
    let peers: Vec<serde_json::Value> = rep.leaderboard().iter().map(|p| {
        serde_json::json!({
            "peer_id": p.peer_id, "score": p.score,
            "positive_events": p.positive_events,
            "negative_events": p.negative_events,
            "is_trusted": p.is_trusted(10.0),
            "is_hostile": p.is_hostile(-50.0),
        })
    }).collect();

    Ok(Json(serde_json::json!({ "peers": peers })))
}

#[derive(serde::Deserialize)]
pub struct TrustRequest {
    pub delta: f64,
}

/// POST /api/mesh/reputation/trust/{peer_id}
pub async fn reputation_trust(
    State(state): State<AppState>,
    Path(peer_id): Path<String>,
    Json(body): Json<TrustRequest>,
) -> R {
    tracing::debug!(service = "reputation", %peer_id, delta = body.delta, "mesh: manual trust");
    if let Err(e) = validate_peer_id(&peer_id) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({
            "error": e
        }))));
    }

    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let mut rep = runtime.reputation.write().await;
    rep.record(&peer_id, ern_mesh::reputation::ReputationEvent::ManualTrust {
        delta: body.delta,
    });
    let new_score = rep.score(&peer_id);

    Ok(Json(serde_json::json!({
        "ok": true, "new_score": new_score,
    })))
}

// ─── Sites ──────────────────────────────────────────────────

/// GET /api/mesh/sites
pub async fn sites_list(State(state): State<AppState>) -> R {
    tracing::debug!(service = "sites", "mesh: listing sites");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let sr = runtime.sites.read().await;
    let sites: Vec<serde_json::Value> = sr.list_sites().iter().map(|name| {
        if let Some(s) = sr.get_site(name) {
            serde_json::json!({
                "name": s.name, "description": s.description,
                "version": s.version, "file_count": s.file_count(),
            })
        } else {
            serde_json::json!({ "name": name })
        }
    }).collect();

    Ok(Json(serde_json::json!({ "sites": sites })))
}

/// DELETE /api/mesh/sites/{name}
pub async fn sites_remove(
    State(state): State<AppState>, Path(name): Path<String>,
) -> R {
    tracing::debug!(service = "sites", %name, "mesh: removing site");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let removed = runtime.sites.write().await.remove_site(&name);
    if removed {
        Ok(Json(serde_json::json!({ "ok": true })))
    } else {
        Err((StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": "Site not found"
        }))))
    }
}

// ─── Relay ──────────────────────────────────────────────────

/// GET /api/mesh/relay
pub async fn relay_status(State(state): State<AppState>) -> R {
    tracing::debug!(service = "relay", "mesh: relay status");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let pool = runtime.relay_pool.read().await;
    Ok(Json(serde_json::json!({
        "enabled": pool.is_enabled(),
        "active_sessions": pool.active_session_count(),
        "total_mb_relayed": pool.total_mb_relayed(),
    })))
}

/// POST /api/mesh/relay/terminate/{peer_id}
pub async fn relay_terminate(
    State(state): State<AppState>, Path(peer_id): Path<String>,
) -> R {
    tracing::debug!(service = "relay", %peer_id, "mesh: terminating session");
    if let Err(e) = validate_peer_id(&peer_id) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({
            "error": e
        }))));
    }

    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let terminated = runtime.relay_pool.write().await.terminate_session(&peer_id);
    Ok(Json(serde_json::json!({
        "ok": terminated,
    })))
}

// ─── Transfers ──────────────────────────────────────────────

/// GET /api/mesh/transfers
pub async fn transfers_list(State(state): State<AppState>) -> R {
    tracing::debug!(service = "transfers", "mesh: listing transfers");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let tm = runtime.transfers.read().await;
    Ok(Json(serde_json::json!({
        "active_count": tm.active_count(),
        "total_count": tm.total_count(),
    })))
}

/// POST /api/mesh/transfers/accept/{id}
pub async fn transfer_accept(
    State(state): State<AppState>, Path(id): Path<String>,
) -> R {
    tracing::debug!(service = "transfers", %id, "mesh: accepting transfer");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let mut tm = runtime.transfers.write().await;
    if let Some(t) = tm.get_transfer_mut(&id) {
        t.accept();
        Ok(Json(serde_json::json!({ "ok": true })))
    } else {
        Err((StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": "Transfer not found"
        }))))
    }
}

/// POST /api/mesh/transfers/reject/{id}
pub async fn transfer_reject(
    State(state): State<AppState>, Path(id): Path<String>,
) -> R {
    tracing::debug!(service = "transfers", %id, "mesh: rejecting transfer");
    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let mut tm = runtime.transfers.write().await;
    if let Some(t) = tm.get_transfer_mut(&id) {
        t.reject();
        Ok(Json(serde_json::json!({ "ok": true })))
    } else {
        Err((StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": "Transfer not found"
        }))))
    }
}

// ─── Site Browse (ern:// proxy) ────────────────────────────

/// GET /api/mesh/sites/browse/{name}/{*path} — serve mesh site content.
///
/// Resolves the file from the SiteManifest, loads its content from
/// ContentStore, and returns the raw bytes with the correct Content-Type.
pub async fn site_browse(
    State(state): State<AppState>,
    Path((name, file_path)): Path<(String, String)>,
) -> Result<axum::response::Response, (StatusCode, Json<serde_json::Value>)> {
    use axum::response::IntoResponse;
    tracing::debug!(service = "sites", %name, %file_path, "mesh: browsing site");

    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else {
        return Err(mesh_disabled());
    };

    let sites = runtime.sites.read().await;
    let Some(manifest) = sites.get_site(&name) else {
        return Err((StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": format!("Site '{}' not found", name)
        }))));
    };

    // Resolve file path (default to index)
    let resolved_path = if file_path.is_empty() {
        &manifest.index_file
    } else {
        &file_path
    };

    let Some(cid_hex) = manifest.get_file_cid(resolved_path) else {
        return Err((StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": format!("File '{}' not found in site '{}'", resolved_path, name)
        }))));
    };

    let cid = ern_mesh::content_store::ContentId::from_hex(cid_hex);
    let cs = runtime.content_store.read().await;

    let data = cs.get(&cid).map_err(|_| {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": format!("Content {} not found in store", cid_hex)
        })))
    })?;

    // Infer content type from file extension
    let content_type = match resolved_path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    };

    Ok(([(axum::http::header::CONTENT_TYPE, content_type)], data).into_response())
}

// ─── Site Publish ───────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct SitePublishRequest {
    pub name: String,
    pub description: String,
    /// Files as { "filename": "base64-encoded content" }
    pub files: std::collections::HashMap<String, String>,
}

/// POST /api/mesh/sites/publish — publish a new mesh site.
///
/// Accepts base64-encoded files, stores them in ContentStore, builds
/// a SiteManifest, and registers it in SiteRegistry.
pub async fn site_publish(
    State(state): State<AppState>,
    Json(body): Json<SitePublishRequest>,
) -> R {
    use base64::Engine;
    tracing::debug!(service = "sites", name = %body.name, "mesh: publishing site");

    if body.name.is_empty() || body.name.len() > 64 {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({
            "error": "Site name must be 1-64 characters"
        }))));
    }
    if body.files.is_empty() {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({
            "error": "Site must have at least one file"
        }))));
    }

    let guard = state.mesh_runtime.read().await;
    let Some(ref runtime) = *guard else { return Err(mesh_disabled()); };

    let peer_id = runtime.peer_id();
    let mut manifest = ern_mesh::mesh_site::SiteManifest::new(
        &body.name, &peer_id, &body.description,
    );

    // Store each file in ContentStore and record CID in manifest
    let mut cs = runtime.content_store.write().await;
    for (filename, b64_content) in &body.files {
        let data = base64::engine::general_purpose::STANDARD
            .decode(b64_content)
            .map_err(|e| (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "error": format!("Invalid base64 for '{}': {}", filename, e)
            }))))?;

        let mime = match filename.rsplit('.').next() {
            Some("html") => Some("text/html".into()),
            Some("css") => Some("text/css".into()),
            Some("js") => Some("application/javascript".into()),
            _ => None,
        };

        let cid = cs.store(&data, mime, Some(filename.clone()))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                "error": format!("Failed to store '{}': {}", filename, e)
            }))))?;

        manifest.add_file(filename, cid.as_hex());
    }

    // Register the manifest
    let mut sr = runtime.sites.write().await;
    sr.publish(manifest).map_err(|e| {
        (StatusCode::BAD_REQUEST, Json(serde_json::json!({
            "error": format!("Publish failed: {}", e)
        })))
    })?;

    Ok(Json(serde_json::json!({
        "ok": true,
        "name": body.name,
        "file_count": body.files.len(),
    })))
}

#[cfg(test)]
#[path = "mesh_svc_infra_tests.rs"]
mod tests;
