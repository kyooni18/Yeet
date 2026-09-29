//! Native ratatui UI facade. View implementation lives in `src/ui/`.
mod chrome;
mod composer;
mod dialogs;
mod files;
mod markdown;
mod render;
mod responsive;
mod sessions;
mod shell;
mod status;
mod task;
mod text;
mod theme;
mod yeet_brand;

pub use render::draw;
pub(crate) use theme::apply_runtime_theme;
pub use theme::initialize_theme;

#[cfg(test)]
mod render_tests;
