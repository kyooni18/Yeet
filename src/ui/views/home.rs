//! Home workbench view: recent objects, workspace activity, and focused details.
use super::super::{status, theme};
use crate::app::App;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    prelude::{Line, Span, Style},
    style::Modifier,
    widgets::{Block, Paragraph},
};

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

    // Desktop / Overview mockup geometry translated at ten pixels per terminal cell.
    let scale = |px: u16| (bounds.width as u32 * px as u32 / 1440) as u16;
    let tab_height = scale(34).max(1);
    let composer_height = scale(36).max(1);
    let status_height = scale(24).max(1);
    let rows = Layout::vertical([
        Constraint::Length(tab_height),
        Constraint::Min(1),
        Constraint::Length(composer_height),
        Constraint::Length(status_height),
    ])
    .split(bounds);
    draw_tabs(frame, rows[0]);

    let body = rows[1];
    let rail_width = scale(232).min(body.width / 3);
    let rail_gap = scale(30).min(body.width.saturating_sub(rail_width));
    let columns = Layout::horizontal([
        Constraint::Length(rail_width),
        Constraint::Length(rail_gap),
        Constraint::Min(1),
    ])
    .split(body);
    draw_sessions_rail(frame, columns[0]);

    let activity_width = scale(540).min(columns[2].width);
    let pane_gap = scale(60).min(columns[2].width.saturating_sub(activity_width));
    let panes = Layout::horizontal([
        Constraint::Length(activity_width),
        Constraint::Length(pane_gap),
        Constraint::Min(1),
    ])
    .split(columns[2]);
    draw_activity(frame, panes[0]);
    draw_inspector(frame, panes[2]);
    draw_usage(frame, panes[0]);

    draw_composer(frame, app, rows[2]);
    status::draw(frame, app, rows[3]);
}

fn draw_tabs(frame: &mut Frame<'_>, area: Rect) {
    if area.width == 0 {
        return;
    }
    let entries = [
        ("Home", "⌂ Home", true),
        ("MM305 crosswind tuning", "▤ MM305 crosswind tuning", false),
        ("theme.rs", "▱ theme.rs", false),
        ("Landing", "▣ Landing", false),
        ("guidance_taem.c", "⚙ guidance_taem.c", false),
        ("#214", "▣ #214", false),
        ("+", "+", false),
    ];
    // Pixel proportions from the mockup; unused width stays available after the final tab.
    let inset = (area.width as u32 * 16 / 1440) as u16;
    let tab_widths =
        [160u32, 248, 160, 96, 160, 72, 48].map(|px| (area.width as u32 * px / 1440) as u16);
    let mut x = area.x + inset;
    for (index, (_name, label, active)) in entries.into_iter().enumerate() {
        if x >= area.right() {
            break;
        }
        let width = tab_widths[index].min(area.right() - x);
        let style = if active {
            Style::default()
                .fg(theme::text())
                .bg(theme::surface_color())
                .add_modifier(ACTIVE)
        } else {
            Style::default()
                .fg(theme::muted())
                .bg(theme::code_background())
        };
        let text = super::super::task::fit(label, width.saturating_sub(4) as usize);
        frame.render_widget(
            Paragraph::new(text).style(style),
            Rect::new(x, area.y, width, area.height),
        );
        x += width;
    }
    frame.render_widget(
        Paragraph::new("─".repeat(area.width as usize)).style(Style::default().fg(theme::border())),
        Rect::new(area.x, area.y + 1, area.width, 1),
    );
}

fn draw_sessions_rail(frame: &mut Frame<'_>, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let entries = [
        ("/", "MM305 crosswind", "18s", true),
        ("?", "TUI shell direction", "3m", false),
        ("●", "Runtime MCP attach", "9m", false),
        ("·", "Provider cleanup", "21m", false),
        ("·", "Foundation memory", "43m", false),
    ];
    let button = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        2.min(area.height),
    );
    frame.render_widget(
        Paragraph::new(" + ").style(
            Style::default()
                .fg(theme::text())
                .bg(theme::selected_color()),
        ),
        button,
    );
    // Recent sessions are a compact list beneath the new-session control.
    // Keep rows grouped at the same two-cell rhythm as the 28px mockup rows.
    for (index, (icon, title, age, selected)) in entries.into_iter().enumerate() {
        let y = area.y.saturating_add(4 + index as u16 * 2);
        if y >= area.bottom() {
            break;
        }

        let age_width = age.chars().count();
        let title_width = area.width.saturating_sub(age_width as u16 + 5) as usize;
        let fitted = super::super::task::fit(title, title_width);
        let padding = " ".repeat(title_width.saturating_sub(fitted.chars().count()));
        let style = if selected {
            Style::default()
                .fg(theme::text())
                .bg(theme::selected_color())
                .add_modifier(ACTIVE)
        } else {
            Style::default().fg(theme::muted())
        };
        let line = Line::from(vec![
            Span::styled(format!("{icon} "), style),
            Span::styled(fitted, style),
            Span::raw(padding),
            Span::raw("  "),
            Span::styled(age, Style::default().fg(theme::muted())),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(style),
            Rect::new(area.x + 1, y, area.width.saturating_sub(2), 1),
        );
    }
}

fn draw_activity(frame: &mut Frame<'_>, area: Rect) {
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
        frame.render_widget(Paragraph::new(changes), change_area);
    }
}

fn draw_usage(frame: &mut Frame<'_>, area: Rect) {
    if area.width == 0 || area.height < 3 {
        return;
    }
    let muted = Style::default().fg(theme::muted());
    let provider_lines: Vec<Line<'static>> = vec![
        Line::from(vec![
            Span::styled("anthropic", muted),
            Span::styled(" 5h ▰▰▰▱▱ 62%", Style::default().fg(theme::text_dim())),
            Span::styled(" wk ▰▰▱▱ 31%", Style::default().fg(theme::text_dim())),
        ]),
        Line::from(vec![
            Span::styled("openai", muted),
            Span::styled(" 5h ▰▱▱▱▱ 12%", Style::default().fg(theme::text_dim())),
            Span::styled(" wk ▰▱▱▱ 4%", Style::default().fg(theme::text_dim())),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(provider_lines),
        Rect::new(
            area.x + 5.min(area.width),
            area.bottom().saturating_sub(3),
            area.width.saturating_sub(10),
            2,
        ),
    );
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
    frame.render_widget(Paragraph::new(text), content);
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
