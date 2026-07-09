//! Interaction mode shared by host runners and provisioners.

use serde::{Deserialize, Serialize};

/// How the user drives the external coding host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionMode {
    /// Astrid supervises the host binary (headless / bounded turns).
    #[default]
    Headless,
    /// User drives the host UI directly; runner refuses supervised spawn.
    Repl,
}
