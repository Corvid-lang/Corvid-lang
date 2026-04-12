//! Effect classification for tools.
//!
//! Corvid v0.1 uses a simple two-class system:
//!
//!   * `Safe` — the default. No special handling.
//!   * `Dangerous` — marked with the `dangerous` keyword. The type checker
//!     requires a prior `approve` statement before any call to such a tool
//!     in the same block.
//!
//! Finer-grained classification (e.g. a `Compensable` variant for effects
//! that can be undone) may be added in later versions. Adding a variant is
//! a non-breaking extension.
//!
//! See `ARCHITECTURE.md` §5.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    /// No special treatment. The default for unannotated tools.
    Safe,

    /// Cannot be automatically undone. Requires a prior `approve` to call.
    Dangerous,
}
