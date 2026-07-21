//! Shared identity for Unicity AOS oracles.
//!
//! An **oracle** is an external coding runtime (Claude Code, Grok Build, Codex)
//! bound into Unicity AOS. This crate contains only host identity; AOS CE owns
//! the shared broker, interaction policy, and product capsule.

#![deny(unsafe_code)]
#![deny(clippy::all)]
#![deny(unreachable_pub)]
#![warn(missing_docs)]

mod host;
mod newtypes;

pub use host::{Host, HostProfile};
pub use newtypes::{HostDisplayName, PackId, PrincipalFamily};
