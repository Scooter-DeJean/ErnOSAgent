//! Browser tool — observable, DOM-aware web browsing with headed/headless modes.
//! Provides open, click, type, navigate, wait, extract, screenshot, evaluate, close.

use anyhow::{Context, Result};
use crate::config::BrowserConfig;
use chromiumoxide::Page;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;



/// Shared browser state — lazily initialized, with named page slots.
pub struct BrowserState {
    pub(crate) browser: Option<chromiumoxide::Browser>,
    _handle: Option<tokio::task::JoinHandle<()>>,
    pub(crate) pages: HashMap<String, Page>,
    pub(crate) next_page_id: usize,
    config: BrowserConfig,
}

impl BrowserState {
    pub fn new() -> Self {
        Self { browser: None, _handle: None, pages: HashMap::new(), next_page_id: 0, config: BrowserConfig::default() }
    }
    pub fn with_config(config: BrowserConfig) -> Self {
        Self { browser: None, _handle: None, pages: HashMap::new(), next_page_id: 0, config }
    }
    /// Get a page by ID, or the most recently opened page.
    pub fn get_page_or_latest(&self, page_id: &str) -> Option<&Page> {
        self.pages.get(page_id).or_else(|| self.pages.values().last())
    }
}

/// Auto-detect Chrome/Chromium binary path.
fn find_chrome_binary() -> Option<String> {
    let candidates = [
        // macOS
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        // Linux
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium-browser",
        "/usr/bin/chromium",
        // Homebrew (macOS / Linux)
        "/opt/homebrew/bin/chromium",
        "/usr/local/bin/chromium",
        // Windows (common paths)
        "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
        "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
    ];
    for c in &candidates {
        if std::path::Path::new(c).exists() {
            return Some(c.to_string());
        }
    }
    // Fallback: check for headless-chrome download in user data
    if let Some(home) = dirs::home_dir() {
        let hc = home.join("Library/Application Support/headless-chrome");
        if hc.exists() {
            if let Ok(entries) = std::fs::read_dir(&hc) {
                for entry in entries.flatten() {
                    let chromium = entry.path().join("chrome-mac/Chromium.app/Contents/MacOS/Chromium");
                    if chromium.exists() {
                        return Some(chromium.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    None
}

/// Ensure browser is initialized, lazily starting Chrome.
pub(crate) async fn ensure_browser(state: &Arc<RwLock<BrowserState>>) -> Result<()> {
    let mut s = state.write().await;
    if s.browser.is_some() { return Ok(()); }

    let config = build_browser_config(&s.config)?;

    let (browser, mut handler) = chromiumoxide::Browser::launch(config)
        .await
        .context("Failed to launch Chrome")?;

    let handle = tokio::spawn(async move {
        while handler.next().await.is_some() {}
    });

    let mode = if s.config.headed { "headed" } else { "headless" };
    tracing::info!(mode, "Chrome browser initialized");
    s.browser = Some(browser);
    s._handle = Some(handle);
    Ok(())
}

/// Build the Chrome browser configuration.
fn build_browser_config(config: &crate::config::BrowserConfig) -> Result<chromiumoxide::BrowserConfig> {
    let chrome = find_chrome_binary()
        .context("Chrome/Chromium not found. Install Google Chrome.")?;

    let user_data_dir = std::env::temp_dir()
        .join(format!("ern-os-chrome-{}", std::process::id()));
    std::fs::create_dir_all(&user_data_dir).ok();

    let mut builder = chromiumoxide::BrowserConfig::builder()
        .chrome_executable(chrome)
        .user_data_dir(user_data_dir)
        .window_size(config.window_width, config.window_height);

    if config.headed {
        builder = builder.with_head();
    } else {
        builder = builder.arg("--headless=new");
    }

    builder = builder
        .arg("--disable-gpu")
        .arg("--no-sandbox")
        .arg("--disable-dev-shm-usage")
        .arg("--no-first-run")
        .arg("--disable-extensions")
        .arg("--disable-default-apps")
        .arg("--disable-background-networking")
        .arg("--disable-sync")
        .arg("--disable-translate");

    builder.build().map_err(|e| anyhow::anyhow!("{}", e))
}

// ─── Legacy API (kept for backwards compat) ───

/// Browse a URL and extract page content as markdown-formatted text.
pub async fn browse_url(
    state: &Arc<RwLock<BrowserState>>,
    url: &str,
) -> Result<String> {
    ensure_browser(state).await?;
    let s = state.read().await;
    let browser = s.browser.as_ref().context("Browser not initialized")?;

    let page = browser.new_page(url).await
        .context("Failed to open page")?;

    let title = page.get_title().await
        .unwrap_or_default()
        .unwrap_or_default();

    /// Maximum characters extracted from a page's visible text.
    /// Limits context window consumption while capturing key content.
    const PAGE_CONTENT_LIMIT: usize = 8000;

    let js_extract = format!("document.body.innerText.substring(0, {})", PAGE_CONTENT_LIMIT);
    let content = page.evaluate(
        js_extract.as_str()
    ).await
    .context("Failed to extract page content")?
    .into_value::<String>()
    .unwrap_or_default();

    page.close().await.ok();

    Ok(format!("# {}\n\nURL: {}\n\n{}", title, url, content))
}

/// Take a screenshot of a URL, returning base64-encoded PNG.
pub async fn screenshot_url(
    state: &Arc<RwLock<BrowserState>>,
    url: &str,
) -> Result<String> {
    ensure_browser(state).await?;
    let s = state.read().await;
    let browser = s.browser.as_ref().context("Browser not initialized")?;

    let page = browser.new_page(url).await
        .context("Failed to open page")?;

    let screenshot = page.screenshot(
        chromiumoxide::page::ScreenshotParams::builder()
            .full_page(true)
            .build(),
    ).await.context("Failed to capture screenshot")?;

    page.close().await.ok();

    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.encode(&screenshot))
}

// ─── Interactive Browser API ───

/// Dispatch a browser action by name.
/// All CDP calls are wrapped in `tokio::time::timeout` using `BrowserConfig.timeout_ms`
/// to prevent deadlocks on unresponsive pages or non-interactive elements (§2.4).
pub async fn execute_action(
    state: &Arc<RwLock<BrowserState>>,
    args: &serde_json::Value,
) -> Result<String> {
    // §2.4: Reject missing action — never silently default to "open"
    let action = match args["action"].as_str() {
        Some(a) => a,
        None => anyhow::bail!(
            "Missing 'action' field. Available actions: open, click, type, navigate, \
             wait, extract, screenshot, evaluate, close, list"
        ),
    };

    let timeout_ms = {
        let s = state.read().await;
        s.config.timeout_ms
    };

    match tokio::time::timeout(
        std::time::Duration::from_millis(timeout_ms),
        dispatch_action(state, action, args),
    ).await {
        Ok(result) => result,
        Err(_) => anyhow::bail!(
            "Browser operation '{}' timed out after {}ms. \
             The page may be unresponsive or the target element non-interactive.",
            action, timeout_ms
        ),
    }
}

/// Inner dispatch — called within the timeout wrapper.
async fn dispatch_action(
    state: &Arc<RwLock<BrowserState>>,
    action: &str,
    args: &serde_json::Value,
) -> Result<String> {
    use super::browser_actions::*;
    match action {
        "open" => action_open(state, args).await,
        "click" => action_click(state, args).await,
        "type" => action_type(state, args).await,
        "navigate" => action_navigate(state, args).await,
        "wait" => action_wait(state, args).await,
        "extract" => action_extract(state, args).await,
        "screenshot" => action_screenshot(state, args).await,
        "evaluate" => action_evaluate(state, args).await,
        "close" => action_close(state, args).await,
        "list" => action_list(state).await,
        other => anyhow::bail!("Unknown browser action: {}", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_chrome_binary() {
        let binary = find_chrome_binary();
        if let Some(path) = &binary {
            assert!(std::path::Path::new(path).exists());
        }
    }

    #[test]
    fn test_browser_state_new() {
        let state = BrowserState::new();
        assert!(state.browser.is_none());
        assert!(state.pages.is_empty());
    }

    #[tokio::test]
    async fn test_missing_action_rejected() {
        let state = Arc::new(RwLock::new(BrowserState::new()));
        // Malformed args with no "action" field
        let args = serde_json::json!({"introspect{action": "snapshot"});
        let result = execute_action(&state, &args).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Missing 'action' field"), "Error should mention missing action: {}", err);
        assert!(err.contains("open"), "Error should list available actions: {}", err);
    }

    #[tokio::test]
    async fn test_unknown_action_rejected() {
        let state = Arc::new(RwLock::new(BrowserState::new()));
        let args = serde_json::json!({"action": "destroy"});
        let result = execute_action(&state, &args).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown browser action"));
    }

    #[tokio::test]
    async fn test_timeout_fires_on_hung_future() {
        // Simulate a hung CDP call — timeout should fire
        let timeout_ms = 100; // 100ms for test speed
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            tokio::time::sleep(std::time::Duration::from_secs(60)), // "hangs" for 60s
        ).await;
        assert!(result.is_err(), "Timeout should fire on hung future");
    }
}
