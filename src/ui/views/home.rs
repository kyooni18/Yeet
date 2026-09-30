//! Home workbench view: recent objects, workspace activity, and focused details.
use super::super::{components::status, theme};
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

    frame.render_widget(
        Block::default().style(Style::default().bg(theme::background())),
        bounds,
    );
    let scale_x = |px: u16| (bounds.width as u32 * px as u32 / 1440) as u16;
    let scale_y = |px: u16| ((bounds.height as u32 * px as u32 + 863) / 864) as u16;
    let tab_height = scale_y(34).max(1);
    let composer_height = scale_y(36).max(1);
    let status_height = scale_y(24).max(1);
    let rows = Layout::vertical([
        Constraint::Length(tab_height),
        Constraint::Min(1),
        Constraint::Length(composer_height),
        Constraint::Length(status_height),
    ])
    .split(bounds);
    draw_tabs(frame, rows[0]);
    let body = rows[1];
    let rail_width = scale_x(232).min(body.width / 3);
    let columns =
        Layout::horizontal([Constraint::Length(rail_width), Constraint::Min(1)]).split(body);
    draw_sessions_rail(frame, columns[0]);
    let activity_width = scale_x(540).min(columns[1].width);
    let pane_gap = scale_x(54).min(columns[1].width.saturating_sub(activity_width));
    let panes = Layout::horizontal([
        Constraint::Length(activity_width),
        Constraint::Length(pane_gap),
        Constraint::Min(1),
    ])
    .split(columns[1]);
    draw_activity(frame, panes[0]);
    draw_inspector(frame, panes[2]);
    draw_composer(frame, app, rows[2]);
    status::draw(frame, app, rows[3]);
}

fn draw_tabs(frame: &mut Frame<'_>, area: Rect) {
    if area.width == 0 {
        return;
    }
    let entries = [
        ("Home", "Home", true),
        ("MM305 crosswind tuning", "MM305 crosswind tuning", false),
        ("theme.rs", "theme.rs", false),
        ("Landing", "Landing", false),
        ("guidance_taem.c", "guidance_taem.c", false),
        ("#214", "#214", false),
        ("+", "+", false),
    ];
    let widths = [160u32, 248, 124, 106, 182, 60, 48];
    let mut x = area.x;
    for (index, (_, label, active)) in entries.into_iter().enumerate() {
        if x >= area.right() {
            break;
        }
        let width = ((area.width as u32 * widths[index] + 1439) / 1440)
            .max(label.chars().count() as u32)
            .min((area.right() - x) as u32) as u16;
        let style = if active {
            Style::default()
                .fg(theme::text())
                .bg(theme::surface_color())
                .add_modifier(ACTIVE)
        } else {
            Style::default().fg(theme::muted()).bg(theme::background())
        };
        frame.render_widget(
            Paragraph::new(label).style(style),
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
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::surface_color())),
        area,
    );
    let entries = [
        ("/", "MM305 crosswind", "18s", true),
        ("?", "TUI shell direction", "3m", false),
        ("●", "Runtime MCP attach", "9m", false),
        ("·", "Provider cleanup", "21m", false),
        ("·", "Foundation memory", "43m", false),
    ];
    let add_button = format!("{:>width$}", "+", width = area.width.div_ceil(2) as usize);
    frame.render_widget(
        Paragraph::new(add_button).style(
            Style::default()
                .fg(theme::text())
                .bg(theme::surface_color()),
        ),
        Rect::new(area.x, area.y, area.width, 2.min(area.height)),
    );
    for (index, (icon, title, age, selected)) in entries.into_iter().enumerate() {
        let row_offsets = [2u16, 4, 5, 6, 8];
        let y = area.y.saturating_add(row_offsets[index]);
        if y >= area.bottom() {
            break;
        }
        let style = if selected {
            Style::default()
                .fg(theme::text())
                .bg(theme::selected_color())
                .add_modifier(ACTIVE)
        } else {
            Style::default().fg(theme::muted())
        };
        let prefix = "  ";
        let age_width = Span::raw(age).width();
        let prefix_width = Span::raw(prefix).width();
        let title = super::super::task::fit(
            title,
            (area.width as usize).saturating_sub(age_width + prefix_width + 4),
        );
        let used_width = prefix_width + Span::raw(&title).width() + age_width;
        let gap = " ".repeat((area.width as usize).saturating_sub(used_width + 2));
        let icon_style = if selected {
            Style::default().fg(theme::text()).add_modifier(ACTIVE)
        } else {
            Style::default().fg(theme::muted())
        };
        let line = Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(icon, icon_style),
            Span::raw(" "),
            Span::styled(title, style),
            Span::raw(gap),
            Span::styled(age, Style::default().fg(theme::muted())),
        ]);
        frame.render_widget(Paragraph::new(line), Rect::new(area.x, y, area.width, 1));
    }
}

fn draw_activity(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::background())),
        area,
    );
    let content_height = area.height;
    let muted = Style::default().fg(theme::muted());
    let active = Style::default()
        .fg(theme::text())
        .bg(theme::selected_color())
        .add_modifier(ACTIVE);
    let items = [
        (
            "Verification: accept a 3.2 km floor?",
            "Landing",
            "4m",
            true,
        ),
        ("▤  MM305 crosswind", "", "18s", false),
        ("▣  Landing", "2 running", "26m", false),
        ("▤  TUI shell direction", "", "3m", false),
    ];
    for (index, (title, context, age, selected)) in items.into_iter().enumerate() {
        let y = area.y.saturating_add(1 + index as u16 * 2);
        if y >= area.y.saturating_add(content_height) {
            break;
        }
        let row = Rect::new(area.x, y, area.width, 1);
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
        let title = super::super::task::fit(
            title,
            (row.width as usize).saturating_sub(suffix.chars().count() + 3),
        );
        let prefix = if selected { "    ▸ " } else { "      " };
        let used = prefix.chars().count() + title.chars().count() + suffix.chars().count();
        let gap = " ".repeat((row.width as usize).saturating_sub(used));
        let line = Line::from(vec![
            Span::styled(prefix, if selected { active } else { muted }),
            Span::styled(title, if selected { active } else { muted }),
            Span::raw(gap),
            Span::styled(suffix, muted),
        ]);
        frame.render_widget(Paragraph::new(line), row);
    }
    let files = [
        ("guidance_taem.c", "+18", "−6"),
        ("taem_candidate_search.c", "+42", "−13"),
        ("taem_candidate_search.h", "+8", "−4"),
    ];
    for (index, (file, added, removed)) in files.into_iter().enumerate() {
        let y = area.y.saturating_add(10 + index as u16 * 2);
        if y >= area.y.saturating_add(content_height) {
            break;
        }
        let file = super::super::task::fit(file, area.width.saturating_sub(13) as usize);
        let line = Line::from(vec![
            Span::styled(format!("  C   {file}          "), muted),
            Span::styled(added, Style::default().fg(theme::success())),
            Span::raw("  "),
            Span::styled(removed, Style::default().fg(theme::error())),
        ]);
        frame.render_widget(Paragraph::new(line), Rect::new(area.x, y, area.width, 1));
    }
    for (index, issue) in [
        "#214  Context rail focus order",
        "#219  Final-speed handoff threshold",
    ]
    .into_iter()
    .enumerate()
    {
        let y = area.y.saturating_add(17 + index as u16 * 2);
        if y < area.y.saturating_add(content_height) {
            let issue = super::super::task::fit(issue, area.width.saturating_sub(5) as usize);
            frame.render_widget(
                Paragraph::new(format!("  ▣  {issue}")).style(muted),
                Rect::new(area.x, y, area.width, 1),
            );
        }
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
