// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! llama-server command-line argument builder.
//!
//! Extracted from llamacpp.rs per §1.1 to keep the orchestrator file
//! under the 500-line cap. Pure function over `&LlamaCppConfig`; no I/O.

use crate::config::LlamaCppConfig;

/// Build the llama-server command-line arguments from a [`LlamaCppConfig`].
pub fn build_server_args(config: &LlamaCppConfig) -> Vec<String> {
    let ctx = if config.context_length > 0 {
        // §2.7: behavior changes are never silent. When the operator
        // sets a non-zero context_length, the auto-derived GGUF value
        // is overridden — log it loudly at server-startup time.
        tracing::info!(
            context_length = config.context_length,
            "Operator-configured context_length overrides GGUF default"
        );
        config.context_length.to_string()
    } else {
        "0".to_string() // 0 = auto-detect from GGUF (legacy default)
    };
    let mut args = vec![
        "--model".to_string(),
        config.model_path.clone(),
        "--port".to_string(),
        config.port.to_string(),
        "--jinja".to_string(), // Use model's built-in Jinja chat template for tool calling
        "-c".to_string(),
        ctx,
        "-np".to_string(),
        "2".to_string(), // Slot 0: main inference. Slot 1: observer audit (dedicated KV cache).
        "-ngl".to_string(),
        config.n_gpu_layers.to_string(),
    ];

    // Multimodal projector for vision support
    if let Some(ref mmproj) = config.mmproj_path {
        args.push("--mmproj".to_string());
        args.push(mmproj.clone());
    }

    // LoRA adapter for incremental learning
    if let Some(ref lora) = config.lora_adapter {
        args.push("--lora".to_string());
        args.push(lora.clone());
    }

    args
}
