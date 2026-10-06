//! Reusable workbench content, independent of terminal layout and navigation.
//!
//! `WorkspaceContent` adapts runtime state and Git data into resources with stable
//! targets. Any view can consume it; `HomeState` adds only selection and refresh.
mod content;
mod home;
mod recent;

pub use content::{ResourceItem, ResourceKind, ResourceTarget, WorkspaceContent};
pub use crate::harness::resources::{ChangeStats, GitChange, GitSnapshot};
pub use home::HomeState;
pub use recent::{RecentView, RecentViews};

#[cfg(test)]
mod tests;
