//! Interpretability, learning, and steering HTTP handlers.
//! Split from `system.rs` per §10.1 (max 500 lines per file).

use crate::web::state::AppState;
use axum::{extract::State, response::IntoResponse, Json};

pub async fn interp_features() -> impl IntoResponse {
    let features = crate::interpretability::features::labeled_features();
    let entries: Vec<serde_json::Value> = features.iter().map(|f| {
        serde_json::json!({
            "index": f.index, "label": f.label,
            "category": f.category, "baseline_activation": f.baseline_activation,
        })
    }).collect();
    Json(serde_json::json!({ "count": entries.len(), "features": entries }))
}

pub async fn interp_snapshots(State(state): State<AppState>) -> impl IntoResponse {
    let dir = state.config.general.data_dir.join("snapshots");
    let mut snapshots = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut paths: Vec<_> = entries.flatten()
            .filter(|e| e.path().extension().map_or(false, |ext| ext == "json"))
            .collect();
        paths.sort_by_key(|e| std::cmp::Reverse(e.file_name()));
        for entry in paths.iter().take(50) {
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                if let Ok(snap) = serde_json::from_str::<serde_json::Value>(&content) {
                    snapshots.push(serde_json::json!({ "file": entry.file_name().to_string_lossy(), "data": snap }));
                }
            }
        }
    }
    Json(serde_json::json!({ "count": snapshots.len(), "snapshots": snapshots }))
}

pub async fn interp_live(State(state): State<AppState>) -> impl IntoResponse {
    let monitor = state.live_monitor.read().await;
    let averages = monitor.averages();
    let features = crate::interpretability::features::labeled_features();

    let entries: Vec<serde_json::Value> = averages.iter().map(|(idx, avg)| {
        let label = features.iter().find(|f| f.index == *idx)
            .map(|f| f.label.clone())
            .unwrap_or_else(|| format!("feature_{}", idx));
        let category = features.iter().find(|f| f.index == *idx)
            .map(|f| f.category.clone())
            .unwrap_or_else(|| "unknown".to_string());
        serde_json::json!({
            "index": idx,
            "label": label,
            "category": category,
            "average_activation": avg,
        })
    }).collect();

    Json(serde_json::json!({
        "window_size": monitor.window_len(),
        "feature_count": entries.len(),
        "features": entries,
    }))
}

pub async fn interp_sae(State(state): State<AppState>) -> impl IntoResponse {
    let sae = state.sae.read().await;
    let (input_dim, hidden_dim, model_loaded) = match sae.as_ref() {
        Some(s) => (s.model_dim, s.num_features, true),
        None => {
            let c = crate::interpretability::trainer::TrainConfig::default();
            (c.model_dim, c.num_features, false)
        }
    };
    let config = crate::interpretability::trainer::TrainConfig::default();
    Json(serde_json::json!({
        "input_dim": input_dim,
        "hidden_dim": hidden_dim,
        "sparsity_coefficient": config.l1_coefficient,
        "architecture": "JumpReLU",
        "model_loaded": model_loaded,
        "feature_count": crate::interpretability::features::labeled_features().len(),
    }))
}

pub async fn steering_vectors(State(state): State<AppState>) -> impl IntoResponse {
    let dir = state.config.general.data_dir.join("steering");
    match crate::steering::vectors::VectorStore::new(&dir) {
        Ok(s) => {
            let vectors: Vec<serde_json::Value> = s.list().iter().map(|v| {
                serde_json::json!({
                    "name": v.name, "path": v.path, "strength": v.strength,
                    "active": v.active, "description": v.description,
                })
            }).collect();
            let active_count = s.active_vectors().len();
            Json(serde_json::json!({ "count": vectors.len(), "active_count": active_count, "vectors": vectors }))
        }
        Err(_) => Json(serde_json::json!({ "count": 0, "active_count": 0, "vectors": [] })),
    }
}

pub async fn learning_status(State(state): State<AppState>) -> impl IntoResponse {
    let golden_count = state.golden_buffer.read().await.count();
    let rejection_count = state.rejection_buffer.read().await.count();
    let dd = &state.config.general.data_dir;
    let adapter_dir = dd.join("adapters");
    let adapter_count = crate::learning::lora::adapters::AdapterStore::new(&adapter_dir)
        .map(|s| s.count()).unwrap_or(0);
    let sleep_count: usize = std::fs::read_to_string(dd.join("sleep_history.json"))
        .ok().and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(&s).ok())
        .map(|v| v.len()).unwrap_or(0);
    Json(serde_json::json!({
        "golden_buffer_size": golden_count, "rejection_buffer_size": rejection_count,
        "adapter_count": adapter_count, "sleep_cycles": sleep_count,
        "supported_methods": ["SFT", "ORPO", "SimPO", "KTO", "DPO", "GRPO"],
    }))
}

pub async fn learning_adapters(State(state): State<AppState>) -> impl IntoResponse {
    let dir = state.config.general.data_dir.join("adapters");
    match crate::learning::lora::adapters::AdapterStore::new(&dir) {
        Ok(store) => {
            let adapters: Vec<serde_json::Value> = store.list().iter().map(|a| {
                serde_json::json!({
                    "id": a.id, "name": a.name, "method": a.method,
                    "path": a.path, "created_at": a.created_at.to_rfc3339(),
                    "param_count": a.param_count,
                })
            }).collect();
            Json(serde_json::json!({ "count": adapters.len(), "adapters": adapters }))
        }
        Err(_) => Json(serde_json::json!({ "count": 0, "adapters": [] })),
    }
}

pub async fn learning_sleep_history(State(state): State<AppState>) -> impl IntoResponse {
    let entries: Vec<serde_json::Value> = std::fs::read_to_string(state.config.general.data_dir.join("sleep_history.json"))
        .ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    Json(serde_json::json!({ "count": entries.len(), "entries": entries }))
}

pub async fn observer_history(State(state): State<AppState>) -> impl IntoResponse {
    let entries: Vec<serde_json::Value> = std::fs::read_to_string(state.config.general.data_dir.join("observer_history.json"))
        .ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    Json(serde_json::json!({ "count": entries.len(), "entries": entries }))
}
