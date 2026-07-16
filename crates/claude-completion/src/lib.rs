#![deny(unsafe_code)]
#![deny(clippy::all)]
#![deny(unreachable_pub)]
#![warn(missing_docs)]

//! claude-completion — Anthropic API LLM provider for Unicity AOS.
//!
//! Implements the `astrid:llm@1.0.0` provider contract against
//! Anthropic's Messages API. Subscribes to:
//!
//! - `llm.v1.request.describe` — replies with a single `ProviderEntry`
//!   advertising the `claude` id, request topic, and stream topic.
//! - `llm.v1.request.generate.claude` — builds an Anthropic request body,
//!   streams the SSE response, and re-publishes the demuxed
//!   [`astrid_sdk::types::StreamEvent`]s on `llm.v1.stream.claude`.
//!
//! This is the per-turn completion path. It bills against Anthropic API
//! usage credits, separate from the Agent SDK credit consumed by
//! `claude -p` (see the sibling crate `claude-runner`). When `capsule-router`
//! picks "Claude" as the model for a turn but doesn't want to spin up
//! a full Claude Code agent loop, the request lands here.

mod anthropic;
mod schemas;
mod sse;

use astrid_sdk::prelude::*;
use astrid_sdk::types::{IpcPayload, StreamEvent};

use crate::anthropic::{STREAM_TOPIC, execute_request, publish_stream};

/// Provider id advertised on the registry describe-collect topic.
const PROVIDER_ID: &str = "claude";

/// claude-completion provider.
#[derive(Default)]
pub struct ClaudeCompletion;

#[capsule]
impl ClaudeCompletion {
    /// Reply to the registry's discovery broadcast with our `ProviderEntry`.
    ///
    /// The registry subscribes to `llm.v1.response.describe` and drains
    /// every reply for a bounded window. The return value is preserved
    /// for legacy interceptor-result callers, but the explicit
    /// `ipc::publish_json` below is what registry post-#752 consumes.
    #[astrid::interceptor("llm_describe")]
    pub fn llm_describe(&self, _payload: serde_json::Value) -> Result<serde_json::Value, SysError> {
        let context_window = env::var("context_window")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(200_000);
        let max_output_tokens = env::var("max_output_tokens")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(8_192);

        let response = serde_json::json!({
            "providers": [{
                "id": PROVIDER_ID,
                "description": "Claude via Anthropic Messages API",
                "capabilities": ["streaming", "tools", "vision"],
                "request_topic": "llm.v1.request.generate.claude",
                "stream_topic": STREAM_TOPIC,
                "context_window": context_window,
                "max_output_tokens": max_output_tokens,
            }]
        });

        ipc::publish_json("llm.v1.response.describe", &response)?;
        Ok(response)
    }

    /// Generate a streaming completion for an LLM request.
    ///
    /// Pre-stream failures surface as a single `StreamEvent::Error` on
    /// the stream topic; in-stream parse failures are skipped (logged).
    /// `request_id` correlation is preserved on every published event.
    #[astrid::interceptor("handle_llm_request")]
    pub fn handle_llm_request(&self, req: IpcPayload) -> Result<(), SysError> {
        if let IpcPayload::LlmRequest {
            request_id,
            model,
            messages,
            tools,
            system,
        } = req
            && let Err(e) = execute_request(request_id, &model, &messages, &tools, &system)
        {
            log::error(format!("claude-completion: LLM request failed: {e}"));
            let _ = publish_stream(request_id, StreamEvent::Error(e.to_string()));
        }
        Ok(())
    }
}
