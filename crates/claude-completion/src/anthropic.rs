//! Anthropic Messages API client logic for claude-completion.
//!
//! Builds the request body from an Astrid `LlmRequest`, drives a streaming
//! HTTP call via `http::stream_start`, and translates Anthropic SSE events
//! into `astrid_sdk` `StreamEvent`s published on `llm.v1.stream.claude`.

use astrid_sdk::prelude::*;
use astrid_sdk::types::{
    ContentPart, IpcPayload, LlmToolDefinition, Message, MessageContent, MessageRole, StreamEvent,
};
use serde_json::Value;
use uuid::Uuid;

use crate::schemas::{AnthropicUsage, ContentBlock, Delta, StreamingEvent};
use crate::sse::SseParser;

/// Topic the LLM provider contract requires us to publish stream events on.
pub(crate) const STREAM_TOPIC: &str = "llm.v1.stream.claude";

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 8192;

/// Build the Anthropic request body, fire the streaming HTTP call, and
/// publish stream events as they arrive.
pub(crate) fn execute_request(
    request_id: Uuid,
    model: &str,
    messages: &[Message],
    tools: &[LlmToolDefinition],
    system: &str,
) -> Result<(), SysError> {
    // API key is read at request time only — never cached at module scope,
    // never logged, never published.
    let api_key = env::var("api_key").unwrap_or_default();
    if api_key.is_empty() {
        return Err(SysError::ApiError("api_key not configured".into()));
    }

    // The contract leaves leading-system extraction to the provider. The
    // explicit `system` field on `LlmRequest` wins; if it's empty, fall
    // back to the leading system message in `messages`.
    let (effective_system, conversation) = split_system(system, messages);

    let resolved_model = if model.is_empty() {
        env::var("model").unwrap_or_else(|_| "claude-sonnet-4-6".into())
    } else {
        model.to_string()
    };

    let max_tokens = env::var("max_output_tokens")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS);

    let mut request_body = serde_json::json!({
        "model": resolved_model,
        "max_tokens": max_tokens,
        "messages": conversation.iter().map(convert_message).collect::<Vec<_>>(),
        "stream": true,
    });

    // System prompt: prompt-cache breakpoint #1 (5m ephemeral).
    if !effective_system.is_empty() {
        request_body["system"] = serde_json::json!([{
            "type": "text",
            "text": effective_system,
            "cache_control": { "type": "ephemeral", "ttl": "5m" },
        }]);
    }

    // Tools: prompt-cache breakpoint #2 sits on the LAST tool definition;
    // Anthropic seals all earlier tools at the same breakpoint.
    if !tools.is_empty() {
        let last_idx = tools.len().saturating_sub(1);
        let api_tools: Vec<Value> = tools
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let mut obj = serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                });
                if i == last_idx {
                    obj["cache_control"] = serde_json::json!({ "type": "ephemeral", "ttl": "5m" });
                }
                obj
            })
            .collect();
        request_body["tools"] = Value::Array(api_tools);
    }

    if let Ok(temp_raw) = env::var("temperature")
        && let Ok(t) = temp_raw.trim().parse::<f64>()
    {
        request_body["temperature"] = serde_json::json!(t);
    }

    let req = http::Request::post(API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&request_body)?;

    let stream = http::stream_start(&req)?;

    if !(200..300).contains(&stream.status()) {
        // Pre-stream HTTP failure: drain ≤4 KiB of body for diagnostics
        // and surface a single Error event. No further events follow.
        //
        // We accumulate raw bytes (not a `String`) and decode once at the
        // end via `from_utf8_lossy` so we never trip the
        // `String::truncate` char-boundary panic — which in WASM would
        // abort the guest module and the consumer would never see a
        // `StreamEvent::Error`. Worst case we overshoot the cap by one
        // chunk; that's fine for a diagnostic.
        const DIAGNOSTIC_CAP: usize = 4096;
        let mut diagnostic_bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.read_chunk()? {
            let take = DIAGNOSTIC_CAP.saturating_sub(diagnostic_bytes.len());
            if take == 0 {
                break;
            }
            let slice = if chunk.len() > take {
                &chunk[..take]
            } else {
                &chunk[..]
            };
            diagnostic_bytes.extend_from_slice(slice);
            if diagnostic_bytes.len() >= DIAGNOSTIC_CAP {
                break;
            }
        }
        let diagnostic = String::from_utf8_lossy(&diagnostic_bytes);
        return Err(SysError::ApiError(format!(
            "Anthropic API error ({}): {diagnostic}",
            stream.status()
        )));
    }

    let mut state = StreamState::default();
    let mut parser = SseParser::new();

    while let Some(chunk) = stream.read_chunk()? {
        parser.feed(&chunk, |_event_name, event| {
            handle_event(request_id, event, &mut state)
        })?;
    }

    // If the stream ended before `message_stop` we still need to close
    // out the consumer. Emit usage (if we collected any) followed by Done.
    if !state.done_emitted {
        emit_usage_if_any(request_id, &state)?;
        publish_stream(request_id, StreamEvent::Done)?;
    }
    Ok(())
    // `stream` drops here — kernel-side HTTP stream resource released.
}

/// Cross-event state held across SSE messages within one request.
#[derive(Default)]
struct StreamState {
    /// `id` of the currently open tool-use block (Anthropic interleaves at
    /// most one streaming tool block at a time; index disambiguates if
    /// they ever overlap).
    current_tool_id: String,
    /// Initial input tokens captured from `message_start.usage`.
    input_tokens: usize,
    /// Cumulative output tokens captured from `message_delta.usage`.
    output_tokens: usize,
    /// Set once we publish `StreamEvent::Done` so we don't double-emit
    /// on a stream that delivers both `message_stop` and EOF.
    done_emitted: bool,
}

fn handle_event(
    request_id: Uuid,
    event: StreamingEvent,
    state: &mut StreamState,
) -> Result<(), SysError> {
    match event {
        StreamingEvent::MessageStart { message } => {
            if let Some(usage) = message.get("usage") {
                if let Some(it) = usage.get("input_tokens").and_then(Value::as_u64) {
                    state.input_tokens = it as usize;
                }
                if let Some(ot) = usage.get("output_tokens").and_then(Value::as_u64) {
                    state.output_tokens = ot as usize;
                }
            }
        }
        StreamingEvent::ContentBlockStart { content_block, .. } => match content_block {
            ContentBlock::Text { .. } | ContentBlock::Thinking { .. } => {}
            ContentBlock::ToolUse { id, name, .. } => {
                state.current_tool_id = id.clone();
                publish_stream(request_id, StreamEvent::ToolCallStart { id, name })?;
            }
        },
        StreamingEvent::ContentBlockDelta { delta, .. } => match delta {
            Delta::TextDelta { text } => {
                if !text.is_empty() {
                    publish_stream(request_id, StreamEvent::TextDelta(text))?;
                }
            }
            Delta::InputJsonDelta { partial_json } => {
                if !state.current_tool_id.is_empty() {
                    publish_stream(
                        request_id,
                        StreamEvent::ToolCallDelta {
                            id: state.current_tool_id.clone(),
                            args_delta: partial_json,
                        },
                    )?;
                }
            }
            Delta::ThinkingDelta { .. } | Delta::SignatureDelta { .. } => {
                // Extended-thinking output: not surfaced via StreamEvent yet.
            }
        },
        StreamingEvent::ContentBlockStop { .. } => {
            if !state.current_tool_id.is_empty() {
                let id = std::mem::take(&mut state.current_tool_id);
                publish_stream(request_id, StreamEvent::ToolCallEnd { id })?;
            }
        }
        StreamingEvent::MessageDelta {
            usage:
                Some(AnthropicUsage {
                    input_tokens,
                    output_tokens,
                }),
            ..
        } => {
            // Anthropic's `message_delta.usage.output_tokens` is cumulative;
            // we overwrite, not accumulate. `input_tokens` is rarely present
            // here but we honour it if Anthropic ever emits one.
            if let Some(it) = input_tokens {
                state.input_tokens = it;
            }
            if let Some(ot) = output_tokens {
                state.output_tokens = ot;
            }
        }
        StreamingEvent::MessageDelta { usage: None, .. } => {}
        StreamingEvent::MessageStop => {
            emit_usage_if_any(request_id, state)?;
            publish_stream(request_id, StreamEvent::Done)?;
            state.done_emitted = true;
        }
        StreamingEvent::Ping => {}
        StreamingEvent::Error { error } => {
            publish_stream(
                request_id,
                StreamEvent::Error(format!("{}: {}", error.error_type, error.message)),
            )?;
            // Continue draining; Anthropic may follow `error` with `message_stop`.
        }
    }
    Ok(())
}

fn emit_usage_if_any(request_id: Uuid, state: &StreamState) -> Result<(), SysError> {
    if state.input_tokens > 0 || state.output_tokens > 0 {
        publish_stream(
            request_id,
            StreamEvent::Usage {
                input_tokens: state.input_tokens,
                output_tokens: state.output_tokens,
            },
        )?;
    }
    Ok(())
}

/// Publish one stream event on `llm.v1.stream.claude` keyed by `request_id`.
pub(crate) fn publish_stream(request_id: Uuid, event: StreamEvent) -> Result<(), SysError> {
    ipc::publish_json(
        STREAM_TOPIC,
        &IpcPayload::LlmStreamEvent { request_id, event },
    )
}

/// Resolve the effective system prompt: explicit `system` field wins;
/// otherwise the leading `MessageRole::System` text message is consumed
/// out of the conversation slice. Subsequent system messages stay in the
/// conversation and get folded into a `user` role on convert (Anthropic
/// has no inline system message).
fn split_system<'a>(system: &str, messages: &'a [Message]) -> (String, &'a [Message]) {
    if !system.is_empty() {
        return (system.to_string(), messages);
    }
    if let Some((first, rest)) = messages.split_first()
        && first.role == MessageRole::System
        && let MessageContent::Text(s) = &first.content
    {
        return (s.clone(), rest);
    }
    (String::new(), messages)
}

/// Convert an Astrid `Message` to the Anthropic content-block format.
fn convert_message(message: &Message) -> Value {
    match &message.content {
        MessageContent::Text(text) => {
            serde_json::json!({
                "role": match message.role {
                    MessageRole::Assistant => "assistant",
                    // System messages that survived `split_system` (i.e. mid-conversation
                    // system turns) get folded into `user` — Anthropic has no inline
                    // system role on messages.
                    MessageRole::User | MessageRole::Tool | MessageRole::System => "user",
                },
                "content": text,
            })
        }
        MessageContent::ToolCalls(calls) => {
            let content: Vec<Value> = calls
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "type": "tool_use",
                        "id": c.id,
                        "name": c.name,
                        "input": c.arguments,
                    })
                })
                .collect();
            serde_json::json!({
                "role": "assistant",
                "content": content,
            })
        }
        MessageContent::ToolResult(result) => {
            serde_json::json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": result.call_id,
                    "content": result.content,
                    "is_error": result.is_error,
                }],
            })
        }
        MessageContent::MultiPart(parts) => {
            let content: Vec<Value> = parts
                .iter()
                .map(|p| match p {
                    ContentPart::Text { text } => {
                        serde_json::json!({ "type": "text", "text": text })
                    }
                    ContentPart::Image { media_type, data } => {
                        serde_json::json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": media_type,
                                "data": data,
                            }
                        })
                    }
                })
                .collect();
            serde_json::json!({
                "role": match message.role {
                    MessageRole::Assistant => "assistant",
                    _ => "user",
                },
                "content": content,
            })
        }
    }
}
