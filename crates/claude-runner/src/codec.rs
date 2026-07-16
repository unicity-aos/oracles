//! Stream-json wire codec for `claude -p --input-format stream-json
//! --output-format stream-json`.
//!
//! Pure encode/decode of the (undocumented but observed) NDJSON grammar
//! Claude Code speaks. No IPC, no host calls — fully unit-testable.
//!
//! ## Wire shape
//!
//! Every message is one JSON object on its own line, terminated by `\n`.
//! Writers must flush after each line.
//!
//! Outbound (host -> claude stdin) is restricted to two kinds of frame
//! emitted by [`encode`]: user turns and control responses. Inbound
//! (claude stdout -> host) covers the richer set in [`Decoded`]; unknown
//! `type` values map to [`Decoded::Unknown`] so newer CLI versions don't
//! tear down the supervisor.
//!
//! ## The `mcp_response` wrapper
//!
//! When replying to an SDK-side MCP tool call via `control_response`, the
//! payload MUST be wrapped as `{"mcp_response":{"content":[...],
//! "isError":...}}`. Omitting the wrapper triggers a 60 s CLI timeout —
//! a protocol constraint the codec preserves.

use serde_json::{Value, json};

/// Hard ceiling on the in-progress line buffer. Claude's stream-json lines
/// are typically a few hundred bytes; this generous cap catches a runaway
/// producer without a `String` allocation panic.
const MAX_LINE_BUFFER_SIZE: usize = 64 * 1024;

/// Codec-level error. Distinct from `SysError` so callers can route
/// "buffer overrun" / "malformed line" differently from host errors.
#[derive(Debug)]
pub(crate) enum CodecError {
    /// Inbound buffer grew past [`MAX_LINE_BUFFER_SIZE`] without a newline.
    LineTooLong,
    /// A complete line failed JSON parse or shape validation.
    Malformed(String),
}

impl core::fmt::Display for CodecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::LineTooLong => {
                write!(f, "stream-json line exceeded {MAX_LINE_BUFFER_SIZE} bytes")
            }
            Self::Malformed(msg) => write!(f, "malformed stream-json line: {msg}"),
        }
    }
}

// ---------- encode ----------

/// An outbound frame from host -> claude stdin.
///
/// The runner only writes user turns now; tool results no longer flow
/// through here. Tool execution is owned by the registered `aos mcp
/// serve` MCP server (Claude calls it directly), so the inline
/// tool-result / control-response write-back is gone.
pub(crate) enum Outbound<'a> {
    /// A user-turn message. Encoded as
    /// `{"type":"user","message":{"role":"user","content":[{"type":"text","text":...}]}}`.
    UserTurn { text: &'a str },
}

/// Encode one outbound frame into a single newline-terminated JSON line.
pub(crate) fn encode(frame: &Outbound<'_>) -> String {
    let value = match frame {
        Outbound::UserTurn { text } => json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{ "type": "text", "text": text }],
            },
        }),
    };

    // `to_string` on an owned `Value` is infallible (no Display impl
    // anywhere in the tree returns Err); the `?` is unnecessary but
    // `unwrap_or_default` keeps us panic-free for the static-analysis
    // crowd. Resulting empty string would simply be skipped by the
    // CLI; not a correctness concern.
    let mut line = serde_json::to_string(&value).unwrap_or_default();
    line.push('\n');
    line
}

// ---------- decode ----------

/// One assistant-message content block, structurally split from Claude's
/// raw `assistant.message.content[]` array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AssistantBlock {
    /// A `{type:"text",text:...}` block. `tool_use` blocks are dropped at
    /// decode: claude executes `mcp__aos__*` tools directly against the
    /// registered `aos mcp serve` MCP server, so the runner never sees or
    /// dispatches them (the only assistant content the runner relays is text).
    Text { text: String },
}

/// One decoded inbound frame.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Decoded {
    /// `{type:"system",subtype:"init",session_id,model,...}`.
    SystemInit { session_id: String, model: String },
    /// `{type:"assistant",message:{content:[...]}}`. Each call carries
    /// the full content array as separate typed blocks (text only).
    Assistant { content_blocks: Vec<AssistantBlock> },
    /// `{type:"user",...tool_result...}` — the CLI's own echo of a
    /// tool_result it injected back into the conversation. Forward-only
    /// observability hook; supervisor doesn't usually need to act.
    UserToolResultEcho { tool_use_id: String },
    /// `{type:"sdk_control_request"|"control_request",request:{...}}`.
    /// Subtype routes to permission gate vs. MCP call.
    ControlRequest {
        request_id: String,
        subtype: String,
        payload: Value,
    },
    /// `{type:"result",subtype,is_error,...}` — end-of-turn terminator.
    Result {
        subtype: String,
        is_error: bool,
        usage: Option<Value>,
        total_cost_usd: Option<f64>,
        permission_denials: Vec<Value>,
    },
    /// Raw `{type:"stream_event",event}` for events the codec does not
    /// destructure into a typed shape (message_start, message_delta,
    /// text_delta, content_block_start for non-tool blocks, etc.).
    StreamEvent { event: Value },
    /// `{type:"ping"}` keepalive.
    Ping,
    /// Any other top-level `type`. Supervisor logs at warn and continues
    /// (forward-compat).
    Unknown(Value),
}

/// Stateful NDJSON decoder. Accumulates partial chunks into a line
/// buffer, yields completed lines as typed [`Decoded`] events.
///
/// The buffer is a `Vec<u8>` rather than a `String` so multi-byte UTF-8
/// characters that straddle chunk boundaries survive intact — we only
/// lossy-decode once a complete line has been assembled, mirroring the
/// SSE parser in `claude-completion`.
#[derive(Debug, Default)]
pub(crate) struct LineDecoder {
    buf: Vec<u8>,
}

impl LineDecoder {
    /// Empty decoder.
    pub(crate) fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Feed a chunk of bytes. Scans for `\n` at the byte level so
    /// UTF-8 code points split across chunk boundaries reassemble
    /// without being replaced by U+FFFD. Returns an iterator over
    /// completed lines.
    pub(crate) fn feed(
        &mut self,
        chunk: &[u8],
    ) -> impl Iterator<Item = Result<Decoded, CodecError>> + '_ {
        self.buf.extend_from_slice(chunk);

        let overrun = self.buf.len() > MAX_LINE_BUFFER_SIZE && !self.buf.contains(&b'\n');

        let mut completed: Vec<Result<Decoded, CodecError>> = Vec::new();

        if overrun {
            // Drop buffer so subsequent feeds don't keep tripping.
            self.buf.clear();
            completed.push(Err(CodecError::LineTooLong));
        } else {
            while let Some(newline_pos) = self.buf.iter().position(|b| *b == b'\n') {
                // Drain the line including the trailing `\n`, then pop it.
                let mut line_bytes: Vec<u8> = self.buf.drain(..=newline_pos).collect();
                line_bytes.pop();
                // Strip a single trailing `\r` (CRLF normalisation).
                if line_bytes.last() == Some(&b'\r') {
                    line_bytes.pop();
                }

                if line_bytes.is_empty() {
                    continue;
                }

                // Decode once now that the line is complete. Any
                // U+FFFD here represents a genuinely malformed byte
                // inside a finished line — never a boundary artefact.
                let line = String::from_utf8_lossy(&line_bytes);
                completed.push(decode_line(&line));
            }
        }

        completed.into_iter()
    }
}

/// Decode one already-line-bounded JSON string.
fn decode_line(line: &str) -> Result<Decoded, CodecError> {
    let value: Value =
        serde_json::from_str(line).map_err(|e| CodecError::Malformed(e.to_string()))?;

    let ty = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| CodecError::Malformed("missing 'type' field".into()))?;

    match ty {
        "system" => decode_system(&value),
        "assistant" => decode_assistant(&value),
        "user" => Ok(decode_user(&value)),
        "stream_event" => Ok(decode_stream_event(&value)),
        "control_request" | "sdk_control_request" => Ok(decode_control_request(&value)),
        "result" => Ok(decode_result(&value)),
        "ping" => Ok(Decoded::Ping),
        _ => Ok(Decoded::Unknown(value)),
    }
}

fn decode_system(value: &Value) -> Result<Decoded, CodecError> {
    // Only `subtype:"init"` is structurally split — others fall through
    // to Unknown so we don't lose them.
    if value.get("subtype").and_then(Value::as_str) != Some("init") {
        return Ok(Decoded::Unknown(value.clone()));
    }
    let session_id = value
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let model = value
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok(Decoded::SystemInit { session_id, model })
}

fn decode_assistant(value: &Value) -> Result<Decoded, CodecError> {
    let content = value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .ok_or_else(|| CodecError::Malformed("assistant missing message.content[]".into()))?;

    let mut blocks = Vec::with_capacity(content.len());
    for block in content {
        let bty = block.get("type").and_then(Value::as_str).unwrap_or("");
        // Only `text` blocks are relayed. `tool_use` blocks are
        // intentionally dropped: claude executes them against the
        // registered MCP server, so the runner never sees or dispatches them.
        // Thinking and any other block types ride through via StreamEvent
        // when --include-partial-messages is on.
        if bty == "text" {
            let text = block
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            blocks.push(AssistantBlock::Text { text });
        }
    }
    Ok(Decoded::Assistant {
        content_blocks: blocks,
    })
}

fn decode_user(value: &Value) -> Decoded {
    // Only path we care about structurally is the CLI's echo of a
    // tool_result it injected. Everything else (raw user turns echoed
    // back) is forwarded as Unknown.
    let tool_use_id = value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .and_then(|arr| {
            arr.iter().find_map(|b| {
                if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                    b.get("tool_use_id")
                        .and_then(Value::as_str)
                        .map(String::from)
                } else {
                    None
                }
            })
        });

    match tool_use_id {
        Some(id) => Decoded::UserToolResultEcho { tool_use_id: id },
        None => Decoded::Unknown(value.clone()),
    }
}

fn decode_stream_event(value: &Value) -> Decoded {
    // Token-level events are passed through verbatim as StreamEvent (the
    // supervisor relays them on `.partial`). tool_use delta/stop framing is
    // no longer special-cased — tool calls execute on the registered MCP
    // server, off this stream.
    match value.get("event") {
        Some(event) => Decoded::StreamEvent {
            event: event.clone(),
        },
        None => Decoded::Unknown(value.clone()),
    }
}

fn decode_control_request(value: &Value) -> Decoded {
    let request = value.get("request").cloned().unwrap_or(Value::Null);
    let request_id = request
        .get("request_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let subtype = request
        .get("subtype")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Decoded::ControlRequest {
        request_id,
        subtype,
        payload: request,
    }
}

fn decode_result(value: &Value) -> Decoded {
    let subtype = value
        .get("subtype")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let is_error = value
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let usage = value.get("usage").cloned();
    let total_cost_usd = value.get("total_cost_usd").and_then(Value::as_f64);
    let permission_denials = value
        .get("permission_denials")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Decoded::Result {
        subtype,
        is_error,
        usage,
        total_cost_usd,
        permission_denials,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- encode ----

    #[test]
    fn encode_user_turn_shape() {
        let line = encode(&Outbound::UserTurn { text: "Hello" });
        assert!(line.ends_with('\n'));
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["role"], "user");
        assert_eq!(v["message"]["content"][0]["type"], "text");
        assert_eq!(v["message"]["content"][0]["text"], "Hello");
    }

    // ---- decode: framing ----

    #[test]
    fn decode_handles_split_chunks() {
        let mut dec = LineDecoder::new();
        let line = r#"{"type":"ping"}"#;
        let (a, b) = line.split_at(5);
        let r1: Vec<_> = dec.feed(a.as_bytes()).collect();
        assert!(r1.is_empty());
        let with_nl = format!("{b}\n");
        let r2: Vec<_> = dec.feed(with_nl.as_bytes()).collect();
        assert_eq!(r2.len(), 1);
        assert!(matches!(r2[0], Ok(Decoded::Ping)));
    }

    #[test]
    fn decode_multiple_lines_one_feed() {
        let mut dec = LineDecoder::new();
        let blob = "{\"type\":\"ping\"}\n{\"type\":\"ping\"}\n";
        let r: Vec<_> = dec.feed(blob.as_bytes()).collect();
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn decode_survives_multibyte_codepoint_split_across_chunks() {
        // The euro sign U+20AC encodes as the three bytes E2 82 AC.
        // Splitting it mid-codepoint must NOT turn into U+FFFD —
        // the buffer is byte-oriented and only lossy-decodes
        // complete lines.
        let mut dec = LineDecoder::new();
        let payload = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"price: \u{20ac}5\"}]}}\n";
        let bytes = payload.as_bytes();
        // Find the euro's first byte (0xE2) and split between byte 1
        // and byte 2 of its 3-byte sequence.
        let euro_start = bytes
            .iter()
            .position(|b| *b == 0xE2)
            .expect("euro byte missing");
        let split = euro_start + 1;

        let r1: Vec<_> = dec.feed(&bytes[..split]).collect();
        assert!(r1.is_empty(), "no complete line yet");

        let r2: Vec<_> = dec.feed(&bytes[split..]).collect();
        assert_eq!(r2.len(), 1);
        match &r2[0] {
            Ok(Decoded::Assistant { content_blocks }) => {
                let AssistantBlock::Text { text } = &content_blocks[0];
                assert_eq!(text, "price: \u{20ac}5");
                assert!(
                    !text.contains('\u{FFFD}'),
                    "boundary split corrupted into U+FFFD"
                );
            }
            other => panic!("expected Assistant, got {other:?}"),
        }
    }

    #[test]
    fn decode_strips_cr() {
        let mut dec = LineDecoder::new();
        let blob = "{\"type\":\"ping\"}\r\n";
        let r: Vec<_> = dec.feed(blob.as_bytes()).collect();
        assert!(matches!(r[0], Ok(Decoded::Ping)));
    }

    #[test]
    fn decode_skips_blank_lines() {
        let mut dec = LineDecoder::new();
        let blob = "\n\n{\"type\":\"ping\"}\n\n";
        let r: Vec<_> = dec.feed(blob.as_bytes()).collect();
        assert_eq!(r.len(), 1);
        assert!(matches!(r[0], Ok(Decoded::Ping)));
    }

    #[test]
    fn decode_line_overrun_yields_error_no_panic() {
        let mut dec = LineDecoder::new();
        let huge = "a".repeat(MAX_LINE_BUFFER_SIZE + 16);
        let r: Vec<_> = dec.feed(huge.as_bytes()).collect();
        assert_eq!(r.len(), 1);
        assert!(matches!(r[0], Err(CodecError::LineTooLong)));
        // Buffer was cleared — next feed of a real line works.
        let r2: Vec<_> = dec.feed(b"{\"type\":\"ping\"}\n").collect();
        assert!(matches!(r2[0], Ok(Decoded::Ping)));
    }

    #[test]
    fn decode_malformed_json_returns_error_not_panic() {
        let mut dec = LineDecoder::new();
        let r: Vec<_> = dec.feed(b"not-json\n").collect();
        assert!(matches!(r[0], Err(CodecError::Malformed(_))));
    }

    // ---- decode: typed variants ----

    #[test]
    fn decode_system_init() {
        let mut dec = LineDecoder::new();
        let line =
            r#"{"type":"system","subtype":"init","session_id":"sid","model":"claude-sonnet-4-6"}"#;
        let r: Vec<_> = dec.feed(format!("{line}\n").as_bytes()).collect();
        match &r[0] {
            Ok(Decoded::SystemInit { session_id, model }) => {
                assert_eq!(session_id, "sid");
                assert_eq!(model, "claude-sonnet-4-6");
            }
            other => panic!("expected SystemInit, got {other:?}"),
        }
    }

    #[test]
    fn decode_system_other_subtype_is_unknown() {
        let mut dec = LineDecoder::new();
        let line = r#"{"type":"system","subtype":"limit"}"#;
        let r: Vec<_> = dec.feed(format!("{line}\n").as_bytes()).collect();
        assert!(matches!(r[0], Ok(Decoded::Unknown(_))));
    }

    #[test]
    fn decode_assistant_text_block() {
        let mut dec = LineDecoder::new();
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#;
        let r: Vec<_> = dec.feed(format!("{line}\n").as_bytes()).collect();
        match &r[0] {
            Ok(Decoded::Assistant { content_blocks }) => {
                assert_eq!(content_blocks.len(), 1);
                let AssistantBlock::Text { text } = &content_blocks[0];
                assert_eq!(text, "hi");
            }
            other => panic!("expected Assistant, got {other:?}"),
        }
    }

    #[test]
    fn decode_non_tool_content_block_stop_is_stream_event() {
        let mut dec = LineDecoder::new();
        let line = r#"{"type":"stream_event","event":{"type":"content_block_stop","index":0}}"#;
        let r: Vec<_> = dec.feed(format!("{line}\n").as_bytes()).collect();
        assert!(matches!(r[0], Ok(Decoded::StreamEvent { .. })));
    }

    #[test]
    fn decode_text_delta_passes_through_as_stream_event() {
        let mut dec = LineDecoder::new();
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"H"}}}"#;
        let r: Vec<_> = dec.feed(format!("{line}\n").as_bytes()).collect();
        assert!(matches!(r[0], Ok(Decoded::StreamEvent { .. })));
    }

    #[test]
    fn decode_user_tool_result_echo() {
        let mut dec = LineDecoder::new();
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"ok"}]}}"#;
        let r: Vec<_> = dec.feed(format!("{line}\n").as_bytes()).collect();
        match &r[0] {
            Ok(Decoded::UserToolResultEcho { tool_use_id }) => {
                assert_eq!(tool_use_id, "toolu_1");
            }
            other => panic!("expected UserToolResultEcho, got {other:?}"),
        }
    }

    #[test]
    fn decode_user_without_tool_result_is_unknown() {
        let mut dec = LineDecoder::new();
        let line =
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#;
        let r: Vec<_> = dec.feed(format!("{line}\n").as_bytes()).collect();
        assert!(matches!(r[0], Ok(Decoded::Unknown(_))));
    }

    #[test]
    fn decode_control_request() {
        let mut dec = LineDecoder::new();
        let line = r#"{"type":"sdk_control_request","request":{"subtype":"mcp_message","request_id":"mcp_1","server_name":"aos"}}"#;
        let r: Vec<_> = dec.feed(format!("{line}\n").as_bytes()).collect();
        match &r[0] {
            Ok(Decoded::ControlRequest {
                request_id,
                subtype,
                payload,
            }) => {
                assert_eq!(request_id, "mcp_1");
                assert_eq!(subtype, "mcp_message");
                assert_eq!(payload["server_name"], "aos");
            }
            other => panic!("expected ControlRequest, got {other:?}"),
        }
    }

    #[test]
    fn decode_result_terminator() {
        let mut dec = LineDecoder::new();
        let line = r#"{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.02,"usage":{"input_tokens":1},"permission_denials":[]}"#;
        let r: Vec<_> = dec.feed(format!("{line}\n").as_bytes()).collect();
        match &r[0] {
            Ok(Decoded::Result {
                subtype,
                is_error,
                usage,
                total_cost_usd,
                permission_denials,
            }) => {
                assert_eq!(subtype, "success");
                assert!(!*is_error);
                assert_eq!(total_cost_usd, &Some(0.02));
                assert!(usage.is_some());
                assert!(permission_denials.is_empty());
            }
            other => panic!("expected Result, got {other:?}"),
        }
    }

    #[test]
    fn decode_unknown_type_is_forward_compat() {
        let mut dec = LineDecoder::new();
        let line = r#"{"type":"new_thing_anthropic_added","foo":1}"#;
        let r: Vec<_> = dec.feed(format!("{line}\n").as_bytes()).collect();
        assert!(matches!(r[0], Ok(Decoded::Unknown(_))));
    }

    #[test]
    fn decode_missing_type_is_malformed() {
        let mut dec = LineDecoder::new();
        let line = r#"{"foo":1}"#;
        let r: Vec<_> = dec.feed(format!("{line}\n").as_bytes()).collect();
        assert!(matches!(r[0], Err(CodecError::Malformed(_))));
    }
}
