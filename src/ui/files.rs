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
    } else if wide {
        draw_wide_body(frame, files, rows[2]);
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
            Span::styled(
                if files.find_locked { "" } else { "▌" },
                Style::default().fg(theme::accent()),
            ),
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

fn centered_offset(cursor: usize, len: usize, height: usize) -> usize {
    cursor
        .saturating_sub(height / 2)
        .min(len.saturating_sub(height))
}

fn draw_list(frame: &mut Frame<'_>, files: &FilesState, area: Rect) {
    let visible = files.visible();
    let height = area.height as usize;
    let offset = centered_offset(files.cursor, visible.len(), height);
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
            theme::text()
        } else {
            theme::text_dim()
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

fn draw_wide_body(frame: &mut Frame<'_>, files: &FilesState, area: Rect) {
    let rail_width = if area.width >= 120 { 24 } else { 0 };
    let inspector_width = 38.min(area.width / 3);
    let [rail, columns, inspector] = Layout::horizontal([
        Constraint::Length(rail_width),
        Constraint::Min(10),
        Constraint::Length(inspector_width),
    ])
    .areas(area);
    if rail_width > 0 {
        draw_rail(frame, files, rail);
    }
    let parents: Vec<&std::path::Path> = files.dir.ancestors().skip(1).take(2).collect();
    let count = parents.len() + 1;
    let column_width = columns.width / count as u16;
    for (index, parent) in parents.iter().rev().enumerate() {
        let x = columns.x + column_width * index as u16;
        let col = Rect::new(x, columns.y, column_width, columns.height);
        let child = if index + 1 == parents.len() {
            files.dir.as_path()
        } else {
            parents[parents.len() - 2 - index]
        };
        draw_parent_column(frame, parent, child, col);
        for y in col.y..col.bottom() {
            frame.render_widget(
                Paragraph::new(Line::styled("│", Style::default().fg(theme::border_dim()))),
                Rect::new(col.right() - 1, y, 1, 1),
            );
        }
    }
    let last_x = columns.x + column_width * parents.len() as u16;
    draw_list(
        frame,
        files,
        Rect::new(last_x, columns.y, columns.right() - last_x, columns.height),
    );
    frame.render_widget(Block::default().style(theme::surface()), inspector);
    draw_inspector(frame, files, inspector);
}

fn draw_rail(frame: &mut Frame<'_>, files: &FilesState, area: Rect) {
    frame.render_widget(Block::default().style(theme::surface()), area);
    let root = std::env::current_dir()
        .ok()
        .and_then(|dir| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let muted = Style::default().fg(theme::muted());
    let dim = Style::default().fg(theme::text_dim());
    let row = |label: String, value: String, style: Style| {
        let gap = (area.width as usize).saturating_sub(3 + label.chars().count() + value.len());
        Line::from(vec![
            Span::styled(format!(" {label}"), style),
            Span::raw(" ".repeat(gap)),
            Span::styled(value, muted),
        ])
    };
    let lines = vec![
        Line::styled(" PLACES", muted),
        row(
            fit(&root, 16),
            String::new(),
            Style::default()
                .fg(theme::text())
                .add_modifier(Modifier::BOLD),
        ),
        row("changed".into(), files.changed.len().to_string(), dim),
        row("open tabs".into(), files.tabs.len().to_string(), dim),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_parent_column(
    frame: &mut Frame<'_>,
    dir: &std::path::Path,
    child: &std::path::Path,
    area: Rect,
) {
    let child_name = child.file_name().map(|n| n.to_string_lossy().into_owned());
    let mut entries: Vec<(String, bool)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                e.metadata().is_ok_and(|m| m.is_dir()),
            )
        })
        .collect();
    entries.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    let selected = entries
        .iter()
        .position(|(name, _)| Some(name) == child_name.as_ref())
        .unwrap_or(0);
    let height = area.height as usize;
    let offset = centered_offset(selected, entries.len(), height);
    let width = area.width.saturating_sub(1) as usize;
    for (row, (index, (name, is_dir))) in entries
        .iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .enumerate()
    {
        let row_area = Rect::new(area.x, area.y + row as u16, area.width.saturating_sub(1), 1);
        let is_selected = index == selected;
        let mut style = Style::default().fg(if is_selected {
            theme::text()
        } else {
            theme::text_dim()
        });
        if is_selected {
            frame.render_widget(
                Block::default().style(Style::default().bg(theme::surface_raised())),
                row_area,
            );
            style = style.add_modifier(Modifier::BOLD);
        }
        let icon = if *is_dir {
            icons::folder(is_selected)
        } else {
            icons::file(name)
        };
        let chevron = if *is_dir { "›" } else { " " };
        let label = fit(name, width.saturating_sub(5));
        let gap = width.saturating_sub(3 + label.chars().count() + 2);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {icon} "), Style::default().fg(theme::muted())),
                Span::styled(label, style),
                Span::raw(" ".repeat(gap)),
                Span::styled(chevron, Style::default().fg(theme::muted())),
            ])),
            row_area,
        );
    }
}

fn draw_inspector(frame: &mut Frame<'_>, files: &FilesState, area: Rect) {
    use std::os::unix::fs::PermissionsExt;
    let Some(entry) = files.selected() else {
        return;
    };
    let path = files.dir.join(&entry.name);
    let meta = std::fs::metadata(&path).ok();
    let stamp = |time: Option<SystemTime>| {
        time.map(|t| {
            chrono::DateTime::<chrono::Local>::from(t)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
    };
    let kind = if entry.is_dir {
        "Folder".to_owned()
    } else {
        std::path::Path::new(&entry.name)
            .extension()
            .map(|e| format!("{} file", e.to_string_lossy()))
            .unwrap_or_else(|| "File".to_owned())
    };
    let size = if entry.is_dir {
        format!(
            "{} items",
            std::fs::read_dir(&path).map_or(0, Iterator::count)
        )
    } else {
        human_size(entry.size)
    };
    let perms = meta
        .as_ref()
        .map(|m| {
            let mode = m.permissions().mode();
            "rwxrwxrwx"
                .chars()
                .enumerate()
                .map(|(i, c)| if mode & (1 << (8 - i)) != 0 { c } else { '-' })
                .collect::<String>()
        })
        .unwrap_or_default();
    let status = files
        .changed
        .get(&entry.name)
        .cloned()
        .unwrap_or_else(|| "clean".to_owned());
    let label = Style::default().fg(theme::muted());
    let value = Style::default().fg(theme::text());
    let mut lines = vec![
        Line::styled(
            format!(
                " {}",
                fit(&entry.name, area.width.saturating_sub(2) as usize)
            ),
            value.add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::styled(" INFORMATION", label),
    ];
    let rows = [
        ("Kind", kind),
        ("Size", size),
        (
            "Created",
            stamp(meta.as_ref().and_then(|m| m.created().ok())),
        ),
        ("Modified", stamp(entry.modified)),
        ("Where", fit(&files.dir.display().to_string(), 20)),
        ("Perms", perms),
    ];
    for (name, text) in rows {
        lines.push(Line::from(vec![
            Span::styled(format!(" {name:<9}"), label),
            Span::styled(text, value),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(" GIT", label));
    lines.push(Line::styled(format!(" {status}"), value));
    frame.render_widget(Paragraph::new(lines), area);
}
