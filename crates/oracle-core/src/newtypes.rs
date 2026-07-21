//! Newtype wrappers for wire strings.

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
    /// Additive AOS oracle-pack id (`claude`, `grok`, `codex`).
    PackId
);
str_newtype!(
    /// Default principal family for a host plugin (`claude-code`, …).
    PrincipalFamily
);
str_newtype!(
    /// Human host product name (`Claude Code`, `Grok Build`, `Codex`).
    HostDisplayName
);
