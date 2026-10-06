//! Platform-independent view instances and their selection lifecycle.
//!
//! Resource identity and presentation titles do not determine instance identity.
//! Renderers keep geometry and native widget state outside these types.

use serde::{Deserialize, Serialize};

/// Identity scoped to its owning collection, independent of vector positions.
/// A view kind and this ID together identify a surface across collections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SurfaceId(u64);

/// An instance identity is independent of its resource and display title.
#[derive(Debug, Clone)]
pub struct View<T> {
    pub id: SurfaceId,
    pub title: String,
    pub state: T,
}
#[derive(Debug, Clone)]
pub struct Tabs<T> {
    views: Vec<View<T>>,
    active: Option<SurfaceId>,
    next: u64,
}
impl<T> Default for Tabs<T> {
    fn default() -> Self {
        Self {
            views: Vec::new(),
            active: None,
            next: 0,
        }
    }
}
impl<T> Tabs<T> {
    pub fn open(&mut self, title: impl Into<String>, state: T) -> SurfaceId {
        self.next = self.next.checked_add(1).expect("view identity exhausted");
        let id = SurfaceId(self.next);
        self.views.push(View {
            id,
            title: title.into(),
            state,
        });
        self.active = Some(id);
        id
    }
    pub fn activate(&mut self, id: SurfaceId) -> bool {
        if self.views.iter().any(|v| v.id == id) {
            self.active = Some(id);
            true
        } else {
            false
        }
    }
    pub fn get(&self, id: SurfaceId) -> Option<&View<T>> {
        self.views.iter().find(|view| view.id == id)
    }
    pub fn get_mut(&mut self, id: SurfaceId) -> Option<&mut View<T>> {
        self.views.iter_mut().find(|view| view.id == id)
    }
    pub fn active_id(&self) -> Option<SurfaceId> {
        self.active
    }
    pub fn len(&self) -> usize {
        self.views.len()
    }
    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }
    pub fn active(&self) -> Option<&View<T>> {
        self.views.iter().find(|v| Some(v.id) == self.active)
    }
    pub fn active_mut(&mut self) -> Option<&mut View<T>> {
        self.views.iter_mut().find(|v| Some(v.id) == self.active)
    }
    pub fn views(&self) -> &[View<T>] {
        &self.views
    }
    pub fn close(&mut self, id: SurfaceId) -> Option<View<T>> {
        let index = self.views.iter().position(|v| v.id == id)?;
        let view = self.views.remove(index);
        if self.active == Some(id) {
            self.active = self
                .views
                .get(index.min(self.views.len().saturating_sub(1)))
                .map(|v| v.id);
        }
        Some(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_preserve_instance_state_and_reject_closed_identities() {
        let mut tabs = Tabs::default();
        let first = tabs.open("same title", 12);
        let second = tabs.open("same title", 99);
        tabs.activate(first);
        assert_eq!(tabs.active().unwrap().state, 12);
        tabs.close(first);
        assert_eq!(tabs.active().unwrap().id, second);
        assert!(!tabs.activate(first));
        assert_eq!(tabs.active().unwrap().state, 99);
        tabs.close(second);
        assert!(tabs.is_empty());
        assert_eq!(tabs.active_id(), None);
        let third = tabs.open("same title", 7);
        assert_ne!(third, first);
        assert_ne!(third, second);
    }
}
