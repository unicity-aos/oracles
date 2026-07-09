//! Newtype wrappers for product wire strings.
//!
//! These exist so call sites cannot mix a principal family with a bus
//! namespace, or a log tag with an MCP prefix, without a type error.

use core::fmt;

macro_rules! str_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(pub &'static str);

        impl $name {
            /// Borrow the underlying static string.
            #[inline]
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                self.0
            }
        }

        impl AsRef<str> for $name {
            #[inline]
            fn as_ref(&self) -> &str {
                self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_tuple(stringify!($name)).field(&self.0).finish()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.0)
            }
        }
    };
}

str_newtype!(
    /// Distro id accepted by `astrid init --distro <id>`.
    DistroId
);
str_newtype!(
    /// Default principal family for the host plugin (`claude-code`, …).
    PrincipalFamily
);
str_newtype!(
    /// MCP server namespace segment used in `mcp__{ns}__*`.
    McpNamespace
);
str_newtype!(
    /// Full MCP tool name prefix including trailing `__` (e.g. `mcp__sage__`).
    McpToolPrefix
);
str_newtype!(
    /// Capsule package / component id (e.g. `sage-mcp`).
    CapsuleName
);
str_newtype!(
    /// Product-local bus namespace (`sage` in `sage.v1.*`).
    BusNamespace
);
str_newtype!(
    /// Human host name for docs/logs (`Claude`, `Grok`, `Codex`).
    HostDisplayName
);
str_newtype!(
    /// Static bus topic string.
    Topic
);
str_newtype!(
    /// Prefix for product audit topics (`sage.v1.audit.`).
    AuditTopicPrefix
);
str_newtype!(
    /// Log line tag (`sage-mcp`).
    LogTag
);
