pub mod agent;
pub mod backend;
pub mod background;
pub mod cli;
pub mod config;
pub mod context_cache;
pub mod core;
pub mod debate;
pub mod edit;
pub mod extensions;
mod foundation_backend;
pub mod general;
pub mod harness;
pub mod harness_ffi;
pub mod install;
pub mod mcp_server;
pub mod memory;
pub mod model;
mod orchestration;
pub mod permission;
mod platform;
pub mod project_settings;
pub mod remote;
pub mod sandbox;
pub mod sandbox_cli;
pub mod session_store;
pub mod shell;
pub mod skyline;
mod text_layout;
pub mod theme;
pub mod tools;
pub mod update;
pub mod web_search;
pub mod workbench;
pub mod workers;

#[path = "../tui/mod.rs"]
pub mod tui;

// Compatibility exports for callers migrating to the terminal subsystem.
pub use tui::{app, ui};
