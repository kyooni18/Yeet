//! Compatibility exports for UI actions and Harness commands.
//!
//! New callers use [`crate::shared_ui::actions`] for UI interaction and
//! [`crate::harness::HarnessCommand`] for runtime commands.
pub use crate::harness::HarnessCommand as FrontendCommand;
pub use crate::shared_ui::actions::{Action, NavigationAction};
