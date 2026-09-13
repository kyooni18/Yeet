//! Terminal-ratio aware layout decisions.
//!
//! Terminal cells are taller than they are wide, so raw column/row ratios are
//! misleading. These helpers use a rough 2:1 cell correction and centralize
//! the geometry policy used by the shell, composer, and modal surfaces.

use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shape {
    Tiny,
    ShortWide,
    Portrait,
    Compact,
    Standard,
    Wide,
    UltraWide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Metrics {
    pub shape: Shape,
    pub header_height: u16,
    pub status_height: u16,
    pub task_height: u16,
    pub suggestion_height: u16,
    pub input_min_lines: u16,
    pub input_max_lines: u16,
    pub horizontal_margin: u16,
    pub content_max_width: u16,
    pub sidebar_width: Option<u16>,
}

pub(super) fn metrics(area: Rect) -> Metrics {
    let shape = shape(area);
    match shape {
        Shape::Tiny => Metrics {
            shape,
            header_height: 1,
            status_height: 1,
            task_height: 1,
            suggestion_height: 3,
            input_min_lines: 1,
            input_max_lines: 1,
            horizontal_margin: 0,
            content_max_width: area.width,
            sidebar_width: None,
        },
        Shape::ShortWide => Metrics {
            shape,
            header_height: 1,
            status_height: 1,
            task_height: 1,
            suggestion_height: 4,
            input_min_lines: 1,
            input_max_lines: 2,
            horizontal_margin: u16::from(area.width >= 64),
            content_max_width: area.width.min(132),
            sidebar_width: None,
        },
        Shape::Portrait => Metrics {
            shape,
            header_height: if area.height >= 14 { 2 } else { 1 },
            status_height: 1,
            task_height: 1,
            suggestion_height: 6,
            input_min_lines: 2,
            input_max_lines: if area.height >= 34 { 6 } else { 4 },
            horizontal_margin: 0,
            content_max_width: area.width,
            sidebar_width: None,
        },
        Shape::Compact => Metrics {
            shape,
            header_height: if area.height >= 16 { 3 } else { 1 },
            status_height: 1,
            task_height: 2,
            suggestion_height: 6,
            input_min_lines: 2,
            input_max_lines: 5,
            horizontal_margin: u16::from(area.width >= 48),
            content_max_width: area.width,
            sidebar_width: None,
        },
        Shape::Standard => Metrics {
            shape,
            header_height: if area.height >= 16 { 3 } else { 1 },
            status_height: 1,
            task_height: 2,
            suggestion_height: 8,
            input_min_lines: 3,
            input_max_lines: 7,
            horizontal_margin: 1,
            content_max_width: 112,
            sidebar_width: sidebar_width(area, 24),
        },
        Shape::Wide => Metrics {
            shape,
            header_height: 3,
            status_height: 1,
            task_height: 2,
            suggestion_height: 8,
            input_min_lines: 3,
            input_max_lines: 7,
            horizontal_margin: 2,
            content_max_width: 120,
            sidebar_width: sidebar_width(area, 28),
        },
        Shape::UltraWide => Metrics {
            shape,
            header_height: 3,
            status_height: 1,
            task_height: 2,
            suggestion_height: 9,
            input_min_lines: 3,
            input_max_lines: if area.height >= 36 { 8 } else { 6 },
            horizontal_margin: 3,
            content_max_width: 132,
            sidebar_width: sidebar_width(area, 30),
        },
    }
}

pub(super) fn shape(area: Rect) -> Shape {
    if area.width < 34 || area.height < 9 {
        return Shape::Tiny;
    }
    let aspect = corrected_aspect(area);
    if area.height <= 14 && area.width >= 58 {
        return Shape::ShortWide;
    }
    if aspect <= 95
        || (area.width < 96 && area.height >= 28 && aspect <= 140)
        || (area.width < 72 && area.height >= 26)
    {
        return Shape::Portrait;
    }
    if area.width < 92 || area.height < 18 {
        return Shape::Compact;
    }
    if area.width >= 176 && aspect >= 230 {
        return Shape::UltraWide;
    }
    if area.width >= 132 && aspect >= 150 {
        return Shape::Wide;
    }
    Shape::Standard
}

pub(super) fn modal_rect(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    if area.width == 0 || area.height == 0 {
        return area;
    }
    let metrics = metrics(area);
    let width_cap = match metrics.shape {
        Shape::UltraWide => 148,
        Shape::Wide => 132,
        Shape::Standard => 116,
        Shape::Portrait => area.width.saturating_sub(2).max(1),
        Shape::ShortWide => 120,
        Shape::Compact | Shape::Tiny => area.width,
    };
    let minimum_width = match metrics.shape {
        Shape::Tiny => area.width,
        Shape::Portrait => area.width.min(44),
        Shape::Compact => area.width.min(54),
        _ => area.width.min(64),
    };
    let width = (area.width.saturating_mul(percent_x) / 100)
        .max(minimum_width)
        .min(width_cap)
        .min(area.width);

    let height_cap = match metrics.shape {
        Shape::ShortWide => area.height.saturating_sub(1).max(1),
        Shape::Portrait => area.height.saturating_sub(2).max(1),
        _ => area.height,
    };
    let minimum_height = match metrics.shape {
        Shape::Tiny | Shape::ShortWide => area.height.min(6),
        _ => area.height.min(12),
    };
    let height = (area.height.saturating_mul(percent_y) / 100)
        .max(minimum_height)
        .min(height_cap)
        .min(area.height);

    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn corrected_aspect(area: Rect) -> u16 {
    if area.height == 0 {
        return u16::MAX;
    }
    let denominator = u32::from(area.height).saturating_mul(2).max(1);
    ((u32::from(area.width).saturating_mul(100) / denominator).min(u16::MAX as u32)) as u16
}

fn sidebar_width(area: Rect, preferred: u16) -> Option<u16> {
    if area.height < 20 || area.width < 108 {
        return None;
    }
    let remaining = area.width.saturating_sub(preferred);
    (remaining >= 82).then_some(preferred.min(area.width / 4).max(24))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_terminal_ratios() {
        assert_eq!(shape(Rect::new(0, 0, 28, 8)), Shape::Tiny);
        assert_eq!(shape(Rect::new(0, 0, 120, 12)), Shape::ShortWide);
        assert_eq!(shape(Rect::new(0, 0, 56, 42)), Shape::Portrait);
        assert_eq!(shape(Rect::new(0, 0, 80, 31)), Shape::Portrait);
        assert_eq!(shape(Rect::new(0, 0, 80, 24)), Shape::Compact);
        assert_eq!(shape(Rect::new(0, 0, 120, 32)), Shape::Standard);
        assert_eq!(shape(Rect::new(0, 0, 160, 30)), Shape::Wide);
        assert_eq!(shape(Rect::new(0, 0, 220, 32)), Shape::UltraWide);
    }

    #[test]
    fn modals_never_escape_unusual_windows() {
        for area in [
            Rect::new(0, 0, 22, 8),
            Rect::new(0, 0, 58, 44),
            Rect::new(0, 0, 160, 12),
            Rect::new(0, 0, 220, 34),
        ] {
            let rect = modal_rect(area, 78, 72);
            assert!(rect.x >= area.x && rect.y >= area.y);
            assert!(rect.right() <= area.right());
            assert!(rect.bottom() <= area.bottom());
        }
    }

    #[test]
    fn shell_metrics_preserve_conversation_width_at_common_sizes() {
        let compact = metrics(Rect::new(0, 0, 80, 24));
        assert_eq!(compact.shape, Shape::Compact);
        assert_eq!(compact.sidebar_width, None);
        assert_eq!(compact.horizontal_margin, 1);

        let standard = metrics(Rect::new(0, 0, 120, 32));
        assert_eq!(standard.shape, Shape::Standard);
        assert_eq!(standard.sidebar_width, Some(24));
        assert_eq!(standard.content_max_width, 112);

        let wide = metrics(Rect::new(0, 0, 160, 30));
        assert_eq!(wide.shape, Shape::Wide);
        assert_eq!(wide.sidebar_width, Some(28));
        assert_eq!(wide.content_max_width, 120);

        let ultrawide = metrics(Rect::new(0, 0, 220, 32));
        assert_eq!(ultrawide.shape, Shape::UltraWide);
        assert_eq!(ultrawide.sidebar_width, Some(30));
        assert_eq!(ultrawide.content_max_width, 132);

        let portrait = metrics(Rect::new(0, 0, 64, 40));
        assert_eq!(portrait.shape, Shape::Portrait);
        assert_eq!(portrait.sidebar_width, None);
    }
}
