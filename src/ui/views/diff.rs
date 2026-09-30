//! Desktop / Diff: contextual changes rail, numbered patch, and inspector.
use super::super::{
    components::{status, tabbar},
    support::theme,
};
use crate::app::{App, WorkbenchTab};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    prelude::{Line, Span, Style},
    widgets::{Block, Paragraph},
};

pub(in crate::ui) fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();
    frame.render_widget(Block::default().style(theme::base()), area);
    if area.width < 8 || area.height < 5 {
        return;
    }
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);
    tabbar::draw(frame, app, rows[0], WorkbenchTab::Diff(app.active_diff));
    let Some(s) = app.diff_tabs.get_mut(app.active_diff) else {
        return;
    };
    s.file_targets.clear();
    s.full_target = None;
    let wide = area.width >= 100;
    let rail_width = if area.width >= 60 {
        (area.width as u32 * 232 / 1440) as u16
    } else {
        0
    };
    let inspector_width = if wide {
        (area.width as u32 * 250 / 1440) as u16
    } else {
        0
    };
    let cols = Layout::horizontal([
        Constraint::Length(rail_width),
        Constraint::Min(1),
        Constraint::Length(inspector_width),
    ])
    .split(rows[1]);
    let rail = cols[0];
    let main = cols[1];
    let inspector = cols[2];
    s.body_target = main;
    if rail.width > 0 {
        frame.render_widget(Block::default().style(theme::surface()), rail);
        text(
            frame,
            Rect::new(rail.x, rail.y, rail.width, 1),
            " Changes",
            theme::secondary(),
        );
        let available = rail.height.saturating_sub(3) as usize;
        let start = s.selected.saturating_sub(available.saturating_sub(1));
        for (row, (i, path)) in s
            .paths
            .iter()
            .enumerate()
            .skip(start)
            .take(available)
            .enumerate()
        {
            let r = Rect::new(rail.x, rail.y + 1 + row as u16, rail.width, 1);
            let style = if i == s.selected {
                theme::selected()
            } else {
                theme::surface().fg(theme::text_dim())
            };
            frame.render_widget(
                Paragraph::new(format!(
                    " {} {}",
                    s.statuses.get(i).map(String::as_str).unwrap_or(""),
                    path.display()
                ))
                .style(style),
                r,
            );
            s.file_targets.push((r, i));
        }
        text(
            frame,
            Rect::new(rail.x, rail.bottom().saturating_sub(1), rail.width, 1),
            &format!(" {} files", s.paths.len()),
            theme::muted(),
        );
    }
    let path = s
        .paths
        .get(s.selected)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Diff".into());
    let controls = if main.width >= 44 {
        26.min(main.width)
    } else {
        0
    };
    text(
        frame,
        Rect::new(main.x, main.y, main.width.saturating_sub(controls), 1),
        &format!(" {path}"),
        theme::text(),
    );
    if controls > 0 {
        let r = Rect::new(main.right() - controls, main.y, controls, 1);
        text(
            frame,
            r,
            if s.full {
                "[full file]  changes only"
            } else {
                " full file  [changes only]"
            },
            theme::secondary(),
        );
        s.full_target = Some(r);
    }
    let patch = Rect::new(
        main.x,
        main.y + 1,
        main.width,
        main.height.saturating_sub(2),
    );
    let additions = s
        .lines
        .iter()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .count();
    let deletions = s
        .lines
        .iter()
        .filter(|l| l.starts_with('-') && !l.starts_with("---"))
        .count();
    let hunks = s.lines.iter().filter(|l| l.starts_with("@@")).count();
    if let Some(error) = &s.error {
        text(frame, patch, error, theme::error());
    } else if s.paths.is_empty() {
        text(
            frame,
            patch,
            "Working tree clean — no changes to review.",
            theme::muted(),
        );
    } else if s.lines.is_empty() {
        text(
            frame,
            patch,
            "No content changes for this file.",
            theme::muted(),
        );
    } else {
        let mut old = 0usize;
        let mut new = 0usize;
        let mut lines = Vec::new();
        for line in &s.lines {
            if line.starts_with("@@") {
                let mut parts = line.split_whitespace();
                parts.next();
                old = parts
                    .next()
                    .and_then(|v| v.trim_start_matches('-').split(',').next()?.parse().ok())
                    .unwrap_or(0);
                new = parts
                    .next()
                    .and_then(|v| v.trim_start_matches('+').split(',').next()?.parse().ok())
                    .unwrap_or(0);
                lines.push(Line::from(Span::styled(
                    line.clone(),
                    Style::default().fg(theme::secondary()),
                )));
                continue;
            }
            let header = line.starts_with("+++")
                || line.starts_with("---")
                || !(line.starts_with('+') || line.starts_with('-') || line.starts_with(' '));
            let (number, color) = if header {
                (String::new(), theme::muted())
            } else if line.starts_with('+') {
                let n = new;
                new += 1;
                (n.to_string(), theme::success())
            } else if line.starts_with('-') {
                let n = old;
                old += 1;
                (n.to_string(), theme::error())
            } else {
                let n = new;
                new += 1;
                old += 1;
                (n.to_string(), theme::text_dim())
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{number:>5} "), Style::default().fg(theme::muted())),
                Span::styled(line.clone(), Style::default().fg(color)),
            ]));
        }
        frame.render_widget(
            Paragraph::new(
                lines
                    .into_iter()
                    .skip(s.scroll)
                    .take(patch.height as usize)
                    .collect::<Vec<_>>(),
            ),
            patch,
        );
    }
    text(
        frame,
        Rect::new(main.x, main.bottom().saturating_sub(1), main.width, 1),
        &format!(" {hunks} hunks · +{additions} −{deletions} · HEAD → working tree"),
        theme::muted(),
    );
    if inspector.width > 0 {
        frame.render_widget(Block::default().style(theme::surface()), inspector);
        let details = format!(
            "{path}\n\n+{additions} −{deletions}\n\n{} / {}\n{}\n{}\n\n{hunks} hunks\n\n[f] full / changes\n[n/p] next / prev hunk\n[←/→] select file\n[Enter] open file\n[r] refresh",
            s.selected + usize::from(!s.paths.is_empty()),
            s.paths.len(),
            s.branch,
            s.base
        );
        frame.render_widget(
            Paragraph::new(details).style(theme::surface().fg(theme::text_dim())),
            Rect::new(
                inspector.x + 1,
                inspector.y,
                inspector.width.saturating_sub(1),
                inspector.height,
            ),
        );
    }
    text(
        frame,
        rows[2],
        " [←/→] file  [↑/↓] scroll  [f] context  [n/p] hunk  [Enter] open file  [r] refresh",
        theme::secondary(),
    );
    status::draw(frame, app, rows[3]);
}
fn text(frame: &mut Frame<'_>, area: Rect, value: &str, color: ratatui::style::Color) {
    frame.render_widget(
        Paragraph::new(value.to_owned()).style(Style::default().fg(color)),
        area,
    );
}
