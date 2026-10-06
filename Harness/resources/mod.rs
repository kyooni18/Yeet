//! Harness-owned filesystem and Git resources. Application UI supplies selection
//! and display intent; this module performs retrieval without presentation state.
mod files;
mod git;
mod refresh;
mod review;

pub use files::{
    FileEntry, FileMetadata, canonical_resource_path, count_text_lines, directory_entries,
    directory_entry_count, file_metadata, git_changes, git_diff, git_identity, git_numstat,
    path_is_dir, path_is_file,
};
pub use git::{ChangeStats, GitChange, GitSnapshot};
pub use review::{repository_root, review_patch};

pub use refresh::GitRefresh;
