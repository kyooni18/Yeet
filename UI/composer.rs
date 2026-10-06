//! Application-owned composer controls, submission policy and interaction intent.
//! Native adapters keep editor cursors, IME, upload I/O and clipboard resources.
mod state;
#[cfg(test)]
mod tests;
mod types;
pub use state::{ComposerState, extension_command_invocation, suggestions};
pub use types::*;
