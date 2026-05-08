//! System handlers — health, status, models, factory reset, tools, training, interpretability, steering, learning, observer, logs.

use crate::web::state::AppState;
use axum::{extract::State, response::IntoResponse, Json};

pub async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// Reads model download progress from file written by Android ModelManager.
pub async fn model_download_progress(State(state): State<AppState>) -> impl IntoResponse {
    let progress_path = state.config.general.data_dir.join("model_download_progress.json");
    match tokio::fs::read_to_string(&progress_path).await {
        Ok(content) => {
            ([("content-type", "application/json")], content).into_response()
        }
        Err(_) => {
            // No progress file = not downloading
            Json(serde_json::json!({"downloading": false})).into_response()
        }
    }
}

pub async fn system_status(State(state): State<AppState>) -> impl IntoResponse {
    let memory = state.memory.read().await;
    let sessions = state.sessions.read().await;
    let provider_healthy = state.provider.health().await;

    Json(serde_json::json!({
        "model": {
            "name": state.model_spec.name,
            "context_length": state.model_spec.context_length,
            "supports_vision": state.model_spec.supports_vision,
            "supports_video": state.model_spec.supports_video,
            "supports_audio": state.model_spec.supports_audio,
            "supports_tool_calling": state.model_spec.supports_tool_calling,
            "supports_thinking": state.model_spec.supports_thinking,
        },
        "memory": memory.status_summary(),
        "provider": state.config.general.active_provider,
        "provider_healthy": provider_healthy,
        "observer": { "enabled": state.config.observer.enabled },
        "sessions": sessions.list().len(),
    }))
}

pub async fn list_models() -> impl IntoResponse {
    let models_dir = std::path::Path::new("./models");
    let mut models = Vec::new();

    if let Ok(entries) = std::fs::read_dir(models_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext == "gguf" {
                    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    let size_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    let is_mmproj = name.starts_with("mmproj");
                    models.push(serde_json::json!({
                        "name": name,
                        "path": path.to_string_lossy(),
                        "size_gb": format!("{:.1}", size_bytes as f64 / 1_073_741_824.0),
                        "is_mmproj": is_mmproj,
                    }));
                }
            }
        }
    }

    Json(serde_json::json!({ "models": models }))
}

pub async fn factory_reset(
    headers: axum::http::HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // §13.5: Destructive operations require explicit confirmation header
    if headers.get("x-confirm-destructive").and_then(|v| v.to_str().ok()) != Some("true") {
        return Json(serde_json::json!({
            "error": "Factory reset requires X-Confirm-Destructive: true header",
            "hint": "This is a destructive operation that wipes all memory and sessions."
        }));
    }
    tracing::warn!("FACTORY RESET initiated via WebUI");

    { let mut memory = state.memory.write().await; memory.clear(); }

    {
        let mut sessions = state.sessions.write().await;
        let ids: Vec<String> = sessions.list().iter().map(|s| s.id.clone()).collect();
        for id in ids { let _ = sessions.delete(&id); }
    }

    let dd = &state.config.general.data_dir;
    let _ = std::fs::write(dd.join("golden_buffer.json"), "[]");
    let _ = std::fs::write(dd.join("rejection_buffer.json"), "[]");
    let _ = std::fs::remove_dir_all(dd.join("timeline"));
    let _ = std::fs::create_dir_all(dd.join("timeline"));
    // Reset onboarding so the welcome flow triggers again
    let _ = std::fs::remove_file(dd.join("user_profile.json"));
    // Clear observer and sleep history
    let _ = std::fs::remove_file(dd.join("observer_history.json"));
    let _ = std::fs::remove_file(dd.join("sleep_history.json"));

    // Restore default prompts from prompts/ (factory defaults) → data/prompts/ (runtime)
    // The user's identity gets re-customized via the onboarding flow after reset.
    let defaults_dir = std::path::Path::new("prompts");
    let runtime_dir = dd.join("prompts");
    let _ = std::fs::create_dir_all(&runtime_dir);
    for name in &["core.md", "identity.md", "observer.md"] {
        let src = defaults_dir.join(name);
        let dst = runtime_dir.join(name);
        match std::fs::copy(&src, &dst) {
            Ok(_) => tracing::info!(file = %name, "Restored default prompt"),
            Err(e) => tracing::warn!(file = %name, error = %e, "Failed to restore default prompt"),
        }
    }

    tracing::info!("Factory reset complete — all data wiped (including onboarding)");
    Json(serde_json::json!({ "ok": true, "message": "Factory reset complete. All data cleared." }))
}

pub async fn tools_catalog() -> impl IntoResponse {
    let l1 = crate::tools::schema::layer1_tools();
    let l2 = crate::tools::schema::layer2_tools();
    Json(serde_json::json!({ "layer1": l1, "layer2": l2 }))
}

pub async fn training_buffers(State(state): State<AppState>) -> impl IntoResponse {
    let dd = &state.config.general.data_dir;
    let golden: Vec<serde_json::Value> = std::fs::read_to_string(dd.join("golden_buffer.json"))
        .ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let rejections: Vec<serde_json::Value> = std::fs::read_to_string(dd.join("rejection_buffer.json"))
        .ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    Json(serde_json::json!({
        "golden": { "count": golden.len(), "entries": golden },
        "rejections": { "count": rejections.len(), "entries": rejections },
    }))
}

// Interpretability, learning, and steering handlers moved to system_interp.rs (§10.1).

pub async fn logs_recent() -> impl IntoResponse {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let log_path = format!("data/logs/ern-os.log.{}", today);
    let mut entries = Vec::new();
    if let Ok(content) = std::fs::read_to_string(&log_path) {
        let lines: Vec<&str> = content.lines().collect();
        let start = if lines.len() > 200 { lines.len() - 200 } else { 0 };
        for line in &lines[start..] {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line) {
                entries.push(parsed);
            }
        }
    }
    Json(serde_json::json!({ "count": entries.len(), "entries": entries }))
}

/// GET /api/self-edits — Read self-edit audit trail.
pub async fn self_edits(State(state): State<AppState>) -> impl IntoResponse {
    let path = state.config.general.data_dir.join("self_edit_log.jsonl");
    let mut entries = Vec::new();
    if let Ok(content) = std::fs::read_to_string(&path) {
        for line in content.lines().rev().take(100) {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line) {
                entries.push(parsed);
            }
        }
    }
    Json(serde_json::json!({ "count": entries.len(), "entries": entries }))
}

/// GET /api/checkpoints — List file checkpoints.
pub async fn checkpoints(State(state): State<AppState>) -> impl IntoResponse {
    let dir = state.config.general.data_dir.join("checkpoints");
    let mut entries = Vec::new();
    if let Ok(read_dir) = std::fs::read_dir(&dir) {
        let mut files: Vec<_> = read_dir.flatten().collect();
        files.sort_by_key(|e| std::cmp::Reverse(
            e.metadata().ok().and_then(|m| m.modified().ok()).unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        ));
        for entry in files.iter().take(50) {
            let name = entry.file_name().to_string_lossy().to_string();
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let modified = entry.metadata().ok()
                .and_then(|m| m.modified().ok())
                .map(|t| {
                    let dt: chrono::DateTime<chrono::Local> = t.into();
                    dt.to_rfc3339()
                })
                .unwrap_or_default();
            entries.push(serde_json::json!({
                "name": name, "size_bytes": size, "modified": modified,
            }));
        }
    }
    Json(serde_json::json!({ "count": entries.len(), "checkpoints": entries }))
}

pub async fn get_prompt(
    State(state): State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> impl IntoResponse {
    if !matches!(name.as_str(), "core" | "identity" | "observer") {
        return Json(serde_json::json!({ "error": "Invalid prompt name. Use 'core', 'identity', or 'observer'." }));
    }
    let path = state.config.general.data_dir.join("prompts").join(format!("{}.md", name));
    match std::fs::read_to_string(&path) {
        Ok(content) => Json(serde_json::json!({ "name": name, "content": content })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

pub async fn save_prompt(
    State(state): State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !matches!(name.as_str(), "core" | "identity" | "observer") {
        return Json(serde_json::json!({ "error": "Invalid prompt name. Use 'core', 'identity', or 'observer'." }));
    }
    let content = match body["content"].as_str() {
        Some(c) => c,
        None => return Json(serde_json::json!({ "error": "Missing 'content' field" })),
    };
    let path = state.config.general.data_dir.join("prompts").join(format!("{}.md", name));
    match std::fs::write(&path, content) {
        Ok(_) => {
            tracing::info!(name = %name, len = content.len(), "Prompt file updated");
            Json(serde_json::json!({ "ok": true, "name": name }))
        }
        Err(e) => Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
    }
}

/// PUT /api/settings/model — Swap the active model and restart.
pub async fn swap_model(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let model_name = match body["model"].as_str() {
        Some(name) => name.to_string(),
        None => return Json(serde_json::json!({"success": false, "error": "Missing 'model' field"})),
    };

    // Update config in memory
    {
        let mut config = state.mutable_config.write().await;

        // Find the model file path
        let models_dir = std::path::Path::new("models");
        let model_path = models_dir.join(&model_name);
        if !model_path.exists() {
            return Json(serde_json::json!({
                "success": false,
                "error": format!("Model file not found: {}", model_path.display()),
            }));
        }

        config.llamacpp.model_path = model_path.to_string_lossy().to_string();

        // Auto-detect mmproj: scan models/ for a multimodal projector matching this model.
        // Convention: model "gemma-4-26B-A4B-it-Q4_K_M.gguf" has mmproj "mmproj-gemma-4-26B-A4B-it-bf16.gguf".
        // We strip the quantisation suffix (Q4_K_M, Q8_0, etc.) and look for an mmproj containing the base name.
        let model_base = model_name.trim_end_matches(".gguf");
        let base_no_quant = strip_quant_suffix(model_base);
        let mmproj = std::fs::read_dir(&models_dir)
            .ok()
            .and_then(|entries| {
                entries.flatten().find(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    name.starts_with("mmproj-") && name.contains(&base_no_quant)
                })
            })
            .map(|e| e.path().to_string_lossy().to_string());

        config.llamacpp.mmproj_path = mmproj.clone();
        tracing::info!(
            model = %model_name,
            mmproj = ?mmproj,
            path = %config.llamacpp.model_path,
            "Model swap: updating config"
        );

        // Persist to ern-os.toml
        if let Ok(serialized) = toml::to_string_pretty(&*config) {
            if let Err(e) = std::fs::write("ern-os.toml", &serialized) {
                tracing::error!(error = %e, "Failed to persist model config");
                return Json(serde_json::json!({"success": false, "error": format!("Config write failed: {}", e)}));
            }
        }
    }

    tracing::info!(model = %model_name, "Model swap: config updated — scheduling restart");

    // Schedule a graceful restart (exit, let process manager restart us)
    tokio::spawn(async {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        tracing::info!("Model swap: exiting for restart");
        std::process::exit(0);
    });

    Json(serde_json::json!({"success": true, "model": model_name, "message": "Restarting with new model..."}))
}

/// List available skill files from data/skills/.
pub async fn list_skills(State(state): State<AppState>) -> impl IntoResponse {
    let skills_dir = state.config.general.data_dir.join("skills");
    let mut skills = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&skills_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "md") {
                let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                let description = content.lines()
                    .find(|l| l.starts_with("description:"))
                    .map(|l| l.trim_start_matches("description:").trim().to_string())
                    .unwrap_or_default();
                skills.push(serde_json::json!({
                    "name": name,
                    "description": description,
                    "path": path.to_string_lossy(),
                }));
            }
        }
    }

    Json(serde_json::json!({"skills": skills, "count": skills.len()}))
}

/// POST /api/stop — Cancel running inference.
/// This endpoint works even when the WebSocket handler is blocked on inference,
/// because it's an independent HTTP request. The WebUI frontend and Discord
/// adapter call this instead of relying on the (blocked) WS message path.
pub async fn stop_inference(State(state): State<AppState>) -> impl IntoResponse {
    state.cancel_flag.store(true, std::sync::atomic::Ordering::Relaxed);
    tracing::info!("HTTP: Inference cancellation requested via /api/stop");
    Json(serde_json::json!({ "ok": true, "message": "Inference cancelled" }))
}

/// POST /api/shutdown — Graceful engine shutdown.
/// Disconnects all platforms before halting.
pub async fn shutdown_engine(
    headers: axum::http::HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // §13.5: Destructive operations require explicit confirmation header
    if headers.get("x-confirm-destructive").and_then(|v| v.to_str().ok()) != Some("true") {
        return Json(serde_json::json!({
            "error": "Shutdown requires X-Confirm-Destructive: true header"
        }));
    }
    tracing::warn!("SHUTDOWN requested via /api/shutdown — disconnecting platforms");
    {
        let mut reg = state.platforms.write().await;
        reg.disconnect_all().await;
    }
    tokio::spawn(async {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        tracing::info!("Shutdown: exiting now");
        std::process::exit(0);
    });
    Json(serde_json::json!({ "ok": true, "message": "Engine shutting down..." }))
}

/// Strip quantisation suffix from a model filename base.
/// e.g. "gemma-4-26B-A4B-it-Q4_K_M" → "gemma-4-26B-A4B-it"
///      "Qwen_Qwen3.6-27B-Q8_0" → "Qwen_Qwen3.6-27B"
fn strip_quant_suffix(base: &str) -> String {
    // Look for the last segment starting with 'Q' followed by a digit (Q4, Q8, Q6, etc.)
    // or common quantisation markers like 'F16', 'F32', 'bf16'
    if let Some(pos) = base.rfind('-') {
        let suffix = &base[pos + 1..];
        let is_quant = (suffix.starts_with('Q') && suffix.len() >= 2 && suffix.as_bytes()[1].is_ascii_digit())
            || suffix == "F16"
            || suffix == "F32"
            || suffix == "bf16";
        if is_quant {
            return base[..pos].to_string();
        }
    }
    // Also handle underscore-separated quantisation (e.g. "Qwen_Qwen3.6-27B-Q8_0")
    // The Q8_0 part includes an underscore, so the '-Q8_0' is a single segment after the last '-'
    base.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_quant_suffix_standard() {
        assert_eq!(strip_quant_suffix("gemma-4-26B-A4B-it-Q4_K_M"), "gemma-4-26B-A4B-it");
    }

    #[test]
    fn test_strip_quant_suffix_q8() {
        assert_eq!(strip_quant_suffix("gemma-4-31B-it-Q8_0"), "gemma-4-31B-it");
    }

    #[test]
    fn test_strip_quant_suffix_no_quant() {
        assert_eq!(strip_quant_suffix("some-model-name"), "some-model-name");
    }

    #[test]
    fn test_strip_quant_suffix_f16() {
        assert_eq!(strip_quant_suffix("mmproj-gemma-4-26B-A4B-it-bf16"), "mmproj-gemma-4-26B-A4B-it");
    }

    #[test]
    fn test_strip_quant_suffix_qwen() {
        // Qwen uses underscore format: "Qwen_Qwen3.6-27B-Q8_0"
        assert_eq!(strip_quant_suffix("Qwen_Qwen3.6-27B-Q8_0"), "Qwen_Qwen3.6-27B");
    }
}
