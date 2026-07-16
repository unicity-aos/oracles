//! Shared identity for Unicity AOS oracles.
//!
//! An **oracle** is an external coding runtime (Claude Code, Grok Build, Codex)
//! bound into Unicity AOS. AOS presents one MCP namespace over the neutral
//! Astrid broker capsule. Hosts differ only where the host product forces it
//! (plugin hooks, principal family, optional supervisor).
//!
//! The host adapters are AOS product components; published Astrid ABI, topic,
//! capsule and crate identifiers remain unchanged underneath them.

#![deny(unsafe_code)]
#![deny(clippy::all)]
#![deny(unreachable_pub)]
#![warn(missing_docs)]

mod host;
mod identity;
mod newtypes;

pub use host::{Host, HostProfile};
pub use identity::OracleIdentity;
pub use newtypes::{
    AuditTopicPrefix, CapsuleName, HostDisplayName, LogTag, McpNamespace, McpToolPrefix, PackId,
    PrincipalFamily, Topic,
};
