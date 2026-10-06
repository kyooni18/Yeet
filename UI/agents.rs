//! Agent group application state, controls and content shared by native adapters.
//! Runtime commands are effects; platform input, geometry and focus handles stay outside UI.
mod projection;
mod state;
#[cfg(test)]
mod tests;
mod types;

pub use projection::{group_status, member_status, member_task, member_verb};
pub use types::*;
