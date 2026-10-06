//! Transcript structure, disclosure state and actions authored once for every host.
pub mod compact;
mod projection;
mod state;
#[cfg(test)]
mod tests;
mod tools;
mod types;
pub use projection::{should_display_activity, should_display_reasoning};
pub use state::ConversationState;
pub use tools::project_tool;
pub use types::*;
