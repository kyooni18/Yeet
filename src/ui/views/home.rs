use super::super::{
    components::{composer, status, tabbar},
    support::{icons, theme},
};
use crate::app::App;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    prelude::{Color, Line, Span, Style},
    style::Modifier,
    widgets::{Block, Paragraph, Wrap},
};

const ACTIVE: Modifier = Modifier::BOLD;

pub(crate) fn draw(frame: &mut Frame<'_>, app: &mut App) {
    app.transcript_area = (0, 0, 0, 0);
    app.sidebar_area = (0, 0, 0, 0);
    app.sidebar_session_targets.clear();
    let bounds = frame.area();
    if bounds.width < 45 || bounds.height < 5 {
        return;
    }
    if bounds.width < 110 {
        draw_compact(frame, app, bounds);
        return;
    }
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::code_background())),
        bounds,
    );
    let scale_x = |px: u16| bounds.width as u32 * px as u32 / 1440;
    let scale_y = |px: u16| (bounds.height as u32 * px as u32 + 432) / 864;
    let tab_height = 1;
    let composer_height = scale_y(36).max(1) as u16;
    let status_height = scale_y(24).max(1) as u16;
    let rows = Layout::vertical([
        Constraint::Length(tab_height),
        Constraint::Min(1),
        Constraint::Length(composer_height),
        Constraint::Length(status_height),
    ])
    .split(bounds);
    tabbar::draw(frame, app, rows[0], tabbar::Active::Home);
    let body = Rect::new(
        bounds.x,
        bounds.y.saturating_add(tab_height),
        bounds.width,
        bounds
            .height
            .saturating_sub(tab_height + composer_height + status_height),
    );
    let columns = Layout::horizontal([Constraint::Length(scale_x(232) as u16), Constraint::Min(1)])
        .split(body);
    draw_sessions_rail(
        frame,
        Rect::new(
            columns[0].x,
            columns[0].y,
            columns[0].width,
            columns[0].height + composer_height,
        ),
    );
    let activity_width = scale_x(540).min(columns[1].width as u32) as u16;
    let content_inset = scale_x(30) as u16;
    let inspector_x = bounds.x + scale_x(832) as u16;
    let inspector = Rect::new(
        inspector_x,
        body.y,
        bounds.right().saturating_sub(inspector_x),
        body.height,
    );
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::background())),
        inspector,
    );
    let inspector_inset = scale_x(24) as u16;
    draw_activity(
        frame,
        Rect::new(
            columns[1].x + content_inset,
            body.y,
            activity_width,
            body.height,
        ),
    );
    draw_inspector(
        frame,
        app,
        Rect::new(
            inspector.x + inspector_inset,
            inspector.y,
            inspector.width.saturating_sub(inspector_inset * 2),
            inspector.height,
        ),
    );
    draw_composer(
        frame,
        app,
        Rect::new(
            bounds.x + scale_x(232) as u16,
            rows[2].y,
            bounds.width.saturating_sub(scale_x(232) as u16),
            rows[2].height,
        ),
    );
    status::draw(frame, app, rows[3]);
}

fn draw_compact(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::background())),
        area,
    );
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);
    tabbar::draw(frame, app, rows[0], tabbar::Active::Home);
    let title = Line::from(vec![
        Span::styled(
            " Overview",
            Style::default().fg(theme::text()).add_modifier(ACTIVE),
        ),
        Span::styled("  •  Yeet workspace", Style::default().fg(theme::muted())),
    ]);
    let body = Paragraph::new(vec![
        title,
        Line::from(""),
        Line::from(Span::styled(
            "Now",
            Style::default().fg(theme::text()).add_modifier(ACTIVE),
        )),
        Line::from("  Verification: accept a 3.2 km floor?"),
        Line::from("  Landing · 4m ago"),
        Line::from(""),
        Line::from(Span::styled(
            "Recent Activity",
            Style::default().fg(theme::text()).add_modifier(ACTIVE),
        )),
        Line::from("  MM305 crosswind tuning · 18s"),
        Line::from("  Landing · 2 running · 26m"),
        Line::from("  TUI shell direction · 3m"),
        Line::from(""),
        Line::from(Span::styled(
            "Changed Files",
            Style::default().fg(theme::text()).add_modifier(ACTIVE),
        )),
        Line::from("  guidance_taem.c  +18  −6"),
        Line::from("  taem_candidate_search.c  +42  −13"),
        Line::from("  taem_candidate_search.h  +8  −4"),
    ])
    .wrap(Wrap { trim: true });
    frame.render_widget(body, rows[1]);
    draw_composer(frame, app, rows[2]);
    status::draw(frame, app, rows[3]);
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
    for (index, (icon, title, age, selected)) in entries.into_iter().enumerate() {
        let row_y = area.y + py(52).max(2) + index as u16 * py(28).max(1);
        if row_y >= area.bottom() {
            break;
        }
        let height = py(28).max(1).min(area.bottom() - row_y);
        let row = Rect::new(area.x, row_y, area.width, height);
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
        let show_age = area.width >= 23;
        let age_width = if show_age { age.len() as u16 } else { 0 };
        let age_x = area.right().saturating_sub(age_width + 1);
        let title_width = age_x.saturating_sub(title_x + u16::from(show_age));
        frame.render_widget(
            Paragraph::new(icon).style(Style::default().fg(fg)),
            Rect::new(icon_x, row_y, 1, 1),
        );
        frame.render_widget(
            Paragraph::new(super::super::task::fit(title, title_width as usize)).style(
                Style::default().fg(fg).add_modifier(if selected {
                    ACTIVE
                } else {
                    Modifier::empty()
                }),
            ),
            Rect::new(title_x, row_y, title_width, 1),
        );
        if show_age {
            frame.render_widget(
                Paragraph::new(age).style(Style::default().fg(Color::Rgb(0x72, 0x71, 0x69))),
                Rect::new(age_x, row_y, age_width, 1),
            );
        }
    }
}

fn draw_activity(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::code_background())),
        area,
    );
    let step = ((area.height as u32 * 28 + 385) / 770).max(1) as u16;
    let body = Style::default().fg(Color::Rgb(0xa6, 0xa6, 0x9c));
    let muted = Style::default().fg(theme::muted());
    let selected_bg = Color::Rgb(0x25, 0x26, 0x33);
    let at = |px: u16| area.x + (area.width as u32 * px as u32 / 540) as u16;
    let label_x = at(52).max(area.x + 4);
    let icon_x = at(24).max(area.x + 2);
    let context_x = at(330);
    let age_x = at(422);
    let bottom = area.bottom().saturating_sub(5);
    let put = |frame: &mut Frame<'_>, x: u16, y: u16, text: &str, style: Style, width: u16| {
        frame.render_widget(Paragraph::new(text).style(style), Rect::new(x, y, width, 1));
    };
    for (index, (icon, title, context, age)) in [
        (
            icons::agent_tab(),
            "Verification: accept a 3.2 km floor?",
            "Landing",
            "4m",
        ),
        (icons::session_tab(), "MM305 crosswind", "", "18s"),
        (icons::agent_tab(), "Landing", "2 running", "26m"),
        (icons::session_tab(), "TUI shell direction", "", "3m"),
    ]
    .into_iter()
    .enumerate()
    {
        let y = area.y + 2 + index as u16 * step;
        if y >= bottom {
            break;
        }
        let selected = index == 0;
        let style = if selected {
            Style::default().fg(theme::text()).bg(selected_bg)
        } else {
            body
        };
        if selected {
            frame.render_widget(
                Block::default().style(Style::default().bg(selected_bg)),
                Rect::new(area.x, y, area.width, 1),
            );
        }
        let title_len = title.chars().count() as u16;
        let show_context = !context.is_empty() && label_x + title_len + 2 <= context_x;
        let show_age = label_x + title_len + 1 <= age_x;
        let end = if show_context {
            context_x.saturating_sub(2)
        } else if show_age {
            age_x.saturating_sub(1)
        } else {
            area.right()
        };
        put(frame, icon_x, y, icon, style, 1);
        put(
            frame,
            label_x,
            y,
            &super::super::task::fit(title, end.saturating_sub(label_x) as usize),
            style,
            end.saturating_sub(label_x),
        );
        if show_context {
            put(
                frame,
                context_x,
                y,
                context,
                muted,
                age_x.saturating_sub(context_x + 1),
            );
        }
        if show_age {
            put(
                frame,
                age_x,
                y,
                age,
                muted,
                area.right().saturating_sub(age_x),
            );
        }
    }
    for (index, (file, added, removed)) in [
        ("guidance_taem.c", "+18", "−6"),
        ("taem_candidate_search.c", "+42", "−13"),
        ("taem_candidate_search.h", "+8", "−4"),
    ]
    .into_iter()
    .enumerate()
    {
        let y = area.y + (8 + index as u16) * step;
        if y >= bottom {
            break;
        }
        let added_x = context_x.max(label_x + 8);
        let removed_x = at(374).max(added_x + 4);
        let fitted = super::super::task::fit(file, added_x.saturating_sub(label_x + 2) as usize);
        put(
            frame,
            icon_x,
            y,
            icons::file(file),
            Style::default().fg(theme::accent()),
            1,
        );
        put(
            frame,
            label_x,
            y,
            &fitted,
            body,
            added_x.saturating_sub(label_x + 1),
        );
        put(
            frame,
            added_x,
            y,
            added,
            Style::default().fg(Color::Rgb(0x76, 0x94, 0x6a)),
            3,
        );
        put(
            frame,
            removed_x,
            y,
            removed,
            Style::default().fg(Color::Rgb(0xc3, 0x40, 0x43)),
            3,
        );
    }
    for (index, issue) in [
        "#214  Context rail focus order",
        "#219  Final-speed handoff threshold",
    ]
    .into_iter()
    .enumerate()
    {
        let y = area.y + (12 + index as u16) * step;
        if y >= bottom {
            break;
        }
        put(frame, icon_x, y, icons::issue_tab(), body, 1);
        put(
            frame,
            label_x,
            y,
            &super::super::task::fit(issue, area.right().saturating_sub(label_x) as usize),
            body,
            area.right().saturating_sub(label_x),
        );
    }
    if area.height >= 12 {
        draw_provider_usage(frame, area);
    }
}

fn draw_provider_usage(frame: &mut Frame<'_>, area: Rect) {
    if area.width == 0 || area.height < 3 {
        return;
    }
    let label = Style::default().fg(theme::muted());
    let value = Style::default().fg(theme::text_dim());
    let track = Style::default().fg(Color::Rgb(0x45, 0x47, 0x60));
    let fill = Style::default().fg(Color::Rgb(0x7e, 0x9c, 0xd8));
    let at = |px: u16| area.x + area.width * px / 540;
    for (row, provider, usage, week_usage) in [
        (0u16, "anthropic", "62%", "31%"),
        (1, "openai", "12%", "4%"),
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
        put(at(322), "week", label);
        put(at(478), week_usage, value);
        for (start, percent) in [
            (166u16, if row == 0 { 62 } else { 12 }),
            (370u16, if row == 0 { 31 } else { 4 }),
        ] {
            let x = at(start);
            let width = (area.width * 96 / 540).min(area.right().saturating_sub(x));
            let meter_y = y;
            if width > 0 && meter_y < area.bottom() {
                frame.render_widget(
                    Paragraph::new("━".repeat(width as usize)).style(track),
                    Rect::new(x, meter_y, width, 1),
                );
                let filled = (width * percent).div_ceil(100);
                if filled > 0 {
                    frame.render_widget(
                        Paragraph::new("━".repeat(filled as usize)).style(fill),
                        Rect::new(x, meter_y, filled, 1),
                    );
                }
            }
        }
    }
}

fn draw_inspector(frame: &mut Frame<'_>, _app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let step = ((area.height as u32 * 28 + 385) / 770).max(1) as u16;
    let heading = Style::default()
        .fg(Color::Rgb(0xd7, 0xd7, 0xa7))
        .add_modifier(ACTIVE);
    let body = Style::default().fg(Color::Rgb(0xa6, 0xa6, 0x9c));
    let subdued = Style::default().fg(Color::Rgb(0x72, 0x71, 0x69));
    let put = |frame: &mut Frame<'_>, x: u16, y: u16, text: &str, style: Style| {
        let x = area.x.saturating_add(x);
        let y = area.y.saturating_add(y);
        if x < area.right() && y < area.bottom() {
            frame.render_widget(
                Paragraph::new(super::super::task::fit(
                    text,
                    area.right().saturating_sub(x) as usize,
                ))
                .style(style),
                Rect::new(x, y, area.right().saturating_sub(x), 1),
            );
        }
    };

    put(frame, 0, 2, "Accept a 3.2 km floor?", heading);
    put(
        frame,
        0,
        2 + step,
        "Verification · Landing · 4m ago",
        subdued,
    );
    let copy_y = area.y + 6 * step;
    let copy = Paragraph::new("Sweep at 3 km misses Final speed for headings above 270°.\nTightest flyable radius is 3.2 km; nominal is 12 km.")
        .style(body).wrap(Wrap { trim: true });
    let copy_height = copy.line_count(area.width) as u16;
    let choices_y = (area.y + 10 * step).max(copy_y + copy_height + 1);
    frame.render_widget(
        copy,
        Rect::new(
            area.x,
            copy_y,
            area.width,
            copy_height.min(area.bottom().saturating_sub(copy_y)),
        ),
    );

    for (index, (key, label)) in [
        ("a", "Accept 3.2 km floor"),
        ("b", "Keep 3.0 km and add margin"),
        ("c", "Have Planner re-sweep first"),
    ]
    .into_iter()
    .enumerate()
    {
        let y = choices_y.saturating_sub(area.y) + index as u16 * step;
        if area.y + y < area.bottom() {
            frame.render_widget(
                Paragraph::new(format!("{key}   {label}")).style(if index == 0 {
                    heading.remove_modifier(ACTIVE)
                } else {
                    body
                }),
                Rect::new(area.x, area.y + y, area.width, 1),
            );
        }
    }
}

fn draw_composer(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    if area.height == 0 {
        return;
    }
    let inset = (area.width as u32 * 30 / 1208).max(2) as u16;
    composer::draw(frame, app, area, area.x + inset + 3);
    if app.input.is_empty() {
        let (x, y, width, height) = app.composer_area;
        frame.render_widget(
            Paragraph::new("Ask Yeet...").style(Style::default().fg(theme::text())),
            Rect::new(x, y, width, height),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn home_keeps_copy_and_change_counts_readable_after_resize() {
        for (width, height) in [(110, 28), (144, 44), (203, 41)] {
            let mut app = App::default();
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            let buffer = terminal.backend().buffer();
            let rows: Vec<String> = buffer
                .content
                .chunks(width as usize)
                .map(|row| row.iter().map(|cell| cell.symbol()).collect())
                .collect();
            let text = rows.join("\n");
            assert!(
                text.contains("Verification: accept a 3.2 km floor?"),
                "{width}x{height}"
            );
            assert!(
                text.contains("270°."),
                "inspector copy clipped at {width}x{height}"
            );
            assert!(
                text.contains("12 km."),
                "inspector copy clipped at {width}x{height}"
            );
            let changed: Vec<&String> = rows
                .iter()
                .filter(|row| row.contains("+18") || row.contains("+42") || row.contains("+8"))
                .collect();
            assert_eq!(changed.len(), 3);
            let column = |row: &str, needle: &str| {
                let byte = row.find(needle).unwrap();
                row[..byte].chars().count()
            };
            assert_eq!(column(changed[0], "+18"), column(changed[1], "+42"));
            assert_eq!(column(changed[1], "+42"), column(changed[2], "+8"));
        }
    }
}
