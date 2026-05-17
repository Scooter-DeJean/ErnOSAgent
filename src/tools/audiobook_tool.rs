// Ern-OS — Audiobook tool — HTTP client to script-reader REST API
//! Integrates with the script-reader audiobook generator (localhost:8000).
//! Provides parse, voices, assign, generate, and status actions.
//! §13.4: Only localhost URLs are accepted to prevent unauthenticated remote requests.

use anyhow::{Context, Result};

/// Validate that a URL is localhost-only (§13.4 compliance).
fn validate_localhost(url: &str) -> Result<()> {
    if url.starts_with("http://localhost") || url.starts_with("http://127.0.0.1") {
        Ok(())
    } else {
        anyhow::bail!("Audiobook tool only accepts localhost URLs (§13.4). Got: {}", url)
    }
}

/// Maximum response body size (10 MiB) to prevent unbounded reads.
const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

/// Execute an audiobook tool action via the script-reader REST API.
pub async fn execute(args: &serde_json::Value) -> Result<String> {
    let action = args["action"].as_str().context("audiobook requires 'action'")?;
    let base_url = args["url"].as_str().unwrap_or("http://localhost:8000");
    validate_localhost(base_url)?;

    tracing::info!(tool = "audiobook", action = %action, url = %base_url, "audiobook tool START");

    match action {
        "parse" => parse_script(args, base_url).await,
        "voices" => list_voices(base_url).await,
        "assign" => assign_voices(args, base_url).await,
        "generate" => generate_audiobook(args, base_url).await,
        "status" => check_status(base_url).await,
        other => anyhow::bail!("Unknown audiobook action: {}", other),
    }
}

async fn parse_script(args: &serde_json::Value, base_url: &str) -> Result<String> {
    let script = args["script"].as_str().context("parse requires 'script'")?;
    let body = serde_json::json!({"script": script});
    post_json(&format!("{}/api/parse", base_url), &body).await
}

async fn list_voices(base_url: &str) -> Result<String> {
    get_text(&format!("{}/api/voices", base_url)).await
}

async fn assign_voices(args: &serde_json::Value, base_url: &str) -> Result<String> {
    let assignments = args.get("assignments").context("assign requires 'assignments'")?;
    let body = serde_json::json!({"assignments": assignments});
    post_json(&format!("{}/api/assign", base_url), &body).await
}

async fn generate_audiobook(args: &serde_json::Value, base_url: &str) -> Result<String> {
    let project_name = args["project_name"].as_str().unwrap_or("audiobook");
    let body = serde_json::json!({"project_name": project_name});
    post_json(&format!("{}/api/generate", base_url), &body).await
}

async fn check_status(base_url: &str) -> Result<String> {
    get_text(&format!("{}/api/status", base_url)).await
}

/// POST JSON to a URL and return the response text, with size limit.
async fn post_json(url: &str, body: &serde_json::Value) -> Result<String> {
    let client = reqwest::Client::new();
    let resp = client.post(url)
        .json(body)
        .send().await
        .with_context(|| format!("POST {} failed — is script-reader running?", url))?;

    let status = resp.status();
    let bytes = resp.bytes().await?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        anyhow::bail!("Response too large: {} bytes (max {})", bytes.len(), MAX_RESPONSE_BYTES);
    }
    let text = String::from_utf8_lossy(&bytes).to_string();
    if !status.is_success() {
        anyhow::bail!("POST {} returned {}: {}", url, status, text);
    }
    Ok(text)
}

/// GET a URL and return the response text, with size limit.
async fn get_text(url: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let resp = client.get(url)
        .send().await
        .with_context(|| format!("GET {} failed — is script-reader running?", url))?;

    let status = resp.status();
    let bytes = resp.bytes().await?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        anyhow::bail!("Response too large: {} bytes (max {})", bytes.len(), MAX_RESPONSE_BYTES);
    }
    let text = String::from_utf8_lossy(&bytes).to_string();
    if !status.is_success() {
        anyhow::bail!("GET {} returned {}: {}", url, status, text);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_localhost_ok() {
        assert!(validate_localhost("http://localhost:8000").is_ok());
        assert!(validate_localhost("http://127.0.0.1:8000").is_ok());
        assert!(validate_localhost("http://localhost").is_ok());
    }

    #[test]
    fn test_validate_localhost_rejects_remote() {
        assert!(validate_localhost("http://example.com:8000").is_err());
        assert!(validate_localhost("https://remote.server.com").is_err());
        assert!(validate_localhost("http://192.168.1.100:8000").is_err());
    }

    #[tokio::test]
    async fn test_unknown_action_fails() {
        let args = serde_json::json!({"action": "destroy"});
        let result = execute(&args).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown audiobook action"));
    }

    #[tokio::test]
    async fn test_parse_requires_script() {
        let args = serde_json::json!({"action": "parse"});
        let result = execute(&args).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("parse requires 'script'"));
    }

    #[tokio::test]
    async fn test_rejects_remote_url() {
        let args = serde_json::json!({"action": "voices", "url": "http://evil.com:8000"});
        let result = execute(&args).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("§13.4"));
    }
}
