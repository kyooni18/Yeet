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
    if bounds.width < 45 || bounds.height < 5 {
        return;
    }
    if bounds.width < 110 {
        draw_compact(frame, app, bounds);
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
    // The reference workbench has a 232px context rail in a 1440px viewport.
    // Scale that proportion down for terminal cells while retaining useful main space.
    let rail_width = ((body.width as u32 * 232 / 1440) as u16)
        .clamp(14, RAIL_WIDTH)
        .min(body.width.saturating_sub(30));
    let work = Layout::horizontal([Constraint::Length(rail_width), Constraint::Min(1)]).split(body);
    draw_recent(frame, work[0]);
    // The mockup's main surface is a 50/50 activity and detail-inspector split.
    let panes =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(work[1]);
    draw_activity(frame, panes[0]);
    draw_inspector(frame, panes[1]);
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
        ("⌂ Home", true),
        ("▤ MM305 crosswind tuning", false),
        ("▱ theme.rs", false),
        ("▣ Landing", false),
        ("⚙ guidance_taem.c", false),
        ("▣ #214", false),
    ];
    let mut spans = Vec::new();
    let mut used = 0usize;
    for (label, active) in entries {
        let width = label.chars().count() + 4;
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
    let add_height = 2.min(area.height);
    frame.render_widget(
        Paragraph::new("+")
            .alignment(ratatui::layout::Alignment::Center)
            .style(
                Style::default()
                    .fg(theme::text())
                    .bg(theme::code_background()),
            ),
        Rect::new(area.x + 1, area.y, area.width.saturating_sub(2), add_height),
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
        let row = Rect::new(area.x, y, area.width, 1);
        let style = if selected {
            Style::default()
                .fg(theme::text())
                .bg(theme::selected_color())
                .add_modifier(ACTIVE)
        } else {
            Style::default().fg(theme::muted())
        };
        if selected {
            frame.render_widget(
                Block::default().style(Style::default().bg(theme::selected_color())),
                row,
            );
        }
        let inner_width = area.width.saturating_sub(2) as usize;
        let title_width =
            inner_width.saturating_sub(icon.chars().count() + age.chars().count() + 2);
        let fitted = super::super::task::fit(title, title_width);
        let gap = " ".repeat(inner_width.saturating_sub(
            icon.chars().count() + fitted.chars().count() + age.chars().count() + 2,
        ));
        let line = Line::from(vec![
            Span::styled(format!("{icon} "), style),
            Span::styled(fitted, style),
            Span::raw(gap),
            Span::styled(format!(" {age}"), Style::default().fg(theme::muted())),
        ]);
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(area.x + 1, y, area.width.saturating_sub(2), 1),
        );
    }
}

fn draw_activity(frame: &mut Frame<'_>, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(Block::default().style(theme::base()), area);
    let usage_height = if area.height >= 6 { 3 } else { 0 };
    let content_height = area.height.saturating_sub(usage_height);
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Min(1),
    ])
    .split(Rect::new(area.x, area.y, area.width, content_height));
    let muted = Style::default().fg(theme::muted());
    let active = Style::default()
        .fg(theme::text())
        .bg(theme::selected_color())
        .add_modifier(ACTIVE);
    let items = [
        (
            "▣  Verification: accept a 3.2 km floor?",
            "Landing",
            "4m",
            true,
        ),
        ("▤  MM305 crosswind", "", "18s", false),
        ("▣  Landing", "2 running", "26m", false),
        ("▤  TUI shell direction", "", "3m", false),
    ];
    for (index, (title, context, age, selected)) in items.into_iter().enumerate() {
        let row = rows[index];
        if selected {
            frame.render_widget(
                Block::default().style(Style::default().bg(theme::selected_color())),
                row,
            );
        }
        let suffix = if context.is_empty() {
            age.to_owned()
        } else {
            format!("{context}  {age}")
        };
        let available = row.width as usize;
        let title_width = available.saturating_sub(suffix.chars().count() + 1);
        let title = super::super::task::fit(title, title_width);
        let gap =
            " ".repeat(available.saturating_sub(title.chars().count() + suffix.chars().count()));
        let line = Line::from(vec![
            Span::styled(title, if selected { active } else { muted }),
            Span::raw(gap),
            Span::styled(suffix, muted),
        ]);
        frame.render_widget(Paragraph::new(line), Rect::new(row.x, row.y, row.width, 1));
    }
    let changes_y = rows[5].y;
    if changes_y < area.y.saturating_add(content_height) {
        let changes: Vec<Line<'static>> = vec![
            Line::from(vec![
                Span::styled("  C  guidance_taem.c", muted),
                Span::styled(
                    "                       +18",
                    Style::default().fg(theme::success()),
                ),
                Span::styled("  -6", Style::default().fg(theme::error())),
            ]),
            Line::from(vec![
                Span::styled("  C  taem_candidate_search.c", muted),
                Span::styled("           +42", Style::default().fg(theme::success())),
                Span::styled("  -13", Style::default().fg(theme::error())),
            ]),
            Line::from(vec![
                Span::styled("  C  taem_candidate_search.h", muted),
                Span::styled("            +8", Style::default().fg(theme::success())),
                Span::styled("  -4", Style::default().fg(theme::error())),
            ]),
            Line::from(Span::styled("  ▣  #214  Context rail focus order", muted)),
            Line::from(Span::styled(
                "  ▣  #219  Final-speed handoff threshold",
                muted,
            )),
        ];
        let change_area = Rect::new(
            area.x,
            changes_y,
            area.width,
            area.y
                .saturating_add(content_height)
                .saturating_sub(changes_y),
        );
        frame.render_widget(
            Paragraph::new(changes).wrap(Wrap { trim: false }),
            change_area,
        );
    }
    if usage_height > 0 {
        let y = area.bottom().saturating_sub(usage_height);
        let provider_lines: Vec<Line<'static>> = vec![
            Line::from(vec![
                Span::styled("anthropic", muted),
                Span::styled(
                    "     5h  ▰▰▰▱▱▱▱▱  62%",
                    Style::default().fg(theme::text_dim()),
                ),
                Span::styled(
                    "     week  ▰▰▱▱▱▱▱▱  31%",
                    Style::default().fg(theme::text_dim()),
                ),
            ]),
            Line::from(vec![
                Span::styled("openai", muted),
                Span::styled(
                    "        5h  ▰▱▱▱▱▱▱▱  12%",
                    Style::default().fg(theme::text_dim()),
                ),
                Span::styled(
                    "     week  ▰▱▱▱▱▱▱▱   4%",
                    Style::default().fg(theme::text_dim()),
                ),
            ]),
        ];
        frame.render_widget(
            Paragraph::new(provider_lines),
            Rect::new(
                area.x + 5,
                y,
                area.width.saturating_sub(10),
                2.min(usage_height),
            ),
        );
    }
}

fn draw_inspector(frame: &mut Frame<'_>, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::surface_color())),
        area,
    );
    let pad = 2.min(area.width);
    let content = Rect::new(
        area.x + pad,
        area.y + 2.min(area.height),
        area.width.saturating_sub(pad + 1),
        area.height.saturating_sub(4),
    );
    let title = Style::default().fg(theme::text()).add_modifier(ACTIVE);
    let secondary = Style::default().fg(theme::muted());
    let body = Style::default().fg(theme::text_dim());
    let text = vec![
        Line::from(Span::styled("Accept a 3.2 km floor?", title)),
        Line::raw(""),
        Line::from(Span::styled("Verification · Landing · 4m ago", secondary)),
        Line::raw(""),
        Line::from(Span::styled(
            "Sweep at 3 km misses Final speed for headings above 270°.",
            body,
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "Tightest flyable radius is 3.2 km; nominal is 12 km.",
            body,
        )),
        Line::raw(""),
        Line::from(Span::styled("a   Accept 3.2 km floor", title)),
        Line::raw(""),
        Line::from(Span::styled("b   Keep 3.0 km and add margin", body)),
        Line::raw(""),
        Line::from(Span::styled("c   Have Planner re-sweep first", body)),
    ];
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), content);
}

fn draw_compact(frame: &mut Frame<'_>, app: &mut App, bounds: Rect) {
    let status_height = 1.min(bounds.height);
    let content_height = bounds.height.saturating_sub(status_height);
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(status_height),
    ])
    .split(bounds);
    let tabs = format!("⌂ Home  ·  {}", app.state.active_model);
    frame.render_widget(
        Paragraph::new(super::super::task::fit(&tabs, rows[0].width as usize)).style(
            Style::default()
                .fg(theme::text())
                .bg(theme::code_background()),
        ),
        rows[0],
    );
    // A compact terminal has no room for desktop tabs, context rail and split
    // inspector at once. Preserve the interaction order with a full-width work
    // surface and composer at the bottom.
    draw_activity(frame, rows[1]);
    draw_composer(frame, app, rows[2]);
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
