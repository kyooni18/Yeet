//! Animated YEET sanctum shown while a conversation is empty.
//!
//! The source reference is the classic red striped network mark.  The terminal
//! version deliberately stays text-native: the stripes are real glyphs, the
//! glow is ANSI colour, and every bit of motion is derived from a monotonic
//! clock so it also works in Yeet Remote without shipping an image asset.

use super::{responsive, theme};
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    prelude::{Color, Line, Modifier, Span, Style},
    widgets::{Block, Paragraph},
};

const VOID: Color = Color::Rgb(8, 3, 3);
const RED_LOW: Color = Color::Rgb(78, 8, 9);
const RED_DIM: Color = Color::Rgb(126, 10, 13);
const RED: Color = Color::Rgb(218, 22, 27);
const RED_HOT: Color = Color::Rgb(255, 74, 65);
const WHITE: Color = Color::Rgb(241, 236, 231);
const WHITE_DIM: Color = Color::Rgb(149, 137, 133);

// Sampled from the supplied mark rather than invented as generic triangle art.
// Each terminal row corresponds to one of the horizontal red stripes.
const LOGO_FULL: &[&str] = &[
    "                     ━━",
    "                    ━━━━━━",
    "                   ━━━━━━━━",
    "                 ━━━━━━━━━━━━",
    "                  ━━━━━━━━━━",
    "              ━━━━  ━━━━━━  ━━━━",
    "             ━━━━━━  ━━━━  ━━━━━━",
    "           ━━━━━━━━━━    ━━━━━━━━━━",
    "          ━━━━━━━━━━━━  ━━━━━━━━━━━━",
    "        ━━━━━━━━━━━━━━  ━━━━━━━━━━━━━━",
    "       ━━━━━━━━━━━━━━━  ━━━━━━━━━━━━━━━",
    "     ━━━━━━━━━━━━━━━━━  ━━━━━━━━━━━━━━━━",
    "    ━━━━━━━━━━━━━━━━━━  ━━━━━━━━━━━━━━━━━━",
];

const LOGO_COMPACT: &[&str] = &[
    "                     ━━",
    "                  ━━━━━━━━",
    "                 ━━━━━━━━━━",
    "            ━━━━━━  ━━━━  ━━━━━━",
    "         ━━━━━━━━━━━━  ━━━━━━━━━━━━",
    "      ━━━━━━━━━━━━━━━  ━━━━━━━━━━━━━━━",
    "    ━━━━━━━━━━━━━━━━━  ━━━━━━━━━━━━━━━━",
    "   ━━━━━━━━━━━━━━━━━━  ━━━━━━━━━━━━━━━━━━",
];

const BIG_TITLE: &[&str] = &[
    "██╗   ██╗███████╗███████╗████████╗",
    "╚██╗ ██╔╝██╔════╝██╔════╝╚══██╔══╝",
    " ╚████╔╝ █████╗  █████╗     ██║   ",
    "  ╚██╔╝  ██╔══╝  ██╔══╝     ██║   ",
    "   ██║   ███████╗███████╗   ██║   ",
];

const TITLE: &str = "YEET";
const CORPORATION: &str = "· AGENTIC SYSTEMS CORE ·";
const DIRECTIVE: &str = "AWAITING DIRECTIVE";

pub(super) fn draw(frame: &mut Frame<'_>, area: Rect, shape: responsive::Shape) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    frame.render_widget(Block::default().style(Style::default().bg(VOID)), area);
    let tick = animation_tick();

    if area.width < 48 || area.height < 6 {
        draw_minimal(frame, area, tick);
        return;
    }

    if matches!(shape, responsive::Shape::Portrait) {
        draw_portrait(frame, area, tick);
        return;
    }

    let lavish = match shape {
        responsive::Shape::Portrait => area.width >= 48 && area.height >= 24,
        responsive::Shape::Standard | responsive::Shape::Wide | responsive::Shape::UltraWide => {
            area.height >= 18
        }
        _ => false,
    };
    if lavish {
        draw_sanctum(frame, area, tick, shape);
    }

    let logo = if lavish { LOGO_FULL } else { LOGO_COMPACT };
    let show_big_title = lavish && area.width >= 46 && area.height >= 18;
    let logo_rows = if show_big_title && matches!(shape, responsive::Shape::Portrait) {
        10.min(logo.len())
    } else if show_big_title {
        8.min(logo.len())
    } else if area.height >= 13 {
        logo.len()
    } else {
        logo.len().min(5)
    };
    let show_corporation = area.width >= CORPORATION.len() as u16 + 4 && area.height >= 14;

    let mut lines = Vec::with_capacity(logo_rows + 7);
    let scan = (tick / 2) % (logo_rows + 5);
    for (row, art) in logo.iter().take(logo_rows).enumerate() {
        let style = stripe_style(row, scan, tick);
        let art = glitch_stripe(art, row, tick);
        lines.push(Line::styled(format!("{art:<44}"), style));
    }

    lines.push(Line::raw(""));
    if show_big_title {
        lines.extend(big_title_lines(tick));
    } else {
        lines.push(title_line(tick));
    }
    if show_corporation {
        lines.push(Line::styled(
            CORPORATION,
            Style::default().fg(if tick % 28 < 14 { RED_DIM } else { RED_LOW }),
        ));
    }
    if area.height >= 10 {
        lines.push(Line::raw(""));
        lines.push(directive_line(tick));
    }

    let height = (lines.len() as u16).min(area.height);
    let center = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(height) / 2,
        area.width,
        height,
    );
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), center);
}

fn animation_tick() -> usize {
    theme::animation_tick()
}

fn stripe_style(row: usize, scan: usize, tick: usize) -> Style {
    let distance = row.abs_diff(scan);
    let color = match distance {
        0 => RED_HOT,
        1 => RED,
        2 => RED_DIM,
        _ if (tick / 8 + row) % 4 == 0 => RED,
        _ => RED_DIM,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

fn glitch_stripe(art: &'static str, row: usize, tick: usize) -> String {
    // A very short, sparse horizontal phase slip.  It reads as CRT/neural
    // instability without making the mark jitter constantly.
    match tick % 79 {
        0 if row % 3 == 0 => format!(" {art}"),
        1 if row % 4 == 0 => art.trim_start_matches(' ').to_owned(),
        _ => art.to_owned(),
    }
}

fn title_line(tick: usize) -> Line<'static> {
    let active = (tick / 3) % 4;
    let mut spans = Vec::with_capacity(4);
    for (index, letter) in ['Y', 'E', 'E', 'T'].into_iter().enumerate() {
        let color = if index == active { RED_HOT } else { WHITE };
        spans.push(Span::styled(
            letter.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

fn big_title_lines(tick: usize) -> Vec<Line<'static>> {
    let scan = (tick / 2) % (BIG_TITLE.len() + 4);
    BIG_TITLE
        .iter()
        .enumerate()
        .map(|(row, text)| {
            let distance = row.abs_diff(scan);
            let color = match distance {
                0 => RED_HOT,
                1 => RED,
                _ => WHITE,
            };
            let shifted = match tick % 97 {
                0 if row == 1 => format!(" {text}"),
                1 if row == 3 => text.trim_start().to_owned(),
                _ => (*text).to_owned(),
            };
            Line::styled(
                shifted,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

fn directive_line(tick: usize) -> Line<'static> {
    let orbit = ["·", "∙", "●", "∙"][(tick / 2) % 4];
    Line::from(vec![
        Span::styled(format!("{orbit}  "), Style::default().fg(RED_DIM)),
        Span::styled(
            DIRECTIVE,
            Style::default().fg(WHITE_DIM).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {orbit}"), Style::default().fg(RED_DIM)),
    ])
}

fn draw_portrait(frame: &mut Frame<'_>, area: Rect, tick: usize) {
    let logo_rows = if area.height >= 24 {
        6
    } else if area.height >= 16 {
        4
    } else {
        2
    };
    let logo_rows = logo_rows.min(LOGO_COMPACT.len());
    let mut lines = Vec::with_capacity(logo_rows + 4);
    let scan = (tick / 2) % (logo_rows + 5);
    for (row, art) in LOGO_COMPACT.iter().take(logo_rows).enumerate() {
        let art = glitch_stripe(art, row, tick);
        lines.push(Line::styled(
            art.trim_start().to_owned(),
            stripe_style(row, scan, tick),
        ));
    }
    lines.push(Line::raw(""));
    lines.push(title_line(tick));
    if area.height >= 10 {
        lines.push(Line::raw(""));
        lines.push(directive_line(tick));
    }

    let height = (lines.len() as u16).min(area.height);
    let center = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(height) / 2,
        area.width,
        height,
    );
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), center);
}

fn draw_sanctum(frame: &mut Frame<'_>, area: Rect, tick: usize, shape: responsive::Shape) {
    let width = match shape {
        responsive::Shape::UltraWide => area.width.min(104),
        responsive::Shape::Wide => area.width.min(88),
        responsive::Shape::Portrait => area.width.min(56),
        _ => area.width.min(72),
    };
    let left = area.x + (area.width - width) / 2;
    let right = left + width.saturating_sub(1);
    if right <= left + 3 {
        return;
    }

    let top = area.y;
    let bottom = area.bottom().saturating_sub(1);
    let spark = top + ((tick / 2) as u16 % area.height.max(1));
    let reverse_spark = bottom.saturating_sub((tick as u16 / 2) % area.height.max(1));
    let buffer = frame.buffer_mut();

    for y in top..=bottom {
        let left_symbol = if y == spark { "◆" } else { "┊" };
        let right_symbol = if y == reverse_spark { "◆" } else { "┊" };
        let left_color = if y == spark { RED_HOT } else { RED_LOW };
        let right_color = if y == reverse_spark { RED_HOT } else { RED_LOW };
        buffer[(left, y)]
            .set_symbol(left_symbol)
            .set_fg(left_color)
            .set_bg(VOID);
        buffer[(right, y)]
            .set_symbol(right_symbol)
            .set_fg(right_color)
            .set_bg(VOID);
    }

    let span = right.saturating_sub(left + 1) as usize;
    if span > 2 {
        let arch = format!("╭{}╮", "─".repeat(span));
        frame.render_widget(
            Paragraph::new(Line::styled(arch, Style::default().fg(RED_LOW)))
                .alignment(Alignment::Center),
            Rect::new(left, top, width, 1),
        );
    }
}

fn draw_minimal(frame: &mut Frame<'_>, area: Rect, tick: usize) {
    let pulse = if tick % 10 < 5 { RED } else { RED_DIM };
    let text = if area.width >= 18 {
        Line::from(vec![
            Span::styled("◆ ", Style::default().fg(pulse)),
            Span::styled(
                TITLE,
                Style::default().fg(WHITE).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ◆", Style::default().fg(pulse)),
        ])
    } else {
        Line::styled(
            "YEET",
            Style::default().fg(WHITE).add_modifier(Modifier::BOLD),
        )
    };
    frame.render_widget(
        Paragraph::new(text).alignment(Alignment::Center),
        Rect::new(area.x, area.y + area.height / 2, area.width, 1),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanline_promotes_one_stripe_to_hot_red() {
        assert_eq!(stripe_style(2, 2, 0).fg, Some(RED_HOT));
        assert_ne!(stripe_style(2, 4, 0).fg, Some(RED_HOT));
    }

    #[test]
    fn glitch_is_sparse_and_deterministic() {
        let art = LOGO_COMPACT[0];
        assert_eq!(glitch_stripe(art, 0, 0), format!(" {art}"));
        assert_eq!(glitch_stripe(art, 0, 2), art);
    }
}
