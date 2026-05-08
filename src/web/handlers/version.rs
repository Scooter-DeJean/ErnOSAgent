//! Version management handler — check updates, update, rollback, history.
//!
//! All version operations use git under the hood since Ern-OS is deployed
//! from a git clone. Update/rollback trigger the system_recompile pipeline.

use axum::response::Json;
use serde_json::json;

/// GET /api/version — current version info.
pub async fn get_version() -> Json<serde_json::Value> {
    let root = std::env::current_dir().unwrap_or_default();

    let hash = run_git(&root, &["rev-parse", "--short", "HEAD"]).await;
    let full_hash = run_git(&root, &["rev-parse", "HEAD"]).await;
    let message = run_git(&root, &["log", "-1", "--format=%s"]).await;
    let date = run_git(&root, &["log", "-1", "--format=%ci"]).await;
    let branch = run_git(&root, &["rev-parse", "--abbrev-ref", "HEAD"]).await;
    let dirty = !run_git(&root, &["status", "--porcelain"]).await.trim().is_empty();

    Json(json!({
        "hash": hash.trim(),
        "full_hash": full_hash.trim(),
        "message": message.trim(),
        "date": date.trim(),
        "branch": branch.trim(),
        "dirty": dirty,
    }))
}

/// GET /api/version/check — fetch from remote and compare.
pub async fn check_updates() -> Json<serde_json::Value> {
    let root = std::env::current_dir().unwrap_or_default();

    // Fetch latest from origin
    let _fetch_result = run_git(&root, &["fetch", "origin"]).await;
    let branch = run_git(&root, &["rev-parse", "--abbrev-ref", "HEAD"]).await;
    let branch = branch.trim();

    // Compare HEAD vs origin/branch
    let local = run_git(&root, &["rev-parse", "HEAD"]).await;
    let remote = run_git(&root, &["rev-parse", &format!("origin/{}", branch)]).await;
    let local = local.trim();
    let remote = remote.trim();

    if local == remote {
        return Json(json!({
            "up_to_date": true,
            "commits_behind": 0,
            "branch": branch,
            "local": local,
            "remote": remote,
        }));
    }

    // Count commits behind
    let behind_log = run_git(&root, &[
        "log", "--oneline", &format!("{}..origin/{}", local, branch)
    ]).await;
    let commits: Vec<&str> = behind_log.trim().lines().collect();

    Json(json!({
        "up_to_date": false,
        "commits_behind": commits.len(),
        "branch": branch,
        "local": local,
        "remote": remote,
        "new_commits": commits,
    }))
}

/// POST /api/version/update — pull latest and recompile.
pub async fn update_version() -> Json<serde_json::Value> {
    let root = std::env::current_dir().unwrap_or_default();
    let branch = run_git(&root, &["rev-parse", "--abbrev-ref", "HEAD"]).await;
    let branch = branch.trim();

    tracing::info!("Version update: starting git pull + recompile");

    // 1. Stash any local changes
    let stash = run_git(&root, &["stash", "push", "-m", "ern-os-auto-stash-before-update"]).await;
    tracing::info!(stash = %stash.trim(), "Stashed local changes");

    // 2. Pull latest
    let pull = run_git(&root, &["pull", "origin", branch]).await;
    if pull.contains("CONFLICT") || pull.contains("error:") {
        // Abort and restore
        let _ = run_git(&root, &["merge", "--abort"]).await;
        let _ = run_git(&root, &["stash", "pop"]).await;
        return Json(json!({
            "success": false,
            "error": format!("Git pull failed: {}", pull.trim()),
            "action": "Merge conflict. Local changes restored.",
        }));
    }

    tracing::info!(pull = %pull.trim(), "Git pull complete");

    // 3. Recompile
    match crate::tools::compiler::run_recompile("", "web").await {
        Ok(msg) => {
            Json(json!({
                "success": true,
                "message": msg,
                "action": "Update applied. System will restart.",
            }))
        }
        Err(e) => {
            // Recompile failed — revert
            let _ = run_git(&root, &["reset", "--hard", "HEAD~1"]).await;
            let _ = run_git(&root, &["stash", "pop"]).await;
            Json(json!({
                "success": false,
                "error": format!("Recompile failed: {}", e),
                "action": "Update reverted. Previous version restored.",
            }))
        }
    }
}

/// POST /api/version/rollback — checkout a specific commit and recompile.
pub async fn rollback_version(
    axum::extract::Json(body): axum::extract::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let hash = match body["hash"].as_str() {
        Some(h) if !h.is_empty() => h.to_string(),
        _ => return Json(json!({"success": false, "error": "Missing 'hash' parameter"})),
    };

    // §13.2: Validate git hash format — must be 7-40 hex chars only.
    // Prevents shell injection via crafted hash values.
    if hash.len() < 7 || hash.len() > 40 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        tracing::warn!(hash = %hash, "Rollback rejected: invalid git hash format");
        return Json(json!({"success": false, "error": "Invalid git hash format — must be 7-40 hex characters"}));
    }

    let root = std::env::current_dir().unwrap_or_default();
    tracing::info!(hash = %hash, "Version rollback: starting");

    // 1. Stash local changes
    let stash = run_git(&root, &["stash", "push", "-m", "ern-os-auto-stash-before-rollback"]).await;
    tracing::info!(stash = %stash.trim(), "Stashed local changes");

    // 2. Record current position for recovery
    let current = run_git(&root, &["rev-parse", "HEAD"]).await;
    let current = current.trim().to_string();

    // 3. Checkout target
    let checkout = run_git(&root, &["checkout", &hash]).await;
    if checkout.contains("error:") || checkout.contains("fatal:") {
        let _ = run_git(&root, &["checkout", &current]).await;
        let _ = run_git(&root, &["stash", "pop"]).await;
        return Json(json!({
            "success": false,
            "error": format!("Checkout failed: {}", checkout.trim()),
        }));
    }

    // 4. Recompile
    match crate::tools::compiler::run_recompile("", "web").await {
        Ok(msg) => {
            Json(json!({
                "success": true,
                "message": msg,
                "rolled_back_to": hash,
                "action": "Rollback applied. System will restart.",
            }))
        }
        Err(e) => {
            // Recompile failed — revert to original position
            let _ = run_git(&root, &["checkout", &current]).await;
            let _ = run_git(&root, &["stash", "pop"]).await;
            Json(json!({
                "success": false,
                "error": format!("Recompile failed after rollback: {}", e),
                "action": "Rollback reverted. Original version restored.",
            }))
        }
    }
}

/// GET /api/version/history — recent commit history.
pub async fn version_history() -> Json<serde_json::Value> {
    let root = std::env::current_dir().unwrap_or_default();

    let log = run_git(&root, &[
        "log", "--oneline", "--format=%H|%h|%s|%ci|%an", "-50"
    ]).await;

    let current = run_git(&root, &["rev-parse", "HEAD"]).await;
    let current = current.trim();

    let commits: Vec<serde_json::Value> = log.trim().lines().filter_map(|line| {
        let parts: Vec<&str> = line.splitn(5, '|').collect();
        if parts.len() >= 4 {
            Some(json!({
                "hash": parts[0],
                "short_hash": parts[1],
                "message": parts.get(2).unwrap_or(&""),
                "date": parts.get(3).unwrap_or(&""),
                "author": parts.get(4).unwrap_or(&""),
                "current": parts[0] == current,
            }))
        } else {
            None
        }
    }).collect();

    Json(json!({
        "commits": commits,
        "total": commits.len(),
    }))
}

/// Run a git command and return stdout.
async fn run_git(root: &std::path::PathBuf, args: &[&str]) -> String {
    match tokio::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .await
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if !output.status.success() && !stderr.is_empty() {
                format!("{}\n{}", stdout, stderr)
            } else {
                stdout
            }
        }
        Err(e) => format!("error: {}", e),
    }
}
