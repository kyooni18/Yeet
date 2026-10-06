//! Terminal frontend: state, interaction, rendering, and terminal lifecycle.
//!
//! Backend services and shared resource models remain outside this subsystem.
//! The legacy `crate::app` and `crate::ui` exports are compatibility aliases.
pub mod app;
pub mod kit;
mod runtime;
pub mod ui;

pub use app::App;
pub use runtime::{run, run_remote};
