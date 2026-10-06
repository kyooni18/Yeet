use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::Style,
    widgets::Paragraph,
};

/// Geometry belongs to the last painted frame, never to persisted view state.
#[derive(Debug, Clone)]
pub struct HitMap<A> {
    bounds: Rect,
    targets: Vec<(Rect, A)>,
}
impl<A> Default for HitMap<A> {
    fn default() -> Self {
        Self {
            bounds: Rect::default(),
            targets: Vec::new(),
        }
    }
}
impl<A> HitMap<A> {
    pub fn begin(&mut self, bounds: Rect) {
        self.bounds = bounds;
        self.targets.clear();
    }
    pub fn register(&mut self, area: Rect, action: A) {
        let area = area.intersection(self.bounds);
        if area.width > 0 && area.height > 0 {
            self.targets.push((area, action));
        }
    }
    pub fn at(&self, point: Position) -> Option<&A> {
        self.targets
            .iter()
            .rev()
            .find(|(r, _)| r.contains(point))
            .map(|(_, a)| a)
    }
    pub fn targets(&self) -> impl Iterator<Item = &(Rect, A)> {
        self.targets.iter()
    }
}

/// A frame-local context: controls paint and register the same geometry.
pub struct Ui<'a, 'f, A> {
    frame: &'a mut Frame<'f>,
    hits: &'a mut HitMap<A>,
    area: Rect,
}
impl<'a, 'f, A> Ui<'a, 'f, A> {
    pub fn new(frame: &'a mut Frame<'f>, hits: &'a mut HitMap<A>, area: Rect) -> Self {
        let area = area.intersection(frame.area());
        Self { frame, hits, area }
    }
    pub fn button(&mut self, area: Rect, label: &str, style: Style, action: A) {
        let area = area.intersection(self.area);
        self.frame
            .render_widget(Paragraph::new(label).style(style), area);
        self.hits.register(area, action);
    }
}

/// Reveal the selected row without underflow at zero-height viewports.
pub fn visible_start(selected: usize, height: usize) -> usize {
    selected.saturating_sub(height.saturating_sub(1))
}
