// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! System logs tool — read-only access to engine logs for self-healing.
//!
//! The agent can read its own error/warn output to diagnose and fix
//! issues proactively. Write access is NOT provided.
//!
//! Reads daily rotating log files (`ern-os.log.YYYY-MM-DD`) produced by
//! `tracing_appender::rolling::daily` in `src/logging/mod.rs`.

use std::path::{Path, PathBuf};

fn paginate_lines(items: &[String], page: usize, per_page: usize) -> String {
    let total = items.len();
    if total == 0 { return String::new(); }
    let total_pages = (total + per_page - 1) / per_page;
    let page = page.min(total_pages);
    let start = (page - 1) * per_page;
    let end = (start + per_page).min(total);
    let mut out = items[start..end].join("\n");
    out.push_str(&format!("\n--- Page {}/{} ({} total) ---", page, total_pages, total));
    out
}

fn get_page(args: &serde_json::Value) -> usize {
    args["page"].as_u64().unwrap_or(1).max(1) as usize
}

fn get_per_page(args: &serde_json::Value) -> usize {
    args["per_page"].as_u64().unwrap_or(30).clamp(1, 100) as usize
}

/// Discover daily rotating log files (`ern-os.log.*`), newest first.
fn discover_log_files(data_dir: &Path) -> Vec<PathBuf> {
    let log_dir = data_dir.join("logs");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&log_dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("ern-os.log."))
                .unwrap_or(false)
        })
        .collect();
    files.sort_by(|a, b| b.cmp(a)); // newest first
    files
}

/// Execute a system_logs action.
pub fn execute(args: &serde_json::Value, data_dir: &Path) -> anyhow::Result<String> {
    tracing::info!(tool = "system_logs", "tool START");
    let action = args["action"].as_str().unwrap_or("tail");
    match action {
        "tail" => tail_logs(data_dir, args),
        "errors" => grep_errors(data_dir, args),
        "search" => search_logs(data_dir, args),
        "self_edits" => list_self_edits(data_dir, args),
        other => Ok(format!("Unknown system_logs action: {}", other)),
    }
}

/// Return the last N lines from the most recent engine log file.
/// Automatically restricts to lines produced since the current process started
/// (i.e. since the last "Ern-OS starting" entry) — prevents stale log pollution.
fn tail_logs(data_dir: &Path, args: &serde_json::Value) -> anyhow::Result<String> {
    let log_files = discover_log_files(data_dir);
    let log_path = match log_files.first() {
        Some(p) => p,
        None => return Ok("No log files found. The engine may not have generated logs yet.".into()),
    };

    let content = std::fs::read_to_string(log_path)?;
    let lines: Vec<String> = current_session_lines(&content)
        .rev()
        .map(|s| s.to_string())
        .collect();
    if lines.is_empty() {
        return Ok("No log lines for the current session yet.".to_string());
    }
    Ok(paginate_lines(&lines, get_page(args), get_per_page(args)))
}

/// Grep for ERROR and WARN lines — current session only.
fn grep_errors(data_dir: &Path, args: &serde_json::Value) -> anyhow::Result<String> {
    let n = args["max"].as_u64().unwrap_or(30) as usize;
    let mut errors = Vec::new();

    // Only the most recent log file; older files are previous sessions.
    if let Some(log_path) = discover_log_files(data_dir).into_iter().next() {
        if let Ok(content) = std::fs::read_to_string(&log_path) {
            let fname = log_path.file_name().unwrap_or_default().to_string_lossy();
            for line in current_session_lines(&content) {
                let lower = line.to_lowercase();
                if lower.contains("\"error\"") || lower.contains("\"warn\"")
                    || lower.contains("panic") || lower.contains("failed")
                {
                    errors.push(format!("[{}] {}", fname, line));
                    if errors.len() >= n { break; }
                }
            }
        }
    }

    // Return latest first
    errors.reverse();
    errors.truncate(n);

    if errors.is_empty() {
        Ok("No errors or warnings in the current session logs.".into())
    } else {
        Ok(paginate_lines(&errors, get_page(args), get_per_page(args)))
    }
}

/// Search logs for a specific pattern — current session only.
fn search_logs(data_dir: &Path, args: &serde_json::Value) -> anyhow::Result<String> {
    let pattern = args["pattern"].as_str().unwrap_or("");
    if pattern.is_empty() {
        anyhow::bail!("Missing 'pattern' parameter");
    }
    let n = args["max"].as_u64().unwrap_or(20) as usize;
    let lower_pattern = pattern.to_lowercase();
    let mut matches = Vec::new();

    // Search current session lines of the most recent daily log.
    if let Some(path) = discover_log_files(data_dir).into_iter().next() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            let fname = path.file_name().unwrap_or_default().to_string_lossy();
            for (i, line) in current_session_lines(&content).enumerate() {
                if line.to_lowercase().contains(&lower_pattern) {
                    matches.push(format!("[{}:{}] {}", fname, i + 1, line));
                    if matches.len() >= n { break; }
                }
            }
        }
    }

    // Also search auxiliary log files (these are session-agnostic by design).
    let aux_files = [
        data_dir.join("recompile_log.md"),
        data_dir.join("self_edit_log.jsonl"),
    ];
    for path in &aux_files {
        if !path.exists() || matches.len() >= n { continue; }
        if let Ok(content) = std::fs::read_to_string(path) {
            let fname = path.file_name().unwrap_or_default().to_string_lossy();
            for (i, line) in content.lines().enumerate() {
                if line.to_lowercase().contains(&lower_pattern) {
                    matches.push(format!("[{}:{}] {}", fname, i + 1, line));
                    if matches.len() >= n { break; }
                }
            }
        }
    }

    if matches.is_empty() {
        Ok(format!("No matches for '{}' in current session logs.", pattern))
    } else {
        Ok(paginate_lines(&matches, get_page(args), get_per_page(args)))
    }
}

/// List recent self-edit audit entries.
fn list_self_edits(data_dir: &Path, args: &serde_json::Value) -> anyhow::Result<String> {
    let path = data_dir.join("self_edit_log.jsonl");

    if !path.exists() {
        return Ok("No self-edit log found. No codebase edits have been made.".into());
    }

    let content = std::fs::read_to_string(&path)?;
    let lines: Vec<String> = content.lines().rev().map(|s| s.to_string()).collect();
    if lines.is_empty() {
        return Ok("No self-edit entries.".to_string());
    }
    Ok(paginate_lines(&lines, get_page(args), get_per_page(args)))
}

/// Return an iterator over lines from the current process session only.
///
/// Finds the position of the last `"Ern-OS starting"` entry in the log
/// (the most recent process start) and yields only the lines from that
/// point forward. This is the authoritative session boundary — the log
/// is append-only and `tracing_appender` guarantees ordering.
///
/// If no session-start marker is found (e.g. empty log or a log without
/// a start event), falls back to all lines so no data is silently lost.
fn current_session_lines(content: &str) -> impl DoubleEndedIterator<Item = &str> {
    let lines: Vec<&str> = content.lines().collect();

    // Walk backwards to find the last occurrence of the startup marker.
    let session_start_idx = lines
        .iter()
        .rposition(|line| line.contains("Ern-OS starting"));

    let start = session_start_idx.unwrap_or(0);
    // Return a slice iterator — supports rev() for tail_logs
    lines[start..].to_vec().into_iter().collect::<Vec<_>>().into_iter()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_dir(name: &str) -> std::path::PathBuf {
        let d = std::path::PathBuf::from(format!("target/test_system_logs_{}", name));
        let _ = fs::create_dir_all(d.join("logs"));
        d
    }

    #[test]
    fn test_tail_empty() {
        let dir = test_dir("tail_empty2");
        // Remove any existing log files
        let _ = fs::remove_dir_all(dir.join("logs"));
        let _ = fs::create_dir_all(dir.join("logs"));
        let args = serde_json::json!({"action": "tail"});
        let result = execute(&args, &dir);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("No log files"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_tail_with_content() {
        let dir = test_dir("tail_content2");
        let log = dir.join("logs/ern-os.log.2026-04-28");
        fs::write(&log, "line1\nline2\nline3\nline4\nline5\n").unwrap();

        let args = serde_json::json!({"action": "tail"});
        let result = execute(&args, &dir).unwrap();
        // tail returns lines in reverse order (newest first)
        assert!(result.contains("line3"));
        assert!(result.contains("line5"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_errors_grep() {
        let dir = test_dir("errors_grep2");
        let log = dir.join("logs/ern-os.log.2026-04-28");
        fs::write(&log, "{\"level\":\"INFO\",\"fields\":{\"message\":\"all good\"}}\n\
                         {\"level\":\"ERROR\",\"fields\":{\"message\":\"something broke\"}}\n\
                         {\"level\":\"WARN\",\"fields\":{\"message\":\"caution\"}}\n\
                         {\"level\":\"INFO\",\"fields\":{\"message\":\"ok\"}}\n").unwrap();

        let args = serde_json::json!({"action": "errors"});
        let result = execute(&args, &dir).unwrap();
        assert!(result.contains("something broke"));
        assert!(result.contains("caution"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_search_pattern() {
        let dir = test_dir("search2");
        let log = dir.join("logs/ern-os.log.2026-04-28");
        fs::write(&log, "Starting server\nListening on 3000\nConnection established\n").unwrap();

        let args = serde_json::json!({"action": "search", "pattern": "3000"});
        let result = execute(&args, &dir).unwrap();
        assert!(result.contains("Listening on 3000"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_self_edits_empty() {
        let dir = test_dir("edits_empty2");
        let _ = fs::remove_file(dir.join("self_edit_log.jsonl"));
        let args = serde_json::json!({"action": "self_edits"});
        let result = execute(&args, &dir).unwrap();
        assert!(result.contains("No self-edit log"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_discover_log_files_ordering() {
        let dir = test_dir("discover2");
        let _ = fs::create_dir_all(dir.join("logs"));
        fs::write(dir.join("logs/ern-os.log.2026-04-27"), "old").unwrap();
        fs::write(dir.join("logs/ern-os.log.2026-04-28"), "new").unwrap();
        fs::write(dir.join("logs/other.txt"), "ignore").unwrap();
        let files = discover_log_files(&dir);
        assert_eq!(files.len(), 2);
        // Newest first
        assert!(files[0].to_str().unwrap().contains("04-28"));
        assert!(files[1].to_str().unwrap().contains("04-27"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_errors_grep_current_session_only() {
        // Verifies that errors() does NOT search previous-day log files —
        // only the most recent log, and only lines since the last startup marker.
        let dir = test_dir("errors_multi2");
        // Old day's log — must be ignored
        fs::write(
            dir.join("logs/ern-os.log.2026-04-27"),
            "{\"level\":\"ERROR\",\"fields\":{\"message\":\"old session error\"}}\n",
        ).unwrap();
        // Current day's log — has a startup marker followed by a new error
        fs::write(
            dir.join("logs/ern-os.log.2026-04-28"),
            "{\"level\":\"INFO\",\"fields\":{\"message\":\"Ern-OS starting\"}}\n\
             {\"level\":\"ERROR\",\"fields\":{\"message\":\"current session error\"}}\n",
        ).unwrap();

        let args = serde_json::json!({"action": "errors"});
        let result = execute(&args, &dir).unwrap();
        assert!(result.contains("current session error"), "current session error must appear");
        assert!(!result.contains("old session error"), "old session error must be excluded");

        let _ = fs::remove_dir_all(&dir);
    }
}
