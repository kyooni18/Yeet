//! Home workbench view: recent objects, workspace activity, and focused details.
use super::super::{
    components::status,
    support::{icons, theme},
};
use crate::app::App;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    prelude::{Color, Line, Span, Style},
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
    // OpenPencil's SVG group transform (-36 px) moves the title bar out of the
    // 864 px artboard; tabs therefore occupy rows 0..34 and the rail starts at 34.
    let scale_x = |px: u16| bounds.width as u32 * px as u32 / 1440;
    // Round design-pixel heights to the nearest terminal row. Ceiling here adds
    // an entire cell to the tab bar at 54 rows and shifts every pane downward.
    let scale_y = |px: u16| (bounds.height as u32 * px as u32 + 432) / 864;
    let tab_height = scale_y(34).max(1) as u16;
    let composer_height = scale_y(36).max(1) as u16;
    let status_height = scale_y(24).max(1) as u16;
    let rows = Layout::vertical([
        Constraint::Length(tab_height),
        Constraint::Min(1),
        Constraint::Length(composer_height),
        Constraint::Length(status_height),
    ])
    .split(bounds);
    draw_tabs(frame, rows[0]);
    let body = Rect::new(
        bounds.x,
        bounds.y.saturating_add(tab_height),
        bounds.width,
        bounds
            .height
            .saturating_sub(tab_height + composer_height + status_height),
    );
    let rail_width = scale_x(232).min(body.width as u32 / 3) as u16;
    let rail_gap = scale_x(30).min(body.width.saturating_sub(rail_width) as u32) as u16;
    let columns = Layout::horizontal([
        Constraint::Length(rail_width),
        Constraint::Length(rail_gap),
        Constraint::Min(1),
    ])
    .split(body);
    draw_sessions_rail(frame, columns[0]);
    let activity_width = scale_x(540).min(columns[2].width as u32) as u16;
    let pane_gap = scale_x(54).min(columns[2].width.saturating_sub(activity_width) as u32) as u16;
    let panes = Layout::horizontal([
        Constraint::Length(activity_width),
        Constraint::Length(pane_gap),
        Constraint::Min(1),
    ])
    .split(columns[2]);
    draw_activity(frame, panes[0]);
    draw_inspector(frame, panes[2]);
    draw_composer(frame, app, rows[2]);
    status::draw(frame, app, rows[3]);
}
fn draw_tabs(frame: &mut Frame<'_>, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let entries = [
        (icons::home(), "Home", true, 0u16),
        (icons::session_tab(), "MM305 crosswind tuning", false, 175),
        (icons::folder(false), "theme.rs", false, 423),
        (icons::agent_tab(), "Landing", false, 547),
        (icons::diff_tab(), "guidance_taem.c", false, 653),
        (icons::issue_tab(), "#214", false, 835),
        ("", "+", false, 916),
    ];
    let ends = [160u16, 423, 547, 653, 835, 895, 964];
    for (index, (icon, label, active, start)) in entries.into_iter().enumerate() {
        let start_px = (area.width as u32 * start as u32 / 1440) as u16;
        let end_px = (area.width as u32 * ends[index] as u32 / 1440) as u16;
        let x = area.x.saturating_add(start_px);
        if x >= area.right() {
            break;
        }
        let tab_right = area.x.saturating_add(end_px).min(area.right());
        let width = tab_right.saturating_sub(x);
        let style = if active {
            Style::default()
                .fg(theme::text())
                .bg(theme::surface_color())
                .add_modifier(ACTIVE)
        } else {
            Style::default().fg(theme::muted()).bg(theme::background())
        };
        frame.render_widget(
            Block::default().style(style),
            Rect::new(x, area.y, width, area.height),
        );
        if !icon.is_empty() {
            let icon_offset = if index == 0 { 15 } else { 0 };
            let icon_x = x + (area.width as u32 * icon_offset / 1440) as u16;
            frame.render_widget(
                Paragraph::new(icon).style(style),
                Rect::new(
                    icon_x,
                    area.y,
                    2.min(area.right().saturating_sub(icon_x)),
                    1,
                ),
            );
        }
        let label_offset = if index == 0 { 36 } else { 21 };
        let label_x = x + (area.width as u32 * label_offset / 1440) as u16;
        frame.render_widget(
            Paragraph::new(label).style(style),
            Rect::new(label_x, area.y, tab_right.saturating_sub(label_x), 1),
        );
    }
    frame.render_widget(
        Paragraph::new("─".repeat(area.width as usize)).style(Style::default().fg(theme::border())),
        Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
    );
}

fn draw_sessions_rail(frame: &mut Frame<'_>, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let px = |value: u16| (value * area.width + 116) / 232;
    let py = |value: u16| (value * area.height + 403) / 806;
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(0x1f, 0x1f, 0x28))),
        area,
    );
    let button = Rect::new(area.x + px(8), area.y + py(12), px(216), py(34).max(1));
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(0x2a, 0x2a, 0x37))),
        button,
    );
    frame.render_widget(
        Paragraph::new("+").style(Style::default().fg(Color::Rgb(0xd7, 0xd7, 0xa7))),
        Rect::new(
            button.x + button.width / 2,
            button.y,
            1.min(button.width),
            1.min(button.height),
        ),
    );
    let entries = [
        ("/", "MM305 crosswind", "18s", true),
        ("?", "TUI shell direction", "3m", false),
        ("●", "Runtime MCP attach", "9m", false),
        ("·", "Provider cleanup", "21m", false),
        ("·", "Foundation memory", "43m", false),
    ];
    let age_px = [198u16, 206, 206, 198, 198];
    let title_width_px = [166u16, 174, 174, 166, 166];
    for (index, (icon, title, age, selected)) in entries.into_iter().enumerate() {
        let y = area.y + py(52 + index as u16 * 28);
        if y >= area.bottom() {
            break;
        }
        let row = Rect::new(area.x, y, area.width, py(28).max(1));
        if selected {
            frame.render_widget(
                Block::default().style(Style::default().bg(Color::Rgb(0x25, 0x26, 0x33))),
                row,
            );
        }
        let fg = if selected {
            Color::Rgb(0xd7, 0xd7, 0xa7)
        } else {
            Color::Rgb(0xa6, 0xa6, 0x9c)
        };
        let icon_x = area.x + px(if index == 0 { 13 } else { 12 });
        let title_x = area.x + px(30);
        let age_x = area.x + px(age_px[index]);
        let title_width = px(title_width_px[index]);
        frame.render_widget(
            Paragraph::new(icon).style(Style::default().fg(fg)),
            Rect::new(icon_x, y, 1, 1),
        );
        frame.render_widget(
            Paragraph::new(super::super::task::fit(title, title_width as usize)).style(
                Style::default().fg(fg).add_modifier(if selected {
                    ACTIVE
                } else {
                    Modifier::empty()
                }),
            ),
            Rect::new(title_x, y, title_width, 1),
        );
        frame.render_widget(
            Paragraph::new(age).style(Style::default().fg(Color::Rgb(0x72, 0x71, 0x69))),
            Rect::new(age_x, y, age.len() as u16, 1),
        );
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
    if content_height >= 10 {
        draw_provider_usage(frame, area);
    }
}

fn draw_provider_usage(frame: &mut Frame<'_>, area: Rect) {
    if area.width == 0 || area.height < 3 {
        return;
    }
    let label = Style::default().fg(theme::muted());
    let value = Style::default().fg(theme::text_dim());
    let track = Style::default().bg(Color::Rgb(0x45, 0x47, 0x60));
    let fill = Style::default().bg(Color::Rgb(0x7e, 0x9c, 0xd8));
    let at = |px: u16| area.x + area.width * px / 540;
    for (row, provider, usage, week_usage) in [
        (0u16, "anthropic", "62%", "31%"),
        (2, "openai", "12%", "4%"),
    ] {
        let y = area.bottom().saturating_sub(4).saturating_add(row);
        if y >= area.bottom() {
            continue;
        }
        let mut put = |x: u16, text: &'static str, style: Style| {
            if x < area.right() {
                frame.render_widget(
                    Paragraph::new(text).style(style),
                    Rect::new(x, y, area.right() - x, 1),
                );
            }
        };
        put(at(24), provider, label);
        put(at(140), "5h", label);
        put(at(272), usage, value);
        put(at(352), "week", label);
        put(at(508), week_usage, value);
        for (start, percent) in [
            (166u16, if row == 0 { 62 } else { 12 }),
            (400u16, if row == 0 { 31 } else { 4 }),
        ] {
            let x = at(start);
            let width = (area.width * 96 / 540).min(area.right().saturating_sub(x));
            let meter_y = y.saturating_add(1);
            if width > 0 && meter_y < area.bottom() {
                frame.render_widget(
                    Paragraph::new(" ".repeat(width as usize)).style(track),
                    Rect::new(x, meter_y, width, 1),
                );
                let filled = width * percent / 100;
                if filled > 0 {
                    frame.render_widget(
                        Paragraph::new(" ".repeat(filled as usize)).style(fill),
                        Rect::new(x, meter_y, filled, 1),
                    );
                }
            }
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
