//! Small shared interaction and surface primitives over Ratatui.
mod surfaces;
mod widgets;
pub use surfaces::{FloatingView, SurfaceId, Tabs, Toast, Toasts, View};
pub use widgets::{HitMap, Ui, visible_start};

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        Terminal,
        backend::TestBackend,
        layout::{Position, Rect},
        style::Style,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn geometry_clips_and_discards_previous_frame_targets() {
        let mut hits = HitMap::default();
        hits.begin(Rect::new(2, 2, 5, 3));
        hits.register(Rect::new(0, 0, 4, 4), 1);
        assert_eq!(hits.at(Position::new(2, 2)), Some(&1));
        assert_eq!(hits.at(Position::new(1, 2)), None);
        hits.register(Rect::new(2, 2, 1, 1), 2);
        assert_eq!(hits.at(Position::new(2, 2)), Some(&2));
        hits.begin(Rect::new(0, 0, 1, 1));
        assert_eq!(hits.at(Position::new(2, 2)), None);
        assert_eq!(visible_start(9, 3), 7);
    }

    #[test]
    fn surfaces_preserve_state_and_expire_notifications() {
        let mut tabs = Tabs::default();
        let first = tabs.open("same title", 12);
        let second = tabs.open("same title", 99);
        tabs.activate(first);
        assert_eq!(tabs.active().unwrap().state, 12);
        tabs.close(first);
        assert_eq!(tabs.active().unwrap().id, second);
        assert!(!tabs.activate(first));
        let floating = FloatingView {
            width: 58,
            height: 9,
            modal: true,
        };
        assert_eq!(floating.area(Rect::new(5, 3, 4, 2)), Rect::new(5, 3, 4, 2));
        assert!(floating.blocks(Rect::new(0, 0, 100, 40), Position::new(0, 0)));
        let now = Instant::now();
        let mut toasts = Toasts::default();
        for i in 0..6 {
            toasts.push(i.to_string(), now, Duration::from_secs(1));
        }
        assert_eq!(toasts.items().len(), 4);
        let mut terminal = Terminal::new(TestBackend::new(4, 2)).unwrap();
        terminal
            .draw(|frame| toasts.paint(frame, Style::default()))
            .unwrap();
        toasts.expire(now + Duration::from_secs(1));
        assert!(toasts.items().is_empty());
    }
}
