// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Ollama provider — secondary provider via OpenAI-compatible API.

use crate::config::OllamaConfig;
use crate::model::ModelSpec;
use crate::provider::{Message, Provider, StreamEvent, stream_parser};
use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::sync::mpsc;

pub struct OllamaProvider {
    config: OllamaConfig,
    client: reqwest::Client,
}

impl OllamaProvider {
    pub fn new(config: &OllamaConfig) -> Self {
        Self {
            config: config.clone(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    fn id(&self) -> &str { "ollama" }
    fn display_name(&self) -> &str { "Ollama" }

    async fn get_model_spec(&self) -> Result<ModelSpec> {
        let url = format!("{}/api/show", self.config.base_url);
        let body = serde_json::json!({ "name": self.config.model });

        let resp = self.client.post(&url).json(&body).send().await
            .context("Failed to connect to Ollama")?;
        let data: serde_json::Value = resp.json().await
            .context("Failed to parse Ollama model info")?;

        // Auto-derive context length — NEVER hardcode (Rule 2.1)
        let context_length = data["model_info"]
            .as_object()
            .and_then(|info| {
                info.iter()
                    .find(|(k, _)| k.contains("context_length"))
                    .and_then(|(_, v)| v.as_u64())
            })
            .map(|v| v as usize)
            .context(
                "Ollama did not report context_length in model_info. \
                 Rule 2.1 forbids hardcoded fallbacks."
            )?;

        // Auto-discover multimodal capabilities from model metadata
        let families = data["details"]["families"].as_array();
        let has_vision = families
            .map(|f| f.iter().any(|v| {
                v.as_str().map_or(false, |s| s.contains("clip") || s.contains("vision"))
            }))
            .unwrap_or(false);

        Ok(ModelSpec {
            name: self.config.model.clone(),
            context_length,
            supports_vision: has_vision,
            supports_video: has_vision, // Vision models support video as frames
            supports_audio: false,
            supports_tool_calling: true,
            supports_thinking: true,
            embedding_dimensions: 0,
        })
    }

    async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&serde_json::Value>,
        _thinking: bool,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let url = format!("{}/v1/chat/completions", self.config.base_url);
        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": messages,
            "stream": true,
        });
        if let Some(tools) = tools {
            body["tools"] = tools.clone();
        }

        // Retry on transient connection errors (connection reset/closed/refused)
        // Matches retry policy in llamacpp.rs per §9.2.
        const MAX_RETRIES: u32 = 3;
        let mut last_err = None;
        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = std::time::Duration::from_millis(500 * (1 << (attempt - 1)));
                tracing::warn!(
                    attempt, delay_ms = delay.as_millis() as u64,
                    "Retrying Ollama request after connection error"
                );
                tokio::time::sleep(delay).await;
            }

            match self.client.post(&url).json(&body).send().await {
                Ok(response) => {
                    if attempt > 0 {
                        tracing::info!(attempt, "Ollama request succeeded after retry");
                    }
                    let (tx, rx) = mpsc::channel(256);
                    tokio::spawn(async move {
                        if let Err(e) = stream_parser::parse_sse_stream(response, tx.clone(), None).await {
                            let _ = tx.send(StreamEvent::Error(e.to_string())).await;
                        }
                    });
                    return Ok(rx);
                }
                Err(e) => {
                    let is_transient = e.is_connect() || e.is_timeout()
                        || e.to_string().contains("connection reset")
                        || e.to_string().contains("connection closed");
                    if is_transient && attempt < MAX_RETRIES {
                        tracing::warn!(
                            attempt, error = %e,
                            "Ollama connection error (will retry)"
                        );
                        last_err = Some(e);
                        continue;
                    }
                    return Err(e).context("Failed to connect to Ollama");
                }
            }
        }
        Err(last_err.map(|e| anyhow::anyhow!(e)).unwrap_or_else(|| anyhow::anyhow!("Ollama retries exhausted")))
    }

    async fn chat_sync(
        &self,
        messages: &[Message],
        tools: Option<&serde_json::Value>,
    ) -> Result<String> {
        let url = format!("{}/v1/chat/completions", self.config.base_url);
        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": messages,
            "stream": false,
        });
        if let Some(tools) = tools {
            body["tools"] = tools.clone();
        }

        let resp = self.client.post(&url).json(&body).send().await
            .context("Failed to connect to Ollama for sync")?;
        let data: serde_json::Value = resp.json().await?;

        Ok(data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let url = format!("{}/v1/embeddings", self.config.base_url);
        let body = serde_json::json!({
            "model": self.config.model,
            "input": text,
        });

        let resp = self.client.post(&url).json(&body).send().await?;
        let data: serde_json::Value = resp.json().await?;

        Ok(data["data"][0]["embedding"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|v| v.as_f64().map(|f| f as f32))
            .collect())
    }

    async fn health(&self) -> bool {
        let url = format!("{}/api/tags", self.config.base_url);
        self.client.get(&url).send().await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    async fn count_tokens(&self, messages: &[Message], tools: Option<&serde_json::Value>, _thinking: bool) -> Result<usize> {
        let url = format!("{}/v1/chat/completions", self.config.base_url);
        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": messages,
            "stream": false,
            "max_tokens": 1,
        });
        if let Some(tools) = tools {
            body["tools"] = tools.clone();
        }

        // Retry on transient connection errors (same policy as chat()/chat_sync() per §9.2)
        let max_retries = 3;
        let mut last_err = None;
        let mut response = None;
        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay = std::time::Duration::from_millis(500 * (1 << (attempt - 1)));
                tracing::warn!(attempt, delay_ms = delay.as_millis() as u64, "Retrying count_tokens after connection error");
                tokio::time::sleep(delay).await;
            }
            match self.client.post(&url).json(&body).send().await {
                Ok(r) => { response = Some(r); break; }
                Err(e) => {
                    let is_connection_error = e.is_connect() || e.is_request()
                        || format!("{}", e).contains("connection closed")
                        || format!("{}", e).contains("connection reset");
                    if is_connection_error && attempt < max_retries {
                        tracing::warn!(attempt, error = %e, "count_tokens connection error (will retry)");
                        last_err = Some(e);
                        continue;
                    }
                    return Err(e).context(format!(
                        "count_tokens: failed to connect to Ollama at {} after {} attempt(s)", url, attempt + 1
                    ));
                }
            }
        }
        let response = match response {
            Some(r) => r,
            None => match last_err {
                Some(e) => return Err(e).context(format!("count_tokens: failed after {} retries", max_retries)),
                None => anyhow::bail!("count_tokens: failed after {} retries — no error captured", max_retries),
            },
        };

        let data: serde_json::Value = response.json().await
            .context("count_tokens: failed to parse Ollama response")?;

        data["usage"]["prompt_tokens"]
            .as_u64()
            .map(|v| v as usize)
            .context("count_tokens: Ollama did not report prompt_tokens in usage")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_id() {
        let config = OllamaConfig::default();
        let provider = OllamaProvider::new(&config);
        assert_eq!(provider.id(), "ollama");
    }
}
