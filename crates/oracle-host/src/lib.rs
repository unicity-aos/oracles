//! Shared host-adapter layer for Astrid oracles.
//!
//! Hosts (Claude Code, Grok Build, Codex) are **not** separate backends.
//! They implement thin adapters over this crate:
//!
//! - [`ids`] — `PrincipalId` / `SessionId` NewTypes + charset validation
//! - [`fs`] — atomic VFS writes
//! - [`install`] — provision/relink loop via [`install::HostProvisioner`]
//! - [`mode`] — shared `InteractionMode`
//! - [`topics`] — host-scoped bus topic builders
//!
//! The MCP broker lives in AOS CE. This crate is everything at the host edge
//! that an Oracle plugin still needs.

#![deny(unsafe_code)]
#![deny(clippy::all)]
#![deny(unreachable_pub)]
#![warn(missing_docs)]

pub mod fs;
pub mod ids;
pub mod install;
pub mod mode;
pub mod topics;

pub use ids::{MAX_ID_LEN, PrincipalId, SessionId};
pub use install::{
    HostProvisioner, InstallComplete, InstallMarker, InstallRequest, InstallStatus, RelinkRequest,
    publish_complete, publish_status, run_install, run_relink,
};
pub use mode::InteractionMode;
pub use topics::HostTopics;
