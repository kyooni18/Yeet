//! Compatibility exports for agent-owned actions.
//!
//! New callers should use [`crate::agents::actions`]. Skyline is a separate
//! capability and does not own these intents or their frontend dispatch.
pub use crate::agents::actions::{Action, FrontendCommand, NavigationAction};
