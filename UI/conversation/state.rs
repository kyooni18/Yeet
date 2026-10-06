//! Stable disclosure and work-selection state independent of native row geometry.
use super::*;
use crate::harness::{HarnessCommand, HarnessState};
use crate::model::{ConversationEntry, ConversationKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationState {
    pub selected: Option<String>,
    pub expanded: BTreeMap<String, bool>,
    pub expand_all: bool,
    /// Existing inspect-work workflow opens running tools and reasoning blocks.
    /// This is a shared preference, not a platform branch.
    pub inspect_work: bool,
    attention: BTreeMap<String, bool>,
}
impl ConversationState {
    pub fn inspect_work() -> Self {
        Self {
            inspect_work: true,
            ..Default::default()
        }
    }
    pub fn expanded_for(&self, id: &str, default: bool) -> bool {
        self.expand_all || self.expanded.get(id).copied().unwrap_or(default)
    }
    pub fn view(&self, entries: &[ConversationEntry], harness: &HarnessState) -> ConversationView {
        super::projection::project(entries, harness, self)
    }
    /// Reconcile attention transitions and stable IDs before publishing a view.
    pub fn project(
        &mut self,
        entries: &[ConversationEntry],
        harness: &HarnessState,
    ) -> ConversationView {
        let view = self.view(entries, harness);
        let ids: Vec<_> = view
            .items
            .iter()
            .filter(|item| !matches!(item, DisplayItem::Entry { .. }))
            .map(|item| item.id().to_owned())
            .collect();
        if self
            .selected
            .as_ref()
            .is_some_and(|selected| !ids.contains(selected))
        {
            self.selected = None;
        }
        for item in &view.items {
            if let DisplayItem::Activity { id, group } = item {
                let attention = group.failed || group.awaits_permission;
                if attention && self.attention.get(id) == Some(&false) {
                    self.expanded.insert(id.clone(), true);
                }
                self.attention.insert(id.clone(), attention);
            }
        }
        self.view(entries, harness)
    }
    pub fn apply(
        &mut self,
        action: ConversationAction,
        view: &ConversationView,
        harness: &HarnessState,
    ) -> ConversationEffect {
        let mut effect = ConversationEffect::default();
        match action {
            ConversationAction::Toggle(id) => {
                if let Some(expanded) = disclosure_expanded(view, &id) {
                    self.expanded.insert(id, !expanded);
                }
            }
            ConversationAction::Select(id) => {
                self.selected = id.filter(|id| {
                    view.items
                        .iter()
                        .any(|item| item.id() == id && !matches!(item, DisplayItem::Entry { .. }))
                })
            }
            ConversationAction::MoveSelection(delta) => {
                let ids: Vec<_> = view
                    .items
                    .iter()
                    .filter(|item| !matches!(item, DisplayItem::Entry { .. }))
                    .map(|item| item.id().to_owned())
                    .collect();
                if ids.is_empty() {
                    self.selected = None;
                } else {
                    let target = self
                        .selected
                        .as_ref()
                        .and_then(|id| ids.iter().position(|item| item == id))
                        .map(|current| current.saturating_add_signed(delta).min(ids.len() - 1))
                        .unwrap_or(if delta < 0 { ids.len() - 1 } else { 0 });
                    self.selected = Some(ids[target].clone());
                }
            }
            ConversationAction::SetExpandAll(expanded) => self.expand_all = expanded,
            ConversationAction::SetInspectWork(inspect) => self.inspect_work = inspect,
            ConversationAction::Copy(ref id)
            | ConversationAction::Edit(ref id)
            | ConversationAction::Regenerate(ref id) => {
                let entry = view.items.iter().find_map(|item| match item {
                    DisplayItem::Entry { entry, .. } if entry.id == *id => Some(entry),
                    _ => None,
                });
                if let Some(entry) = entry {
                    match (&entry.kind, action) {
                        (
                            ConversationKind::User { content }
                            | ConversationKind::Assistant { content, .. },
                            ConversationAction::Copy(_),
                        ) => {
                            effect.copy = Some(CopyEffect {
                                entry_id: entry.id.clone(),
                                text: content.clone(),
                            })
                        }
                        (ConversationKind::User { content }, ConversationAction::Edit(_))
                            if !harness.is_streaming
                                && view.last_user_id.as_deref() == Some(entry.id.as_str()) =>
                        {
                            effect.edit_draft = Some(content.clone())
                        }
                        (ConversationKind::Assistant { .. }, ConversationAction::Regenerate(_))
                            if !harness.is_streaming
                                && view.last_assistant_id.as_deref() == Some(entry.id.as_str()) =>
                        {
                            effect.command = Some(HarnessCommand::RegenerateLast)
                        }
                        _ => {}
                    }
                }
            }
            ConversationAction::Reset => {
                let inspect = self.inspect_work;
                *self = Self::default();
                self.inspect_work = inspect;
            }
        }
        effect
    }
}
fn disclosure_expanded(view: &ConversationView, id: &str) -> Option<bool> {
    for item in &view.items {
        match item {
            DisplayItem::Activity {
                id: group_id,
                group,
            } => {
                if group_id == id {
                    return Some(group.expanded);
                }
                if let Some(event) = group
                    .events
                    .iter()
                    .find(|event| event.key() == id && !event.trace().details.is_empty())
                {
                    return Some(event.trace().expanded);
                }
            }
            DisplayItem::Reasoning {
                id: reasoning_id,
                trace,
                ..
            } if reasoning_id == id => return Some(trace.expanded),
            _ => {}
        }
    }
    None
}
