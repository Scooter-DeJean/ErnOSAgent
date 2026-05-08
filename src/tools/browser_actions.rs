//! Browser action handlers — click, type, navigate, wait, extract, screenshot, evaluate, close, list.
//! Split from `browser_tool.rs` per §10.1 (max 500 lines per file).

use anyhow::{Context, Result};
use chromiumoxide::Page;
use std::sync::Arc;
use tokio::sync::RwLock;
use super::browser_tool::{BrowserState, ensure_browser};

/// Open a new page and return its page_id.
pub(crate) async fn action_open(state: &Arc<RwLock<BrowserState>>, args: &serde_json::Value) -> Result<String> {
    ensure_browser(state).await?;
    let url = args["url"].as_str().unwrap_or("about:blank");
    let mut s = state.write().await;

    let browser = s.browser.as_ref().context("Browser not initialized")?;
    let page = browser.new_page(url).await
        .context("Failed to open page")?;

    let title = page.get_title().await.unwrap_or_default().unwrap_or_default();
    let page_id = format!("page_{}", s.next_page_id);
    let context = get_page_context(&page).await;
    s.next_page_id += 1;
    s.pages.insert(page_id.clone(), page);

    tracing::info!(page_id = %page_id, url = %url, "Browser page opened");
    Ok(format!("Opened page '{}': {} — {}{}", page_id, url, title, context))
}

/// Get a page by ID from state (read lock).
fn get_page<'a>(state: &'a tokio::sync::RwLockReadGuard<BrowserState>, args: &serde_json::Value) -> Result<&'a Page> {
    let page_id = args["page_id"].as_str().unwrap_or("page_0");
    state.pages.get(page_id)
        .with_context(|| format!("Page '{}' not found. Open pages: {:?}", page_id, state.pages.keys().collect::<Vec<_>>()))
}

/// Extract a DOM summary from the page: title, URL, and interactive elements.
/// This gives the model awareness of what's actually on the page so it can
/// choose valid selectors instead of guessing blindly.
async fn get_page_context(page: &Page) -> String {
    let js = r#"(() => {
        try {
            const title = document.title || '';
            const url = location.href || '';
            const parts = [];
            const headings = [...document.querySelectorAll('h1,h2,h3')].slice(0, 10);
            for (const h of headings) {
                parts.push('  <' + h.tagName.toLowerCase() + '>' + (h.innerText||'').trim().substring(0, 80) + '</' + h.tagName.toLowerCase() + '>');
            }
            const links = [...document.querySelectorAll('a[href]')].slice(0, 20);
            for (const a of links) {
                parts.push('  <a href="' + a.href + '">' + (a.innerText||'').trim().substring(0, 50) + '</a>');
            }
            const buttons = [...document.querySelectorAll('button, input[type=submit], input[type=button]')].slice(0, 10);
            for (const b of buttons) {
                const tag = b.tagName.toLowerCase();
                parts.push('  <' + tag + '>' + ((b.innerText||b.value||'')).trim() + '</' + tag + '>');
            }
            const inputs = [...document.querySelectorAll('input:not([type=hidden]):not([type=submit]):not([type=button]), textarea, select')].slice(0, 10);
            for (const i of inputs) {
                const tag = i.tagName.toLowerCase();
                parts.push('  <' + tag + ' type="' + (i.type||'') + '" name="' + (i.name||'') + '" id="' + (i.id||'') + '">');
            }
            return JSON.stringify({
                title: title, url: url,
                links: links.length, buttons: buttons.length, inputs: inputs.length,
                elements: parts.join('\n')
            });
        } catch(e) { return '{}'; }
    })()"#;

    match page.evaluate(js).await {
        Ok(val) => {
            let raw = val.into_value::<String>().unwrap_or_default();
            let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
            format_page_context_from_json(&parsed)
        }
        Err(_) => String::new()
    }
}

/// Format page context JSON into a human-readable DOM summary.
fn format_page_context_from_json(parsed: &serde_json::Value) -> String {
    let title = parsed["title"].as_str().unwrap_or("");
    let url = parsed["url"].as_str().unwrap_or("");
    let elements = parsed["elements"].as_str().unwrap_or("(empty page)");

    if elements.is_empty() || elements == "(empty page)" {
        format!("\n\n--- Page Context ---\nTitle: {}\nURL: {}\nLinks: 0 | Buttons: 0 | Inputs: 0\nDOM: (empty page — no interactive elements)", title, url)
    } else {
        format!("\n\n--- Page Context ---\nTitle: {}\nURL: {}\nLinks: {} | Buttons: {} | Inputs: {}\nDOM:\n{}",
            title, url, parsed["links"], parsed["buttons"], parsed["inputs"], elements)
    }
}

/// Click an element by CSS selector. Returns page context on failure.
pub(crate) async fn action_click(state: &Arc<RwLock<BrowserState>>, args: &serde_json::Value) -> Result<String> {
    let selector = args["selector"].as_str().context("'selector' required for click")?;
    let s = state.read().await;
    let page = get_page(&s, args)?;

    match page.find_element(selector).await {
        Ok(element) => {
            element.click().await
                .with_context(|| format!("Failed to click: {}", selector))?;
            let context = get_page_context(page).await;
            Ok(format!("Clicked: {}{}", selector, context))
        }
        Err(_) => {
            let context = get_page_context(page).await;
            anyhow::bail!(
                "Element '{}' not found on this page.{}\n\nUse the DOM above to choose a valid selector.",
                selector, context
            )
        }
    }
}

/// Type text into an element.
pub(crate) async fn action_type(state: &Arc<RwLock<BrowserState>>, args: &serde_json::Value) -> Result<String> {
    let selector = args["selector"].as_str().context("'selector' required for type")?;
    let text = args["text"].as_str().context("'text' required for type")?;
    let s = state.read().await;
    let page = get_page(&s, args)?;

    match page.find_element(selector).await {
        Ok(element) => {
            element.click().await.ok();
            element.type_str(text).await
                .with_context(|| format!("Failed to type into: {}", selector))?;
            Ok(format!("Typed '{}' into {}", text, selector))
        }
        Err(_) => {
            let context = get_page_context(page).await;
            anyhow::bail!(
                "Element '{}' not found for typing.{}\n\nUse the DOM above to choose a valid selector.",
                selector, context
            )
        }
    }
}

/// Navigate an existing page to a new URL.
pub(crate) async fn action_navigate(state: &Arc<RwLock<BrowserState>>, args: &serde_json::Value) -> Result<String> {
    let url = args["url"].as_str().context("'url' required for navigate")?;
    let s = state.read().await;
    let page = get_page(&s, args)?;

    page.goto(url).await
        .with_context(|| format!("Failed to navigate to: {}", url))?;

    let title = page.get_title().await.unwrap_or_default().unwrap_or_default();
    let context = get_page_context(page).await;
    Ok(format!("Navigated to: {} — {}{}", url, title, context))
}

/// Wait for an element to appear.
pub(crate) async fn action_wait(state: &Arc<RwLock<BrowserState>>, args: &serde_json::Value) -> Result<String> {
    let selector = args["selector"].as_str().context("'selector' required for wait")?;
    /// Default wait timeout in milliseconds for element appearance.
    const DEFAULT_WAIT_MS: u64 = 5000;
    let timeout_ms = args["timeout_ms"].as_u64().unwrap_or(DEFAULT_WAIT_MS);

    // §13.2: Validate CSS selector to prevent JavaScript injection.
    // Only allow characters that are valid in CSS selectors.
    if !selector.chars().all(|c| c.is_alphanumeric() || " -_.:# []>+~=^$*|,\"()".contains(c)) {
        anyhow::bail!("Selector contains disallowed characters: {}", selector);
    }

    let s = state.read().await;
    let page = get_page(&s, args)?;

    // Use serde_json for safe string escaping into JS context
    let safe_selector = serde_json::to_string(selector)
        .context("Failed to serialize selector")?;

    let js = format!(
        r#"new Promise((resolve, reject) => {{
            const start = Date.now();
            const check = () => {{
                if (document.querySelector({sel})) resolve(true);
                else if (Date.now() - start > {timeout}) reject('timeout');
                else setTimeout(check, 100);
            }};
            check();
        }})"#,
        sel = safe_selector, timeout = timeout_ms
    );

    page.evaluate(js).await
        .with_context(|| format!("Wait for '{}' timed out after {}ms", selector, timeout_ms))?;

    Ok(format!("Element found: {}", selector))
}

/// Extract text content from an element.
pub(crate) async fn action_extract(state: &Arc<RwLock<BrowserState>>, args: &serde_json::Value) -> Result<String> {
    let selector = args["selector"].as_str().context("'selector' required for extract")?;
    let attribute = args["attribute"].as_str();
    let s = state.read().await;
    let page = get_page(&s, args)?;

    let js = if let Some(attr) = attribute {
        format!("document.querySelector('{}')?.getAttribute('{}')", selector, attr)
    } else {
        format!("document.querySelector('{}')?.innerText?.substring(0, 4000)", selector)
    };

    let result = page.evaluate(js).await
        .context("Extract evaluation failed")?
        .into_value::<serde_json::Value>()
        .unwrap_or(serde_json::Value::Null);

    Ok(serde_json::to_string_pretty(&result)?)
}

/// Take a screenshot and save to data/images/.
pub(crate) async fn action_screenshot(state: &Arc<RwLock<BrowserState>>, args: &serde_json::Value) -> Result<String> {
    let s = state.read().await;
    let page = get_page(&s, args)?;

    let screenshot = page.screenshot(
        chromiumoxide::page::ScreenshotParams::builder()
            .full_page(true)
            .build(),
    ).await.context("Screenshot failed")?;

    let id = uuid::Uuid::new_v4().to_string();
    let filename = format!("{}.png", id);
    let dir = std::path::PathBuf::from("data/images");
    std::fs::create_dir_all(&dir).ok();
    std::fs::write(dir.join(&filename), &screenshot)
        .context("Failed to save screenshot")?;

    Ok(format!("![screenshot](/api/images/{})", filename))
}

/// Evaluate JavaScript on a page.
pub(crate) async fn action_evaluate(state: &Arc<RwLock<BrowserState>>, args: &serde_json::Value) -> Result<String> {
    let script = args["script"].as_str().context("'script' required for evaluate")?;
    let s = state.read().await;
    let page = get_page(&s, args)?;

    let result = page.evaluate(script).await
        .context("JavaScript evaluation failed")?
        .into_value::<serde_json::Value>()
        .unwrap_or(serde_json::Value::Null);

    Ok(serde_json::to_string_pretty(&result)?)
}

/// Close a page and release resources.
pub(crate) async fn action_close(state: &Arc<RwLock<BrowserState>>, args: &serde_json::Value) -> Result<String> {
    let page_id = args["page_id"].as_str().unwrap_or("page_0");
    let mut s = state.write().await;

    if let Some(page) = s.pages.remove(page_id) {
        page.close().await.ok();
        tracing::info!(page_id = %page_id, "Browser page closed");
        Ok(format!("Closed page '{}'", page_id))
    } else {
        anyhow::bail!("Page '{}' not found", page_id)
    }
}

/// List all open pages.
pub(crate) async fn action_list(state: &Arc<RwLock<BrowserState>>) -> Result<String> {
    let s = state.read().await;
    if s.pages.is_empty() {
        return Ok("No pages open.".to_string());
    }
    let list: Vec<String> = s.pages.keys().cloned().collect();
    Ok(format!("Open pages: {}", list.join(", ")))
}
