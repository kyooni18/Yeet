//! Full-screen keyboard file browser (`Mode::Files`).
use super::super::{
    components::{composer, status, tabbar},
    support::{icons, responsive, theme},
    task::fit,
};
use crate::harness::resources;
use crate::tui::app::{
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

pub(in crate::platforms::tui::ui) fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(theme::base().bg(theme::code_background())),
        area,
    );
    let Some(files) = app.files.as_ref() else {
        return;
    };
    if area.height < 4 || area.width < 8 {
        return;
    }
    if responsive::shape(area) == responsive::Shape::Portrait {
        draw_portrait(frame, app, files, area);
        return;
    }
    let hints = if files.hints {
        app.keymap.hints(Context::Files, &HINT_ACTIONS)
    } else {
        Vec::new()
    };
    let wide = area.width >= 100;
    if wide {
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(area);
        let active = files
            .active_tab()
            .map_or(tabbar::Active::Files, tabbar::Active::File);
        tabbar::draw(frame, app, rows[0], active);
        if files.diff {
            draw_diff(frame, files, rows[1]);
        } else {
            draw_wide_body(frame, files, rows[1]);
            draw_wide_breadcrumb(frame, files, rows[1]);
        }
        composer::draw(frame, app, rows[2], 0);
        status::draw(frame, app, rows[3]);
        return;
    }
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

    let active = match files.active_tab() {
        Some(index) => tabbar::Active::File(index),
        None => tabbar::Active::Files,
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

fn draw_wide_breadcrumb(frame: &mut Frame<'_>, files: &FilesState, body: Rect) {
    if body.height == 0 || body.width < 36 {
        return;
    }
    let x = body.x + 24.min(body.width / 4) + 1;
    let width = body.right().saturating_sub(x).min(64);
    let location = files.selected_path().unwrap_or_else(|| files.dir.clone());
    let parts = location
        .components()
        .rev()
        .take(4)
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .filter(|part| part != "/")
        .collect::<Vec<_>>();
    let label = parts.into_iter().rev().collect::<Vec<_>>().join("  ›  ");
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!(" {} ", fit(&label, width.saturating_sub(2) as usize)),
            Style::default()
                .fg(theme::muted())
                .bg(theme::surface_color()),
        )),
        Rect::new(x, body.bottom() - 1, width, 1),
    );
}

fn draw_portrait(frame: &mut Frame<'_>, app: &App, files: &FilesState, area: Rect) {
    frame.render_widget(
        Block::default().style(theme::base().bg(theme::code_background())),
        area,
    );
    draw_location(frame, files, Rect::new(area.x, area.y, area.width, 1));
    let body = Rect::new(
        area.x,
        area.y + 2,
        area.width,
        area.height.saturating_sub(3),
    );
    if files.diff {
        draw_diff(frame, files, body);
    } else {
        let item_rows = (files.visible().len() + 1)
            .saturating_mul(2)
            .min(u16::MAX as usize) as u16;
        let list_height = item_rows.min(14).min(body.height.saturating_sub(7));
        let list = Rect::new(body.x, body.y, body.width, list_height);
        draw_portrait_list(frame, files, list);
        let details = Rect::new(body.x, list.bottom(), body.width, body.height - list_height);
        draw_portrait_details(frame, files, details);
    }
    if files.hints {
        let hints = app.keymap.hints(Context::Files, &HINT_ACTIONS);
        let height = (hints.len() as u16).min(area.height.saturating_sub(2));
        draw_hints(
            frame,
            &hints,
            Rect::new(area.x, area.bottom() - 1 - height, area.width, height),
        );
    }
    draw_status(
        frame,
        app,
        files,
        Rect::new(area.x, area.bottom() - 1, area.width, 1),
    );
}

fn draw_portrait_list(frame: &mut Frame<'_>, files: &FilesState, area: Rect) {
    if area.height < 2 {
        return;
    }
    if files.dir.parent().is_some() {
        frame.render_widget(
            Paragraph::new(Line::styled("     ..", Style::default().fg(theme::muted()))),
            Rect::new(area.x, area.y, area.width, 1),
        );
    }
    let visible = files.visible();
    let slots = area.height.saturating_sub(2) as usize / 2;
    let offset = centered_offset(files.cursor, visible.len(), slots);
    for (row, (index, entry)) in visible
        .iter()
        .enumerate()
        .skip(offset)
        .take(slots)
        .enumerate()
    {
        let selected = index == files.cursor;
        let row_area = Rect::new(area.x, area.y + 2 + row as u16 * 2, area.width, 1);
        if selected {
            frame.render_widget(
                Block::default().style(Style::default().bg(theme::selected_color())),
                row_area,
            );
        }
        let icon = if entry.is_dir {
            icons::folder(false)
        } else {
            icons::file(&entry.name)
        };
        let name = if entry.is_dir {
            format!("{}/", entry.name)
        } else {
            entry.name.clone()
        };
        let style = Style::default()
            .fg(if selected {
                theme::text()
            } else {
                theme::text_dim()
            })
            .add_modifier(if selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    if selected { "▏ " } else { "   " },
                    Style::default().fg(theme::accent()),
                ),
                Span::styled(format!("{icon}  "), Style::default().fg(theme::muted())),
                Span::styled(fit(&name, area.width.saturating_sub(7) as usize), style),
            ])),
            row_area,
        );
    }
}

fn draw_portrait_details(frame: &mut Frame<'_>, files: &FilesState, area: Rect) {
    let Some(entry) = files.selected() else {
        return;
    };
    if area.height < 3 {
        return;
    }
    let divider = "─".repeat(area.width.saturating_sub(4) as usize);
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!("  {divider}"),
            Style::default().fg(theme::border_dim()),
        )),
        Rect::new(area.x, area.y, area.width, 1),
    );
    let size = human_size(entry.size);
    let lines = files
        .selected_lines
        .map(|n| format!(" · {n} lines"))
        .unwrap_or_default();
    let modified = entry
        .modified
        .map(|time| {
            chrono::DateTime::<chrono::Local>::from(time)
                .format("%b %d %H:%M")
                .to_string()
        })
        .unwrap_or_default();
    let status = change_label(files.changed.get(&entry.name).map(String::as_str));
    let delta = files
        .diff_stats
        .map(|(add, remove)| format!(" · +{add} -{remove}"))
        .unwrap_or_default();
    let details = [
        (2, entry.name.clone(), true),
        (4, format!("{size}{lines}"), false),
        (6, modified, false),
        (8, format!("{status}{delta}"), false),
    ];
    for (offset, text, bold) in details {
        if offset >= area.height {
            break;
        }
        let style = Style::default()
            .fg(if bold {
                theme::text()
            } else {
                theme::text_dim()
            })
            .add_modifier(if bold {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!("  {}", fit(&text, area.width.saturating_sub(3) as usize)),
                style,
            )),
            Rect::new(area.x, area.y + offset, area.width, 1),
        );
    }
}

fn change_label(status: Option<&str>) -> &'static str {
    match status.unwrap_or_default().trim() {
        "M" => "modified",
        "A" => "added",
        "D" => "deleted",
        "??" => "untracked",
        "R" => "renamed",
        _ => "clean",
    }
}

fn change_stage(status: Option<&str>) -> &'static str {
    match status.unwrap_or_default().as_bytes() {
        [b'?', b'?'] => "",
        [index, worktree] if *index != b' ' && *worktree != b' ' => " · staged + unstaged",
        [index, _] if *index != b' ' => " · staged",
        [_, worktree] if *worktree != b' ' => " · unstaged",
        _ => "",
    }
}

fn friendly_path(path: &std::path::Path) -> String {
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from)
        && let Ok(relative) = path.strip_prefix(home)
    {
        return format!("~/{}", relative.display());
    }
    path.display().to_string()
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
    let offset = if area.height >= 20 {
        files.cursor.saturating_sub(3)
    } else {
        centered_offset(files.cursor, visible.len(), height)
    };
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
                Block::default().style(Style::default().bg(theme::selected_color())),
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
        .map(|value| value.trim().to_owned())
        .unwrap_or_else(|| "clean".to_owned());
    let size = if entry.is_dir {
        let count = resources::directory_entry_count(&files.dir.join(&entry.name));
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
    frame.render_widget(
        Block::default().style(theme::base().bg(theme::code_background())),
        area,
    );
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
    frame.render_widget(
        Block::default().style(theme::base().bg(theme::code_background())),
        area,
    );
    let cue = app
        .keymap
        .keys_for(Context::Files, Action::ToggleHints)
        .into_iter()
        .next()
        .map(|key| format!("{key} hints"))
        .unwrap_or_default();
    let cue_width = cue.chars().count() as u16 + 1;
    let path = friendly_path(&files.dir);
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
    frame.render_widget(
        Block::default().style(theme::base().bg(theme::code_background())),
        columns,
    );
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
    frame.render_widget(Block::default().style(theme::base()), inspector);
    draw_inspector(frame, files, inspector);
}

fn draw_rail(frame: &mut Frame<'_>, files: &FilesState, area: Rect) {
    frame.render_widget(Block::default().style(theme::base()), area);
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
    let mut lines = vec![
        Line::raw(""),
        Line::styled(" PLACES", muted),
        Line::raw(""),
        Line::styled(
            format!(
                " {:<width$}",
                fit(&root, area.width.saturating_sub(2) as usize),
                width = area.width.saturating_sub(1) as usize
            ),
            Style::default()
                .fg(theme::text())
                .bg(theme::surface_color())
                .add_modifier(Modifier::BOLD),
        ),
        row("changed".into(), files.changed.len().to_string(), dim),
        row("open tabs".into(), files.tabs.len().to_string(), dim),
        Line::raw(""),
        Line::styled(
            "─".repeat(area.width.saturating_sub(2) as usize),
            Style::default().fg(theme::border_dim()),
        ),
        Line::raw(""),
        Line::styled(" RECENT", muted),
    ];
    let workspace = std::env::current_dir().unwrap_or_default();
    for recent in files.recent_dirs.iter().take(5) {
        let display = recent
            .strip_prefix(&workspace)
            .ok()
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| {
                recent
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| recent.display().to_string())
            });
        lines.push(row(
            fit(&display, area.width.saturating_sub(3) as usize),
            String::new(),
            dim,
        ));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_parent_column(
    frame: &mut Frame<'_>,
    dir: &std::path::Path,
    child: &std::path::Path,
    area: Rect,
) {
    let child_name = child.file_name().map(|n| n.to_string_lossy().into_owned());
    let entries: Vec<(String, bool)> = resources::directory_entries(dir)
        .into_iter()
        .map(|entry| (entry.name, entry.is_dir))
        .collect();
    let selected = entries
        .iter()
        .position(|(name, _)| Some(name) == child_name.as_ref())
        .unwrap_or(0);
    let height = area.height as usize;
    let offset = selected;
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
                Block::default().style(Style::default().bg(theme::surface_color())),
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
    let Some(entry) = files.selected() else {
        return;
    };
    let path = files.dir.join(&entry.name);
    let metadata = resources::file_metadata(&path);
    let stamp = |time: Option<SystemTime>| {
        time.map(|t| {
            chrono::DateTime::<chrono::Local>::from(t)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
    };
    let kind = file_kind(entry);
    let size = if entry.is_dir {
        format!("{} items", resources::directory_entry_count(&path))
    } else {
        human_size(entry.size)
    };
    let perms = metadata
        .as_ref()
        .and_then(|metadata| metadata.permissions)
        .map(|mode| {
            "rwxrwxrwx"
                .chars()
                .enumerate()
                .map(|(i, c)| if mode & (1 << (8 - i)) != 0 { c } else { '-' })
                .collect::<String>()
        })
        .unwrap_or_default();
    let status = change_label(files.changed.get(&entry.name).map(String::as_str));
    let stage = change_stage(files.changed.get(&entry.name).map(String::as_str));
    let label = Style::default().fg(theme::muted());
    let value = Style::default().fg(theme::text());
    let mut lines = vec![
        Line::styled(
            format!(
                " {}  {}",
                if entry.is_dir {
                    icons::folder(false)
                } else {
                    icons::file(&entry.name)
                },
                fit(&entry.name, area.width.saturating_sub(5) as usize)
            ),
            value.add_modifier(Modifier::BOLD),
        ),
        Line::styled(format!(" {} · {size}", kind), label),
        Line::raw(""),
        Line::styled(" INFORMATION", label),
        Line::raw(""),
    ];
    let rows = [
        ("Kind", kind),
        ("Size", size),
        (
            "Created",
            stamp(metadata.as_ref().and_then(|metadata| metadata.created)),
        ),
        ("Modified", stamp(entry.modified)),
        (
            "Opened",
            stamp(metadata.as_ref().and_then(|metadata| metadata.accessed)),
        ),
        (
            "Where",
            fit(
                &friendly_path(&files.dir),
                area.width.saturating_sub(13) as usize,
            ),
        ),
        ("Perms", perms),
        (
            "Lines",
            files
                .selected_lines
                .map(|lines| format!("{lines} · UTF-8 · LF"))
                .unwrap_or_default(),
        ),
    ];
    for (name, text) in rows {
        lines.push(Line::from(vec![
            Span::styled(format!(" {name:<9}"), label),
            Span::styled(text, value),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(" GIT", label));
    lines.push(Line::raw(""));
    for (name, text) in [
        ("Status", format!("{status}{stage}")),
        ("Branch", files.git_branch.clone()),
        ("Commit", files.git_commit.clone()),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!(" {name:<9}"), label),
            Span::styled(fit(&text, area.width.saturating_sub(12) as usize), value),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn file_kind(entry: &crate::tui::app::files::FileEntry) -> String {
    if entry.is_dir {
        return "Folder".into();
    }
    match std::path::Path::new(&entry.name)
        .extension()
        .and_then(|ext| ext.to_str())
    {
        Some("rs") => "Rust source".into(),
        Some("c" | "h") => "C source".into(),
        Some("md") => "Markdown".into(),
        Some(ext) => format!("{ext} file"),
        None => "File".into(),
    }
}
