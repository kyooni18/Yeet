//! Reusable workbench resources, independent of terminal layout and navigation.
//! Shared Home selection and projection live in `UI/home`; resource refreshes
//! and filesystem/Git access remain in Harness.
mod content;
mod recent;

pub use crate::harness::resources::{ChangeStats, GitChange, GitSnapshot};
pub use content::{ResourceItem, ResourceKind, ResourceTarget, WorkspaceContent};
pub use recent::{RecentView, RecentViews};

#[cfg(test)]
mod tests;
