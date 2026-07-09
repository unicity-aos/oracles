//! mimir-mcp — thin product capsule over the shared agent broker.

#![deny(unsafe_code)]
#![deny(clippy::all)]

use oracle_broker::{handlers, ProductProfile};
use astrid_sdk::prelude::*;

/// mimir MCP broker capsule.
#[derive(Default)]
pub struct MimirMcp;

fn ensure_product() {
    oracle_broker::install(&ProductProfile::MIMIR);
}

#[capsule]
impl MimirMcp {
    /// Product-local tools.describe.
    #[astrid::interceptor("describe_tools")]
    pub fn describe_tools(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_product();
        handlers::describe_tools(payload)
    }

    /// tool.v1.response.describe.*
    #[astrid::interceptor("collect_tool_descriptors")]
    pub fn collect_tool_descriptors(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_product();
        handlers::collect_tool_descriptors(payload)
    }

    /// astrid.v1.capsules_loaded
    #[astrid::interceptor("handle_capsules_changed")]
    pub fn handle_capsules_changed(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_product();
        handlers::handle_capsules_changed(payload)
    }

    /// astrid.v1.request.mcp.tools.list
    #[astrid::interceptor("handle_mcp_list")]
    pub fn handle_mcp_list(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_product();
        handlers::handle_mcp_list(payload)
    }

    /// astrid.v1.request.mcp.tool.call
    #[astrid::interceptor("handle_mcp_call")]
    pub fn handle_mcp_call(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_product();
        handlers::handle_mcp_call(payload)
    }

    /// astrid.v1.request.mcp.approval.respond
    #[astrid::interceptor("handle_mcp_approval")]
    pub fn handle_mcp_approval(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_product();
        handlers::handle_mcp_approval(payload)
    }

    /// astrid.v1.request.mcp.ingress.respond
    #[astrid::interceptor("handle_mcp_ingress_respond")]
    pub fn handle_mcp_ingress_respond(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_product();
        handlers::handle_mcp_ingress_respond(payload)
    }

    /// astrid.v1.request.mcp.grant.respond
    #[astrid::interceptor("handle_mcp_grant_respond")]
    pub fn handle_mcp_grant_respond(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_product();
        handlers::handle_mcp_grant_respond(payload)
    }

    /// hook.v1.event.before_tool_call
    #[astrid::interceptor("handle_before_tool_call")]
    pub fn handle_before_tool_call(&self, payload: serde_json::Value) -> Result<(), SysError> {
        ensure_product();
        handlers::handle_before_tool_call(payload)
    }
}
