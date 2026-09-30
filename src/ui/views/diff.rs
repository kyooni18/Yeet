//! Git review: directory rail, unified patch, and a quiet file inspector.
use super::super::{
    components::{composer, status, tabbar},
    support::{icons, theme},
};
use crate::app::{App, WorkbenchTab, diff::DiffState};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    prelude::{Color, Line, Modifier, Span, Style},
    widgets::{Block, Paragraph},
};
use std::path::PathBuf;

pub(in crate::ui) fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(theme::base().bg(theme::code_background())),
        area,
    );
    if area.width < 8 || area.height < 5 {
        return;
    }
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(area);
    tabbar::draw(frame, app, rows[0], WorkbenchTab::Diff(app.active_diff));
    let Some(s) = app.diff_tabs.get_mut(app.active_diff) else {
        return;
    };
    s.file_targets.clear();
    s.full_target = None;
    s.changes_target = None;
    let cols = Layout::horizontal([
        Constraint::Length(if area.width >= 60 { area.width / 6 } else { 0 }),
        Constraint::Min(1),
        Constraint::Length(if area.width >= 100 {
            area.width * 17 / 100
        } else {
            0
        }),
    ])
    .split(rows[1]);
    let (rail, main, inspector) = (cols[0], cols[1], cols[2]);
    draw_rail(frame, s, rail);
    let inset = if main.width >= 50 { 4 } else { 1 };
    let heading = Rect::new(
        main.x + inset,
        main.y + 1,
        main.width.saturating_sub(inset),
        1,
    );
    let path = s.paths.get(s.selected);
    let name = path
        .and_then(|p| p.file_name())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Diff".into());
    let controls = if main.width >= 44 { 27 } else { 0 };
    text(
        frame,
        Rect::new(
            heading.x,
            heading.y,
            heading.width.saturating_sub(controls),
            1,
        ),
        &name,
        Style::default()
            .fg(theme::text())
            .add_modifier(Modifier::BOLD),
    );
    if controls > 0 {
        let r = Rect::new(main.right() - controls - 1, heading.y, controls, 1);
        let full_style = if s.full {
            theme::surface().bg(theme::surface_raised())
        } else {
            theme::base().fg(theme::muted())
        };
        let changes_style = if !s.full {
            theme::surface().bg(theme::surface_raised())
        } else {
            theme::base().fg(theme::muted())
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" full file ", full_style),
                Span::raw("  "),
                Span::styled(" changes only ", changes_style),
            ])),
            r,
        );
        s.full_target = Some(Rect::new(r.x, r.y, 11, 1));
        s.changes_target = Some(Rect::new(r.x + 13, r.y, 14, 1));
    }
    let patch = Rect::new(
        main.x,
        main.y + 3,
        main.width,
        main.height.saturating_sub(6),
    );
    s.body_target = patch;
    let mut additions = 0;
    let mut deletions = 0;
    // Parse the whole patch before scrolling so line numbers remain correct.
    let rendered = patch_rows(&s.lines, inset, &mut additions, &mut deletions);
    if let Some(error) = &s.error {
        text(frame, patch, error, Style::default().fg(theme::error()));
    } else if s.paths.is_empty() {
        text(
            frame,
            patch,
            " Working tree clean — no changes to review.",
            Style::default().fg(theme::muted()),
        );
    } else if s.lines.is_empty() {
        text(
            frame,
            patch,
            " No content changes for this file.",
            Style::default().fg(theme::muted()),
        );
    } else {
        let first = rendered
            .iter()
            .position(|(index, _)| *index >= s.scroll)
            .unwrap_or(rendered.len());
        if let Some((index, _)) = rendered.get(first) {
            s.scroll = *index;
        }
        for (row, (_, line)) in rendered
            .into_iter()
            .skip(first)
            .take(patch.height as usize)
            .enumerate()
        {
            frame.render_widget(
                Paragraph::new(line.clone()).style(line.style),
                Rect::new(patch.x, patch.y + row as u16, patch.width, 1),
            );
        }
    }
    let hunks = s.lines.iter().filter(|l| l.starts_with("@@")).count();
    let status = s
        .statuses
        .get(s.selected)
        .map(String::as_str)
        .unwrap_or("  ");
    let state = if status == "??" {
        "untracked"
    } else if status.trim().is_empty() {
        "clean"
    } else if status.as_bytes().get(1).is_some_and(|b| *b != b' ') && !status.starts_with(' ') {
        "staged + unstaged"
    } else if status.starts_with(' ') {
        "unstaged"
    } else {
        "staged"
    };
    let summary = format!(" {hunks} hunks · +{additions} -{deletions} · {state} ");
    text(
        frame,
        Rect::new(
            main.x + 1,
            main.bottom().saturating_sub(3),
            (summary.chars().count() as u16).min(main.width.saturating_sub(2)),
            1,
        ),
        &summary,
        theme::surface()
            .bg(theme::surface_raised())
            .fg(theme::text_dim()),
    );
    draw_inspector(frame, s, inspector, &name, additions, deletions);
    // Keep the draft and composer geometry shared with the other workbench views.
    let composer_area = Rect::new(main.x, main.bottom().saturating_sub(1), main.width, 1);
    composer::draw(frame, app, composer_area, main.x + inset + 2);
    frame
        .buffer_mut()
        .set_style(composer_area, Style::default().bg(theme::surface_raised()));
    status::draw(frame, app, rows[2]);
}

fn draw_rail(frame: &mut Frame<'_>, s: &mut DiffState, rail: Rect) {
    if rail.width == 0 {
        return;
    }
    frame.render_widget(Block::default().style(theme::surface()), rail);
    let mut rows: Vec<(String, Option<usize>)> = Vec::new();
    let mut previous = PathBuf::new();
    for (i, path) in s.paths.iter().enumerate() {
        let parent = path.parent().unwrap_or(std::path::Path::new(""));
        let mut directory = PathBuf::new();
        for (depth, part) in parent.components().enumerate() {
            directory.push(part);
            if !previous.starts_with(&directory) {
                if !rows.is_empty() && depth == 0 {
                    rows.push((String::new(), None));
                }
                rows.push((
                    format!(
                        " {}{} {} {}",
                        "  ".repeat(depth),
                        icons::chevron(true),
                        icons::folder(false),
                        part.as_os_str().to_string_lossy()
                    ),
                    None,
                ));
            }
        }
        let depth = parent.components().count();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        rows.push((
            format!(" {}{} {}", "  ".repeat(depth + 1), icons::file(&name), name),
            Some(i),
        ));
        previous = parent.into();
    }
    let available = rail.height.saturating_sub(4) as usize;
    let selected_row = rows
        .iter()
        .position(|(_, i)| *i == Some(s.selected))
        .unwrap_or(0);
    let start = selected_row.saturating_sub(available.saturating_sub(1));
    let mut last = rail.y + 1;
    for (row, (label, index)) in rows.into_iter().skip(start).take(available).enumerate() {
        let r = Rect::new(rail.x, rail.y + 1 + row as u16, rail.width, 1);
        let style = if index == Some(s.selected) {
            theme::surface()
                .bg(theme::selected_color())
                .fg(theme::text())
                .add_modifier(Modifier::BOLD)
        } else {
            theme::surface().fg(if index.is_some() {
                theme::text_dim()
            } else {
                theme::muted()
            })
        };
        text(frame, r, &label, style);
        if let Some(i) = index {
            s.file_targets.push((r, i));
        }
        last = r.y + 2;
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {} files  ", s.paths.len()),
                Style::default().fg(theme::muted()),
            ),
            Span::styled(
                format!("+{}  ", s.totals.added),
                Style::default().fg(theme::success()),
            ),
            Span::styled(
                format!("-{}", s.totals.removed),
                Style::default().fg(theme::error()),
            ),
        ])),
        Rect::new(
            rail.x,
            last.min(rail.bottom().saturating_sub(1)),
            rail.width,
            1,
        ),
    );
}

fn draw_inspector(
    frame: &mut Frame<'_>,
    s: &DiffState,
    area: Rect,
    name: &str,
    added: usize,
    removed: usize,
) {
    if area.width == 0 {
        return;
    }
    frame.render_widget(Block::default().style(theme::surface()), area);
    let inner = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(3),
        area.height.saturating_sub(1),
    );
    let mut lines = vec![
        Line::styled(
            name.to_owned(),
            Style::default()
                .fg(theme::text())
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::from(vec![
            Span::styled(
                format!(
                    "{}  ",
                    s.statuses.get(s.selected).map(|v| v.trim()).unwrap_or("")
                ),
                Style::default().fg(theme::muted()),
            ),
            Span::styled(format!("+{added}  "), Style::default().fg(theme::success())),
            Span::styled(format!("-{removed}"), Style::default().fg(theme::error())),
        ]),
        Line::raw(""),
        Line::raw(""),
        Line::styled(
            format!(
                "{} / {}",
                s.selected + usize::from(!s.paths.is_empty()),
                s.paths.len()
            ),
            Style::default().fg(theme::text_dim()),
        ),
        Line::raw(""),
        Line::styled(s.base.clone(), Style::default().fg(theme::muted())),
        Line::raw(""),
        Line::raw(""),
    ];
    let mut contexts = Vec::new();
    for line in &s.lines {
        if line.starts_with("@@") {
            let context = line.split("@@").nth(2).unwrap_or("").trim();
            if !context.is_empty() && !contexts.contains(&context) {
                contexts.push(context);
            }
        }
    }
    lines.extend(
        contexts
            .into_iter()
            .take(6)
            .map(|context| Line::styled(context.to_owned(), Style::default().fg(theme::text()))),
    );
    lines.extend([
        Line::raw(""),
        Line::raw(""),
        Line::styled(s.branch.clone(), Style::default().fg(theme::muted())),
        Line::raw(""),
        Line::styled("f context · n/p hunk", Style::default().fg(theme::muted())),
        Line::styled("←/→ file · Enter open", Style::default().fg(theme::muted())),
    ]);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Preserve Git indices for keyboard hunk navigation while hiding transport headers.
fn patch_rows(
    lines: &[String],
    inset: u16,
    additions: &mut usize,
    deletions: &mut usize,
) -> Vec<(usize, Line<'static>)> {
    let mut result = Vec::new();
    let (mut old, mut new) = (0usize, 0usize);
    let mut in_hunk = false;
    for (index, line) in lines.iter().enumerate() {
        if line.starts_with("@@") {
            let mut parts = line.split_whitespace().skip(1);
            old = parts
                .next()
                .and_then(|v| v.trim_start_matches('-').split(',').next()?.parse().ok())
                .unwrap_or(0);
            new = parts
                .next()
                .and_then(|v| v.trim_start_matches('+').split(',').next()?.parse().ok())
                .unwrap_or(0);
            in_hunk = true;
            if !result.is_empty() {
                result.push((index, Line::raw("")));
            }
            let context = line.split("@@").nth(2).unwrap_or("").trim();
            let label = if context.is_empty() {
                line.clone()
            } else {
                format!("@@ {context}")
            };
            result.push((
                index,
                Line::styled(
                    format!("{}{}", " ".repeat(inset as usize), label),
                    Style::default().fg(theme::muted()),
                ),
            ));
            result.push((index, Line::raw("")));
            continue;
        }
        let marker = line.chars().next().unwrap_or(' ');
        if !in_hunk {
            if line.starts_with("Binary files")
                || line.starts_with("rename ")
                || line.starts_with("old mode")
                || line.starts_with("new mode")
            {
                result.push((
                    index,
                    Line::styled(
                        format!("{}{}", " ".repeat(inset as usize), line),
                        Style::default().fg(theme::muted()),
                    ),
                ));
            }
            continue;
        }
        let (number, foreground, background) = match marker {
            '+' => {
                let n = new;
                new += 1;
                *additions += 1;
                (n, theme::success(), tint(theme::success()))
            }
            '-' => {
                let n = old;
                old += 1;
                *deletions += 1;
                (n, theme::error(), tint(theme::error()))
            }
            ' ' => {
                let n = new;
                old += 1;
                new += 1;
                (n, theme::text_dim(), theme::code_background())
            }
            _ => {
                result.push((
                    index,
                    Line::styled(line.clone(), Style::default().fg(theme::muted())),
                ));
                continue;
            }
        };
        let code = line.get(1..).unwrap_or("");
        let mut spans = vec![
            Span::styled(
                format!(
                    "{}{number:>3}  ",
                    " ".repeat(inset.saturating_sub(1) as usize)
                ),
                Style::default().fg(theme::muted()),
            ),
            Span::styled(format!("{marker}  "), Style::default().fg(foreground)),
        ];
        // Changed text retains its semantic color; declarations get a gentle warm accent.
        let color = if marker == ' '
            && ["const ", "let ", "fn ", "pub ", "def ", "class "]
                .iter()
                .any(|prefix| code.trim_start().starts_with(prefix))
        {
            theme::text()
        } else {
            foreground
        };
        spans.push(Span::styled(code.to_owned(), Style::default().fg(color)));
        result.push((
            index,
            Line::from(spans).style(Style::default().bg(background)),
        ));
    }
    result
}
fn tint(color: Color) -> Color {
    match (theme::code_background(), color) {
        (Color::Rgb(r, g, b), Color::Rgb(cr, cg, cb)) => Color::Rgb(
            ((r as u16 * 9 + cr as u16) / 10) as u8,
            ((g as u16 * 9 + cg as u16) / 10) as u8,
            ((b as u16 * 9 + cb as u16) / 10) as u8,
        ),
        _ => theme::surface_color(),
    }
}
fn text(frame: &mut Frame<'_>, area: Rect, value: &str, style: Style) {
    frame.render_widget(Paragraph::new(value.to_owned()).style(style), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn review_rows_hide_headers_keep_numbers_and_fill_change_bands() {
        let mut app = App::default();
        app.diff_tabs.push(DiffState {
            paths: vec!["guidance/team.c".into()],
            statuses: vec![" M".into()],
            lines: [
                "diff --git a/team.c b/team.c",
                "--- a/team.c",
                "+++ b/team.c",
                "@@ -418,2 +418,2 @@ select_team_mode()",
                "-old",
                "+new",
                " context",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            ..Default::default()
        });
        let mut terminal = Terminal::new(TestBackend::new(144, 44)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let contents = buffer
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(!contents.contains("diff --git"));
        assert!(!contents.contains("+++ b/"));
        assert!(contents.contains("418  -  old"));
        assert!(contents.contains("418  +  new"));
        assert!(contents.contains("419     context"));
        let patch = app.diff_tabs[0].body_target;
        for (label, color) in [("old", theme::error()), ("new", theme::success())] {
            let y = (patch.y..patch.bottom())
                .find(|y| {
                    (patch.x..patch.right())
                        .map(|x| buffer[(x, *y)].symbol())
                        .collect::<String>()
                        .contains(label)
                })
                .unwrap();
            assert_eq!(buffer[(patch.x, y)].bg, tint(color));
            assert_eq!(buffer[(patch.right() - 1, y)].bg, tint(color));
        }
        let target = app.diff_tabs[0].file_targets[0].0;
        assert!(contents.contains("guidance"));
        assert!(target.y > 1);
        app.diff_tabs[0].scroll = 3;
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let contents = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(contents.contains("select_team_mode()"));
        for (width, height) in [(80, 24), (40, 18), (8, 5)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|f| draw(f, &mut app)).unwrap();
        }
    }
}
