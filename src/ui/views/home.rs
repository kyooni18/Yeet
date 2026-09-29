//! Home workbench view: recent objects, workspace activity, and focused details.
use super::super::{status, theme};
use crate::app::App;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    prelude::{Line, Span, Style},
    style::Modifier,
    widgets::{Block, Paragraph, Wrap},
};

const RAIL_WIDTH: u16 = 23;
const ACTIVE: Modifier = Modifier::BOLD;

pub(crate) fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let bounds = frame.area();
    frame.render_widget(Block::default().style(theme::base()), bounds);
    if bounds.width < 45 || bounds.height < 5 {
        return;
    }
    let status_height = 1.min(bounds.height);
    let content_height = bounds.height.saturating_sub(status_height);
    let content = Rect::new(bounds.x, bounds.y, bounds.width, content_height);
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(content);
    draw_tabs(frame, rows[0]);
    let body = rows[1];
    let rail_width = RAIL_WIDTH.min(body.width.saturating_sub(30));
    let work = Layout::horizontal([Constraint::Length(rail_width), Constraint::Min(1)]).split(body);
    draw_recent(frame, work[0]);
    let main_and_inspector =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(work[1]);
    draw_activity(frame, main_and_inspector[0]);
    if body.width >= 110 {
        draw_inspector(frame, main_and_inspector[1]);
    } else {
        frame.render_widget(Block::default().style(theme::base()), main_and_inspector[1]);
    }
    let composer = Rect::new(
        bounds.x.saturating_add(rail_width),
        rows[2].y,
        rows[2].width.saturating_sub(rail_width),
        rows[2].height,
    );
    draw_composer(frame, app, composer);
    status::draw(
        frame,
        app,
        Rect::new(
            bounds.x,
            bounds.y + content_height,
            bounds.width,
            status_height,
        ),
    );
}

fn draw_tabs(frame: &mut Frame<'_>, area: Rect) {
    if area.width == 0 {
        return;
    }
    let entries = [
        ("⌂ Home", true, 16usize),
        ("▤ MM305 crosswind tuning", false, 25),
        ("▱ theme.rs", false, 17),
        ("▣ Landing", false, 17),
        ("⚙ guidance_taem.c", false, 21),
        ("▣ #214", false, 12),
    ];
    let mut spans = Vec::new();
    let mut used = 0usize;
    for (label, active, width) in entries {
        if used + width > area.width as usize {
            break;
        }
        let style = if active {
            Style::default().fg(theme::text()).add_modifier(ACTIVE)
        } else {
            Style::default().fg(theme::muted())
        };
        spans.push(Span::styled(format!("{label:<width$}"), style));
        used += width;
    }
    if used < area.width as usize {
        spans.push(Span::styled("+", Style::default().fg(theme::muted())));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::code_background())),
        Rect::new(area.x, area.y, area.width, 1),
    );
    frame.render_widget(
        Paragraph::new("─".repeat(area.width as usize)).style(Style::default().fg(theme::border())),
        Rect::new(area.x, area.y + 1, area.width, 1),
    );
}

fn draw_recent(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::surface_color())),
        area,
    );
    if area.width == 0 || area.height == 0 {
        return;
    }
    let add_area = Rect::new(area.x, area.y, area.width, 2.min(area.height));
    frame.render_widget(
        Paragraph::new("+")
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(theme::text()).bg(theme::code_background())),
        add_area,
    );
    let rows = [
        ("/", "MM305 crosswind", "18s"),
        ("?", "TUI shell direction", "3m"),
        ("●", "Runtime MCP attach", "9m"),
        ("·", "Provider cleanup", "21m"),
        ("·", "Foundation memory", "43m"),
    ];
    for (i, (icon, title, age)) in rows.iter().enumerate() {
        let y = area.y.saturating_add(2 + i as u16 * 2);
        if y >= area.bottom() {
            break;
        }
        let selected = i == 0;
        if selected {
            frame.render_widget(
                Block::default().style(Style::default().bg(theme::selected_color())),
                Rect::new(area.x, y, area.width, 2.min(area.bottom() - y)),
            );
        }
        let style = if selected {
            Style::default().fg(theme::text()).add_modifier(ACTIVE)
        } else {
            Style::default().fg(theme::muted())
        };
        let inner_width = area.width.saturating_sub(2) as usize;
        let title_width = inner_width.saturating_sub(icon.len() + age.len() + 2);
        let line = Line::from(vec![
            Span::styled(format!("{icon} "), style),
            Span::styled(super::super::task::fit(title, title_width), style),
            Span::styled(format!(" {age}"), Style::default().fg(theme::muted())),
        ]);
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(area.x + 1, y, area.width.saturating_sub(2), 1),
        );
    }
}

fn draw_activity(frame: &mut Frame<'_>, area: Rect) {
    if area.width == 0 {
        return;
    }
    frame.render_widget(Block::default().style(theme::base()), area);
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Min(2),
    ])
    .split(area);
    let selected = Style::default()
        .fg(theme::text())
        .bg(theme::selected_color())
        .add_modifier(ACTIVE);
    frame.render_widget(
        Paragraph::new("▣  Verification: accept a 3.2 km floor?     Landing    4m").style(selected),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new("▤  MM305 crosswind                                      18s")
            .style(Style::default().fg(theme::muted())),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new("▣  Landing                                               2 running   26m")
            .style(Style::default().fg(theme::muted())),
        rows[2],
    );
    frame.render_widget(
        Paragraph::new("▤  TUI shell direction                                   3m")
            .style(Style::default().fg(theme::muted())),
        rows[3],
    );
    frame.render_widget(Paragraph::new("\n C  guidance_taem.c                         +18  -6\n C  taem_candidate_search.c                 +42  -13\n C  taem_candidate_search.h                  +8   -4").style(Style::default().fg(theme::muted())).wrap(Wrap { trim: false }), rows[4]);
    frame.render_widget(
        Paragraph::new(
            "\n ▣  #214  Context rail focus order\n ▣  #219  Final-speed handoff threshold",
        )
        .style(Style::default().fg(theme::muted())),
        rows[5],
    );
    if area.height >= 8 {
        let y = area.bottom().saturating_sub(4);
        frame.render_widget(Paragraph::new("anthropic     5h  ▰▰▰▱▱▱▱▱ 62%      week ▰▰▱▱▱▱▱▱ 31%\nopenai        5h  ▰▱▱▱▱▱▱▱ 12%      week ▰▱▱▱▱▱▱▱  4%").style(Style::default().fg(theme::muted())), Rect::new(area.x+5,y,area.width.saturating_sub(10),2));
    }
}

fn draw_inspector(frame: &mut Frame<'_>, area: Rect) {
    if area.width == 0 {
        return;
    }
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::surface_color())),
        area,
    );
    let pad = 2.min(area.width);
    let x = area.x + pad;
    let width = area.width.saturating_sub(pad + 1);
    let content = Rect::new(x, area.y + 2, width, area.height.saturating_sub(4));
    let text = "Accept a 3.2 km floor?\n\nVerification · Landing · 4m ago\n\nSweep at 3 km misses final speed for headings above 270°.\n\nTightest flyable radius is 3.2 km; nominal is 12 km.\n\na   Accept 3.2 km floor\n\nb   Keep 3.0 km and add margin\n\nc   Have Planner re-sweep first";
    frame.render_widget(
        Paragraph::new(text)
            .style(Style::default().fg(theme::muted()))
            .wrap(Wrap { trim: false }),
        content,
    );
}

fn draw_composer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if area.height == 0 {
        return;
    }
    let style = Style::default()
        .fg(theme::text())
        .bg(theme::surface_color());
    let prompt = if app.input.is_empty() {
        "Ask Yeet..."
    } else {
        &app.input
    };
    let line = Line::from(vec![
        Span::styled(
            "+  ",
            Style::default()
                .fg(theme::secondary())
                .bg(theme::surface_color()),
        ),
        Span::styled(prompt, style),
        Span::styled(
            " →",
            Style::default()
                .fg(theme::muted())
                .bg(theme::surface_color()),
        ),
    ]);
    frame.render_widget(Paragraph::new(line).style(style), area);
}
