//! Native ratatui UI facade. View implementation lives in `src/ui/`.
mod components;
mod dialogs;
mod render;
mod shell;
mod support;
mod task;
mod views;

pub use render::draw;
pub(crate) use support::theme::apply_runtime_theme;
pub use support::theme::initialize_theme;

#[cfg(test)]
mod render_tests;
