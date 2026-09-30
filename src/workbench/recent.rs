use super::ResourceTarget;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct RecentView {
    pub target: ResourceTarget,
    pub title: String,
    pub visited_at: DateTime<Utc>,
}

/// Bounded, most-recent-first history keyed by resource, never by tab index.
#[derive(Debug, Clone, Default)]
pub struct RecentViews {
    entries: Vec<RecentView>,
}

impl RecentViews {
    pub fn entries(&self) -> &[RecentView] {
        &self.entries
    }

    pub fn visit(&mut self, target: ResourceTarget, title: impl Into<String>) {
        let title = title.into();
        self.entries.retain(|entry| entry.target != target);
        self.entries.insert(
            0,
            RecentView {
                target,
                title,
                visited_at: Utc::now(),
            },
        );
        self.entries.truncate(20);
    }

    pub fn update_title(&mut self, target: &ResourceTarget, title: impl Into<String>) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| &entry.target == target)
        {
            entry.title = title.into();
        }
    }
}
