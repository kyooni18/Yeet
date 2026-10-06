//! Harness-owned filesystem and Git resources. Application UI supplies selection
//! and display intent; this module performs retrieval without presentation state.
mod files;
mod git;
mod review;
mod refresh;

pub use files::{
    FileEntry, count_text_lines, directory_entries, git_changes, git_diff, git_identity,
    git_numstat,
};
pub use git::{ChangeStats, GitChange, GitSnapshot};
pub use review::{repository_root, review_patch};

pub use refresh::GitRefresh;
