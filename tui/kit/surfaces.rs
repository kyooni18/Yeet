use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// A floating surface owns placement policy; callers paint its inner rectangle.
#[derive(Debug, Clone, Copy)]
pub struct FloatingView {
    pub width: u16,
    pub height: u16,
    pub modal: bool,
}
impl FloatingView {
    pub fn area(&self, bounds: Rect) -> Rect {
        let width = self.width.min(bounds.width);
        let height = self.height.min(bounds.height);
        Rect::new(
            bounds.x + (bounds.width - width) / 2,
            bounds.y + (bounds.height - height) / 2,
            width,
            height,
        )
    }
    pub fn paint(
        &self,
        frame: &mut Frame<'_>,
        title: &str,
        style: Style,
        content: impl FnOnce(&mut Frame<'_>, Rect),
    ) {
        let area = self.area(frame.area());
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .style(style);
        let inner = block.inner(area);
        frame.render_widget(Clear, area);
        frame.render_widget(block, area);
        content(frame, inner);
    }
    /// Modal surfaces block underlying input even outside their painted bounds.
    pub fn blocks(&self, bounds: Rect, point: ratatui::layout::Position) -> bool {
        self.modal || self.area(bounds).contains(point)
    }
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub expires: Instant,
}
#[derive(Debug, Clone, Default)]
pub struct Toasts {
    items: Vec<Toast>,
}
impl Toasts {
    pub fn push(&mut self, message: impl Into<String>, now: Instant, duration: Duration) {
        self.items.retain(|t| t.expires > now);
        if self.items.len() == 4 {
            self.items.remove(0);
        }
        self.items.push(Toast {
            message: message.into(),
            expires: now + duration,
        });
    }
    pub fn expire(&mut self, now: Instant) {
        self.items.retain(|t| t.expires > now);
    }
    pub fn items(&self) -> &[Toast] {
        &self.items
    }
    /// Toasts are non-modal, contain no hit targets, and never take focus.
    pub fn paint(&self, frame: &mut Frame<'_>, style: Style) {
        let bounds = frame.area();
        let width = bounds.width.min(52);
        let mut bottom = bounds.bottom();
        for toast in self.items.iter().rev() {
            let height = 4.min(bottom.saturating_sub(bounds.y));
            if height == 0 || width == 0 {
                break;
            }
            bottom -= height;
            let area = Rect::new(bounds.right() - width, bottom, width, height);
            frame.render_widget(Clear, area);
            frame.render_widget(
                Paragraph::new(toast.message.as_str())
                    .wrap(Wrap { trim: false })
                    .block(Block::default().borders(Borders::ALL))
                    .style(style),
                area,
            );
        }
    }
}
