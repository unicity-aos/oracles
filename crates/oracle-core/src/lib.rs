//! Shared product identity for Astrid-governed oracles.
//!
//! Three products live in one monorepo:
//! - **Sage** — Claude Code
//! - **Mimir** — Grok Build
//! - **Sibyl** — Codex
//!
//! The MCP broker is shared. Host plugins differ (hooks, install paths,
//! principal families). All wire-facing strings are derived from a single
//! [`ProductProfile`] so product crates cannot drift string-by-string.

#![deny(unsafe_code)]
#![deny(clippy::all)]
#![deny(unreachable_pub)]
#![warn(missing_docs)]

mod newtypes;
mod product;

pub use newtypes::{
    AuditTopicPrefix, BusNamespace, CapsuleName, DistroId, HostDisplayName, LogTag, McpNamespace,
    McpToolPrefix, PrincipalFamily, Topic,
};
pub use product::{Product, ProductProfile};
