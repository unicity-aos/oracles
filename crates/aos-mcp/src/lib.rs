//! aos-mcp — the neutral runtime capsule behind the Unicity AOS MCP surface.
//!
//! One capsule for every host (Claude Code, Grok Build, Codex). Host plugins
//! only change principal + hooks; they all talk to this broker.

#![deny(unsafe_code)]
#![deny(clippy::all)]

use astrid_sdk::prelude::*;
use oracle_broker::handlers;

mod host_hooks;

/// Unicity AOS MCP broker capsule over the Astrid runtime wire.
#[derive(Default)]
pub struct AosMcp;

fn ensure_identity() {
    oracle_broker::install_aos();
}

#[capsule]
impl AosMcp {
    /// Token-validated hook ingress from the Codex plugin.
    #[astrid::interceptor("handle_codex_hook")]
    pub fn handle_codex_hook(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_identity();
        host_hooks::handle("codex", payload)
    }

    /// Token-validated hook ingress from the Claude plugin.
    #[astrid::interceptor("handle_claude_hook")]
    pub fn handle_claude_hook(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_identity();
        host_hooks::handle("claude", payload)
    }

    /// Token-validated hook ingress from the Grok plugin.
    #[astrid::interceptor("handle_grok_hook")]
    pub fn handle_grok_hook(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_identity();
        host_hooks::handle("grok", payload)
    }

    /// Relay a validated bridge response to the exact authenticated host call.
    #[astrid::interceptor("relay_host_hook_response")]
    pub fn relay_host_hook_response(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_identity();
        host_hooks::relay_response(payload)
    }

    /// astrid.v1.tools.describe
    #[astrid::interceptor("describe_tools")]
    pub fn describe_tools(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_identity();
        handlers::describe_tools(payload)
    }

    /// tool.v1.response.describe.*
    #[astrid::interceptor("collect_tool_descriptors")]
    pub fn collect_tool_descriptors(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_identity();
        handlers::collect_tool_descriptors(payload)
    }

    /// astrid.v1.capsules_loaded
    #[astrid::interceptor("handle_capsules_changed")]
    pub fn handle_capsules_changed(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_identity();
        handlers::handle_capsules_changed(payload)
    }

    /// astrid.v1.request.mcp.tools.list
    #[astrid::interceptor("handle_mcp_list")]
    pub fn handle_mcp_list(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_identity();
        handlers::handle_mcp_list(payload)
    }

    /// astrid.v1.request.mcp.tool.call
    #[astrid::interceptor("handle_mcp_call")]
    pub fn handle_mcp_call(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_identity();
        handlers::handle_mcp_call(payload)
    }

    /// astrid.v1.request.mcp.approval.respond
    #[astrid::interceptor("handle_mcp_approval")]
    pub fn handle_mcp_approval(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_identity();
        handlers::handle_mcp_approval(payload)
    }

    /// astrid.v1.request.mcp.ingress.respond
    #[astrid::interceptor("handle_mcp_ingress_respond")]
    pub fn handle_mcp_ingress_respond(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_identity();
        handlers::handle_mcp_ingress_respond(payload)
    }

    /// astrid.v1.request.mcp.grant.respond
    #[astrid::interceptor("handle_mcp_grant_respond")]
    pub fn handle_mcp_grant_respond(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_identity();
        handlers::handle_mcp_grant_respond(payload)
    }

    /// hook.v1.event.before_tool_call
    #[astrid::interceptor("handle_before_tool_call")]
    pub fn handle_before_tool_call(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_identity();
        handlers::handle_before_tool_call(payload)
    }
}
