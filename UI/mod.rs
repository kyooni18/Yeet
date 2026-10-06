//! Platform-independent application structure and interaction.
//!
//! UI owns semantic views, navigation, actions and application UI state. It
//! depends on Harness commands and state; platform adapters translate these
//! concepts into native visuals and input without choosing application structure.
//! Terminal rectangles, DOM nodes and native framework types stay in Platforms.
//! The `.ui` design documents remain design specifications, not executable input.

pub mod actions;
pub mod agents;
pub mod agents_session;
pub mod application;
pub mod navigation;
pub mod shell;
pub mod surfaces;
pub mod workbench;

pub use actions::{Action, NavigationAction};
