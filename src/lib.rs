pub mod actions;
// Compatibility import; execution ownership lives in Harness.
pub use harness::agent;
pub use harness::agents;
pub mod backend;
pub mod background;
pub mod cli;
pub mod config;
// Compatibility import; prompt cache planning lives in Harness.
pub use harness::context_cache;
// Compatibility import while runtime callers migrate to Harness ownership.
pub use harness::core;
pub mod debate;
pub mod edit;
pub mod extensions;
mod foundation_backend;
pub mod general;
#[path = "../Harness/mod.rs"]
pub mod harness;
pub mod harness_ffi;
pub mod install;
pub mod mcp_server;
pub mod memory;
// Compatibility import; runtime state and event schema belongs to Harness.
pub use harness::model;
// Compatibility import; runtime approval arbitration lives in Harness.
pub use harness::permission;
mod platform;
pub mod project_settings;
pub mod remote;
pub mod sandbox;
pub mod sandbox_cli;
// Compatibility import; persistent session ownership lives in Harness.
pub use harness::session_store;
pub mod shell;
pub mod skyline;
mod text_layout;
pub mod theme;
// Compatibility import; tool execution lives in Harness.
pub use harness::tools;
pub mod update;
pub mod web_search;
pub mod workbench;
// Compatibility import; runtime workers live in Harness.
pub use harness::workers;

#[path = "../UI/mod.rs"]
pub mod shared_ui;

#[path = "../tui/mod.rs"]
pub mod tui;

// Compatibility exports for callers migrating to the terminal subsystem.
pub use tui::{app, ui};
