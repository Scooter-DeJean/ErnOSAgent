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
        "1".to_string(), // Single slot — prevents unused slots wasting KV cache
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

    // RPC backends — when set, llama-server distributes layers across
    // this node's GPU AND the listed remote rpc-server endpoints.
    // Empty-string treated as None to avoid emitting `--rpc ` with no
    // value (which llama-server rejects).
    if let Some(ref rpc) = config.rpc_servers {
        if !rpc.is_empty() {
            // §13.4: surface the security posture of llama.cpp's
            // rpc-server (unauthenticated, unencrypted) at runtime so
            // operators see it in logs even if they never read the
            // toml field's doc comment. Loud warn at server startup.
            tracing::warn!(
                rpc_servers = %rpc,
                "RPC mesh enabled — llama.cpp rpc-server is UNAUTHENTICATED and UNENCRYPTED. \
                 Only use on trusted private networks (e.g. Tailscale, WireGuard)."
            );
            args.push("--rpc".to_string());
            args.push(rpc.clone());
        }
    }

    args
}
