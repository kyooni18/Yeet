//! Full-screen keyboard file browser (`Mode::Files`).
use super::{icons, tabbar, task::fit, theme};
use crate::app::{
    App,
    files::FilesState,
    keymap::{Action, Context},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    prelude::{Line, Modifier, Span, Style},
    widgets::{Block, Paragraph},
};
use std::time::SystemTime;

const HINT_ACTIONS: [Action; 7] = [
    Action::ToggleInfo,
    Action::ToggleDiff,
    Action::ToggleChangedOnly,
    Action::Find,
    Action::ParentFolder,
    Action::OpenAsTab,
    Action::FocusInput,
];

pub(super) fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    frame.render_widget(Block::default().style(theme::base()), area);
    let Some(files) = app.files.as_ref() else {
        return;
    };
    if area.height < 4 || area.width < 8 {
        return;
    }
    let hints = if files.hints {
        app.keymap.hints(Context::Files, &HINT_ACTIONS)
    } else {
        Vec::new()
    };
    let wide = area.width >= 100;
    let info_height = if files.info && !wide { 4 } else { 0 };
    let hints_height = hints.len() as u16;
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(info_height),
        Constraint::Length(hints_height),
        Constraint::Length(1),
    ])
    .split(area);

    let active = match files.active_tab {
        Some(index) => tabbar::Active::File(index),
        None => tabbar::Active::Home,
    };
    tabbar::draw(frame, app, rows[0], active);
    draw_location(frame, files, rows[1]);
    if files.diff {
        draw_diff(frame, files, rows[2]);
    } else if files.info && wide {
        let list_width = (rows[2].width * 2 / 5).clamp(30, 60);
        let [list, gap, inspector] = Layout::horizontal([
            Constraint::Length(list_width),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .areas(rows[2]);
        draw_list(frame, files, list);
        for y in gap.y..gap.bottom() {
            frame.render_widget(
                Paragraph::new(Line::styled("│", Style::default().fg(theme::border_dim()))),
                Rect::new(gap.x, y, 1, 1),
            );
        }
        draw_info(frame, files, inspector);
    } else {
        draw_list(frame, files, rows[2]);
    }
    if info_height > 0 {
        frame.render_widget(Block::default().style(theme::surface()), rows[3]);
        draw_info(frame, files, rows[3]);
    }
    if !hints.is_empty() {
        draw_hints(frame, &hints, rows[4]);
    }
    draw_status(frame, app, files, rows[5]);
}

fn draw_location(frame: &mut Frame<'_>, files: &FilesState, area: Rect) {
    let line = if let Some(query) = &files.find {
        Line::from(vec![
            Span::styled(" / ", Style::default().fg(theme::accent())),
            Span::styled(query.clone(), Style::default().fg(theme::text())),
            Span::styled("▌", Style::default().fg(theme::accent())),
        ])
    } else {
        let parts: Vec<String> = files
            .dir
            .components()
            .rev()
            .take(3)
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .filter(|p| p != "/")
            .collect();
        let path = parts.into_iter().rev().collect::<Vec<_>>().join(" / ");
        Line::styled(
            format!(" {}", fit(&path, area.width.saturating_sub(2) as usize)),
            Style::default().fg(theme::text_dim()),
        )
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_list(frame: &mut Frame<'_>, files: &FilesState, area: Rect) {
    let visible = files.visible();
    let height = area.height as usize;
    let offset = files.cursor.saturating_sub(height.saturating_sub(1));
    for (row, (index, entry)) in visible
        .iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .enumerate()
        .map(|(r, x)| (r, x))
    {
        let selected = index == files.cursor;
        let changed = files.is_changed(entry);
        let icon = format!(
            "{} ",
            if entry.is_dir {
                icons::folder(selected)
            } else {
                icons::file(&entry.name)
            }
        );
        let name = if entry.is_dir {
            format!("{}/", entry.name)
        } else {
            entry.name.clone()
        };
        let color = if changed {
            theme::warning()
        } else if entry.is_dir {
            theme::accent()
        } else {
            theme::text()
        };
        let mut style = Style::default().fg(color);
        if selected {
            style = style.add_modifier(Modifier::BOLD);
        }
        let row_area = Rect::new(area.x, area.y + row as u16, area.width, 1);
        if selected {
            frame.render_widget(
                Block::default().style(Style::default().bg(theme::surface_raised())),
                row_area,
            );
        }
        let marker = if selected { "▌" } else { " " };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(marker, Style::default().fg(theme::accent())),
                Span::styled(format!(" {icon}"), Style::default().fg(theme::muted())),
                Span::styled(fit(&name, area.width.saturating_sub(4) as usize), style),
            ])),
            row_area,
        );
    }
}

fn draw_diff(frame: &mut Frame<'_>, files: &FilesState, area: Rect) {
    let lines: Vec<Line> = files
        .diff_lines
        .iter()
        .take(area.height as usize)
        .map(|line| {
            let color = match line.chars().next() {
                Some('+') if !line.starts_with("+++") => theme::success(),
                Some('-') if !line.starts_with("---") => theme::error(),
                Some('@') => theme::accent(),
                _ => theme::text_dim(),
            };
            Line::styled(
                format!(" {}", fit(line, area.width.saturating_sub(2) as usize)),
                Style::default().fg(color),
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_info(frame: &mut Frame<'_>, files: &FilesState, area: Rect) {
    let Some(entry) = files.selected() else {
        return;
    };
    let status = files
        .changed
        .get(&entry.name)
        .cloned()
        .unwrap_or_else(|| "clean".to_owned());
    let size = if entry.is_dir {
        let count = std::fs::read_dir(files.dir.join(&entry.name)).map_or(0, Iterator::count);
        format!("{count} items")
    } else {
        human_size(entry.size)
    };
    let values = [
        entry.name.clone(),
        size,
        entry.modified.map(age).unwrap_or_default(),
        status,
    ];
    let lines: Vec<Line> = values
        .into_iter()
        .map(|v| {
            Line::styled(
                format!(" {}", fit(&v, area.width.saturating_sub(2) as usize)),
                Style::default().fg(theme::text_dim()),
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_hints(frame: &mut Frame<'_>, hints: &[(String, &'static str)], area: Rect) {
    frame.render_widget(Block::default().style(theme::surface()), area);
    let lines: Vec<Line> = hints
        .iter()
        .map(|(key, label)| {
            Line::from(vec![
                Span::styled(
                    format!(" {key:<7}"),
                    Style::default()
                        .fg(theme::accent())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(*label, Style::default().fg(theme::text_dim())),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_status(frame: &mut Frame<'_>, app: &App, files: &FilesState, area: Rect) {
    frame.render_widget(Block::default().style(theme::surface()), area);
    let cue = app
        .keymap
        .keys_for(Context::Files, Action::ToggleHints)
        .into_iter()
        .next()
        .map(|key| format!("{key} hints"))
        .unwrap_or_default();
    let cue_width = cue.chars().count() as u16 + 1;
    let path = files.dir.display().to_string();
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!(
                " {}",
                fit(&path, area.width.saturating_sub(cue_width + 2) as usize)
            ),
            Style::default().fg(theme::muted()),
        )),
        area,
    );
    if area.width > cue_width {
        frame.render_widget(
            Paragraph::new(Line::styled(cue, Style::default().fg(theme::muted()))),
            Rect::new(area.right() - cue_width, area.y, cue_width, 1),
        );
    }
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / f64::from(1 << 20))
    } else if bytes >= 1 << 10 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn age(time: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(time)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match secs {
        0..=59 => "now".to_owned(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}
