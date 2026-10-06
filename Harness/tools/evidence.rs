//! Owns model-visible tool evidence and task-scoped source provenance.

use std::collections::{HashMap, HashSet};

use crate::core::Usage;

use super::ReadCacheEntry;

#[derive(Debug, Clone)]
pub(super) struct MutationValidation {
    pub(super) error_count: usize,
    pub(super) changed_paths: Vec<String>,
}

impl MutationValidation {
    pub(super) fn write_validation_passed(&self) -> bool {
        self.error_count == 0
    }
}

#[derive(Debug, Clone)]
pub(super) struct EditReadCoverage {
    pub(super) snapshot: String,
    pub(super) ranges: Vec<(usize, usize)>,
}

#[derive(Default)]
pub(super) struct ToolEvidence {
    pub(super) read_cache: HashMap<String, Vec<ReadCacheEntry>>,
    pub(super) edit_snapshots: HashMap<String, String>,
    pub(super) edit_read_coverage: HashMap<String, EditReadCoverage>,
    pub(super) searches: HashSet<String>,
    pub(super) listings: HashSet<String>,
    pub(super) shell_inspections: HashSet<String>,
    pub(super) auxiliary_usage: Option<Usage>,
    pub(super) latest_mutation: Option<MutationValidation>,
    pub(super) web_searches: HashSet<String>,
    pub(super) web_sources: HashSet<String>,
    pub(super) web_reads: HashSet<String>,
    pub(super) workspace_generation: u64,
    pub(super) workspace_write_generation: u64,
}

impl ToolEvidence {
    /// Clears duplicate-suppression evidence whose tool results are no longer visible.
    pub(super) fn reset_model_visible_window(&mut self) {
        self.read_cache.clear();
        self.searches.clear();
        self.listings.clear();
        self.shell_inspections.clear();
        self.web_searches.clear();
        self.web_reads.clear();
    }

    /// Clears all task-scoped evidence while preserving workspace generations.
    pub(super) fn finish_task(&mut self) {
        self.reset_model_visible_window();
        self.edit_snapshots.clear();
        self.edit_read_coverage.clear();
        self.web_sources.clear();
        self.auxiliary_usage = None;
        self.latest_mutation = None;
    }

    /// Invalidates source snapshots and workspace inspections affected by known paths.
    pub(super) fn invalidate_paths(&mut self, changed_paths: &HashSet<String>) {
        self.read_cache
            .retain(|path, _| !changed_paths.contains(path));
        self.edit_snapshots
            .retain(|path, _| !changed_paths.contains(path));
        self.edit_read_coverage
            .retain(|path, _| !changed_paths.contains(path));
        self.searches.clear();
        self.listings.clear();
        self.shell_inspections.clear();
        self.workspace_generation = self.workspace_generation.wrapping_add(1);
    }

    /// Invalidates workspace evidence after a mutation with unknown scope.
    pub(super) fn invalidate_workspace(&mut self) {
        self.read_cache.clear();
        self.edit_snapshots.clear();
        self.edit_read_coverage.clear();
        self.searches.clear();
        self.listings.clear();
        self.shell_inspections.clear();
        self.workspace_generation = self.workspace_generation.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> ToolEvidence {
        let mut evidence = ToolEvidence::default();
        evidence.read_cache.insert(
            "a.rs".into(),
            vec![ReadCacheEntry {
                path: "a.rs".into(),
                snapshot: "snapshot-a".into(),
                start: 1,
                end: 2,
                total: 2,
                anchored: "a".into(),
            }],
        );
        evidence.edit_snapshots.insert("a.rs".into(), "a".into());
        evidence.edit_snapshots.insert("b.rs".into(), "b".into());
        evidence.edit_read_coverage.insert(
            "a.rs".into(),
            EditReadCoverage {
                snapshot: "a".into(),
                ranges: vec![(1, 2)],
            },
        );
        evidence.searches.insert("query".into());
        evidence.listings.insert(".".into());
        evidence.shell_inspections.insert("pwd".into());
        evidence.web_searches.insert("web query".into());
        evidence.web_sources.insert("https://example.com".into());
        evidence.web_reads.insert("https://example.com".into());
        evidence.latest_mutation = Some(MutationValidation {
            error_count: 0,
            changed_paths: vec!["a.rs".into()],
        });
        evidence.auxiliary_usage = Some(Usage::default());
        evidence.workspace_generation = 7;
        evidence.workspace_write_generation = 11;
        evidence
    }

    #[test]
    fn model_window_reset_keeps_task_scoped_provenance() {
        let mut evidence = seeded();
        evidence.reset_model_visible_window();
        assert!(evidence.read_cache.is_empty());
        assert!(evidence.searches.is_empty());
        assert!(evidence.web_searches.is_empty());
        assert!(evidence.web_reads.is_empty());
        assert!(evidence.edit_snapshots.contains_key("a.rs"));
        assert!(evidence.edit_read_coverage.contains_key("a.rs"));
        assert!(evidence.web_sources.contains("https://example.com"));
        assert!(evidence.latest_mutation.is_some());
        assert!(evidence.auxiliary_usage.is_some());
        assert_eq!(evidence.workspace_generation, 7);
        assert_eq!(evidence.workspace_write_generation, 11);
    }

    #[test]
    fn path_invalidation_removes_changed_snapshots_and_advances_generation() {
        let mut evidence = seeded();
        evidence.invalidate_paths(&HashSet::from(["a.rs".into()]));
        assert!(!evidence.read_cache.contains_key("a.rs"));
        assert!(!evidence.edit_snapshots.contains_key("a.rs"));
        assert!(evidence.edit_snapshots.contains_key("b.rs"));
        assert!(evidence.edit_read_coverage.is_empty());
        assert!(evidence.searches.is_empty());
        assert!(evidence.web_sources.contains("https://example.com"));
        assert_eq!(evidence.workspace_generation, 8);
        evidence.invalidate_workspace();
        assert!(evidence.read_cache.is_empty());
        assert!(evidence.edit_snapshots.is_empty());
        assert!(evidence.edit_read_coverage.is_empty());
        assert!(evidence.web_sources.contains("https://example.com"));
        assert!(evidence.web_reads.contains("https://example.com"));
        assert_eq!(evidence.workspace_generation, 9);
    }

    #[test]
    fn finishing_task_clears_task_and_model_evidence() {
        let mut evidence = seeded();
        evidence.finish_task();
        assert!(evidence.read_cache.is_empty());
        assert!(evidence.edit_snapshots.is_empty());
        assert!(evidence.edit_read_coverage.is_empty());
        assert!(evidence.web_sources.is_empty());
        assert!(evidence.web_reads.is_empty());
        assert!(evidence.latest_mutation.is_none());
        assert!(evidence.auxiliary_usage.is_none());
        assert_eq!(evidence.workspace_generation, 7);
        assert_eq!(evidence.workspace_write_generation, 11);
    }
}
