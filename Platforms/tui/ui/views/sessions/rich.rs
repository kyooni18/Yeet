//! Terminal presentation of tool-specific details: diffs, shell sessions and
//! source previews. The shared projection owns the data; this only paints it.
use super::super::diff::tint;
use crate::shared_ui::conversation::{
    DiagnosticItem, DiffDetail, DiffFile, DiffLineKind, OutputStream, RichDetail,
    SourceAvailability, SourceDetail, TerminalDetail, TerminalState,
};
use crate::tui::ui::support::{
    text::{cell_width, format_elapsed, prefixed_wrapped_line},
    theme,
};
use ratatui::prelude::{Color, Line, Modifier, Span, Style};

const INDENT: usize = 2;
/// Rows shown for one command's output or one file's source before the rest is
/// left to the raw view; keeps long results from becoming huge off-screen buffers.
const MAX_OUTPUT_ROWS: usize = 240;
const LIVE_OUTPUT_ROWS: usize = 40;
const MAX_SOURCE_ROWS: usize = 400;
/// Pads with NBSP: ratatui wraps a full-width all-space row into two rows and
/// does not paint its background.
const PAD: char = '\u{a0}';

pub(super) fn rich_lines(details: &[RichDetail], width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width);
    if width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    for detail in details {
        if !lines.is_empty() && !matches!(detail, RichDetail::Notice { .. }) {
            // Blocks stay visually separate without a heading per block.
            if !matches!(detail, RichDetail::Diagnostics { .. }) {
                lines.push(Line::default());
            }
        }
        match detail {
            RichDetail::Diff(diff) => diff_lines(diff, width, &mut lines),
            RichDetail::Terminal(terminal) => terminal_lines(terminal, width, &mut lines),
            RichDetail::Source(source) => source_lines(source, width, &mut lines),
            RichDetail::Diagnostics { items } => diagnostic_lines(items, width, &mut lines),
            RichDetail::Notice { message, is_error } => {
                let style = Style::default().fg(if *is_error {
                    theme::error_subtle()
                } else {
                    theme::muted()
                });
                lines.extend(prefixed_wrapped_line(
                    Span::raw("  "),
                    Line::styled(
                        if *is_error { "Error:" } else { "Note:" },
                        style.add_modifier(Modifier::BOLD),
                    ),
                    width as u16,
                ));
                for line in sanitize(message).lines() {
                    lines.extend(prefixed_wrapped_line(
                        Span::raw("  "),
                        Line::styled(expand_tabs(line), style),
                        width as u16,
                    ));
                }
            }
        }
    }
    lines.into_iter().map(|line| clip(line, width)).collect()
}

// ---- text safety and geometry -------------------------------------------

/// Drops terminal control sequences and non-printing controls so tool output
/// can never move the cursor, retitle the window or spoof other rows.
pub(super) fn sanitize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    skip_csi(&mut chars);
                }
                Some(']' | 'P' | '^' | '_') => {
                    chars.next();
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            '\u{9b}' => skip_csi(&mut chars),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\t' | '\r' => out.push(ch),
            // Bidirectional overrides can visually reorder neighbouring rows.
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => {}
            ch if ch.is_control() => {}
            ch => out.push(ch),
        }
    }
    out
}
fn skip_csi(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for next in chars.by_ref() {
        if ('\u{40}'..='\u{7e}').contains(&next) {
            break;
        }
    }
}
/// Logical lines of already-sanitized text. A carriage return moves the
/// cursor to the start; it does not erase the unwritten suffix of the row.
fn logical_lines(text: &str) -> Vec<String> {
    text.split('\n')
        .map(|line| {
            let mut row = Vec::new();
            for segment in line.split('\r') {
                for (index, ch) in expand_tabs(segment).chars().enumerate() {
                    if let Some(previous) = row.get_mut(index) {
                        *previous = ch;
                    } else {
                        row.push(ch);
                    }
                }
            }
            row.into_iter().collect()
        })
        .collect()
}
fn expand_tabs(line: &str) -> String {
    if !line.contains('\t') {
        return line.to_owned();
    }
    let mut out = String::new();
    let mut column = 0;
    for ch in line.chars() {
        if ch == '\t' {
            let fill = 4 - column % 4;
            out.push_str(&" ".repeat(fill));
            column += fill;
        } else {
            out.push(ch);
            column += cell_width(ch.encode_utf8(&mut [0; 4]));
        }
    }
    out
}
/// Splits one logical line into rows of at most `width` cells without losing a
/// character; a wide glyph that cannot fit at all becomes `…`.
fn wrap_cells(line: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = vec![String::new()];
    let mut used = 0;
    for ch in line.chars() {
        let cells = cell_width(ch.encode_utf8(&mut [0; 4]));
        let (ch, cells) = if cells > width {
            ('…', 1)
        } else {
            (ch, cells)
        };
        if used + cells > width {
            rows.push(String::new());
            used = 0;
        }
        rows.last_mut().unwrap().push(ch);
        used += cells;
    }
    rows
}
fn clip(line: Line<'static>, width: usize) -> Line<'static> {
    if line.width() <= width {
        return line;
    }
    let style = line.style;
    let mut used = 0;
    let mut spans = Vec::new();
    for span in line.spans {
        let mut kept = String::new();
        for ch in span.content.chars() {
            let cells = cell_width(ch.encode_utf8(&mut [0; 4]));
            if used + cells > width {
                break;
            }
            kept.push(ch);
            used += cells;
        }
        let complete = kept.len() == span.content.len();
        spans.push(Span::styled(kept, span.style));
        if !complete {
            break;
        }
    }
    Line::from(spans).style(style)
}
/// One row filled to `width` with `background`, so wrapped and blank rows keep
/// an unbroken band.
fn banded(mut spans: Vec<Span<'static>>, width: usize, background: Option<Color>) -> Line<'static> {
    let Some(background) = background else {
        return Line::from(spans);
    };
    let used: usize = spans.iter().map(Span::width).sum();
    for span in &mut spans {
        span.style = span.style.bg(background);
    }
    if used < width {
        spans.push(Span::styled(
            PAD.to_string().repeat(width - used),
            Style::default().bg(background),
        ));
    }
    Line::from(spans)
}
fn digits(value: usize) -> usize {
    value.max(1).to_string().len()
}
fn muted(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(theme::muted()))
}
fn note(text: &str, width: usize, out: &mut Vec<Line<'static>>) {
    out.extend(prefixed_wrapped_line(
        Span::raw("  "),
        Line::styled(text.to_owned(), Style::default().fg(theme::muted())),
        width as u16,
    ));
}
fn artifact_note(prefix: &str, artifact: &Option<String>) -> String {
    match artifact {
        Some(id) => format!("{prefix} · artifact {id}"),
        None => prefix.to_owned(),
    }
}

// ---- diff -----------------------------------------------------------------

fn diff_lines(diff: &DiffDetail, width: usize, out: &mut Vec<Line<'static>>) {
    for (index, file) in diff.files.iter().enumerate() {
        if index > 0 {
            out.push(Line::default());
        }
        file_header(file, width, out);
        if !file.has_diff {
            let text = match &diff.externalized {
                Some(external) => artifact_note(
                    "diff stored externally; hunks not captured here",
                    &external.artifact_id,
                ),
                None => "no diff captured".to_owned(),
            };
            note(&text, width, out);
            continue;
        }
        if file.hunks.is_empty() {
            note("no line changes", width, out);
        }
        let numbers = file
            .hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .flat_map(|line| [line.old, line.new])
            .flatten()
            .max()
            .unwrap_or(0);
        let digit_width = digits(numbers).max(2);
        // Two number columns, a marker and separators, then at least a few cells of code.
        let gutter = INDENT + digit_width * 2 + 2 + 2;
        let compact = width < gutter + 8;
        for hunk in &file.hunks {
            let range = |start: usize, end: Option<usize>| match end {
                Some(end) if end >= start => format!("{start},{}", end + 1 - start),
                Some(_) => format!("{},0", start.saturating_sub(1)),
                None => start.to_string(),
            };
            note(
                &format!(
                    "@@ -{} +{} @@{}",
                    range(hunk.old_start, hunk.old_end),
                    range(hunk.new_start, hunk.new_end),
                    if hunk.truncated {
                        " · preview truncated"
                    } else {
                        ""
                    },
                ),
                width,
                out,
            );
            let text_width = width.saturating_sub(if compact { INDENT + 2 } else { gutter });
            for line in &hunk.lines {
                if line.kind == DiffLineKind::Omitted {
                    note(
                        "⋯ middle of this hunk omitted by the edit preview",
                        width,
                        out,
                    );
                    continue;
                }
                let (marker, color) = match line.kind {
                    DiffLineKind::Added => ('+', theme::success()),
                    DiffLineKind::Removed => ('−', theme::error()),
                    _ => (' ', theme::text_dim()),
                };
                let background = match line.kind {
                    DiffLineKind::Added => Some(tint(theme::success())),
                    DiffLineKind::Removed => Some(tint(theme::error())),
                    _ => None,
                };
                let body = sanitize(&line.text);
                let body = logical_lines(&body).into_iter().next().unwrap_or_default();
                for (row, segment) in wrap_cells(&body, text_width).into_iter().enumerate() {
                    let number = |value: Option<usize>| match value {
                        Some(value) if row == 0 => format!("{value:>digit_width$}"),
                        _ => " ".repeat(digit_width),
                    };
                    let mut spans = vec![Span::raw(" ".repeat(INDENT))];
                    if !compact {
                        spans.push(muted(format!("{} {} ", number(line.old), number(line.new))));
                    }
                    spans.push(Span::styled(
                        format!("{} ", if row == 0 { marker } else { ' ' }),
                        Style::default().fg(color),
                    ));
                    spans.push(Span::styled(
                        segment,
                        Style::default().fg(if background.is_some() {
                            color
                        } else {
                            theme::text()
                        }),
                    ));
                    out.push(banded(spans, width, background));
                }
            }
        }
        for warning in &file.warnings {
            out.extend(prefixed_wrapped_line(
                Span::raw("  "),
                Line::styled(
                    format!("warning: {warning}"),
                    Style::default().fg(theme::warning()),
                ),
                width as u16,
            ));
        }
    }
    if diff.files.is_empty()
        && let Some(external) = &diff.externalized
    {
        note(
            &artifact_note(
                "edit result stored externally; hunks not captured here",
                &external.artifact_id,
            ),
            width,
            out,
        );
    }
}

fn file_header(file: &DiffFile, width: usize, out: &mut Vec<Line<'static>>) {
    let mut spans = vec![Span::styled(
        sanitize(&file.path),
        Style::default()
            .fg(theme::text())
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(previous) = &file.previous_path {
        spans.push(muted(format!(" ← {}", sanitize(previous))));
    }
    if file.operation != "update" {
        spans.push(muted(format!(" · {}", file.operation)));
    }
    if file.has_diff {
        let (added, removed) = (file.added(), file.removed());
        let mark = if file.truncated { "+" } else { "" };
        if added > 0 {
            spans.push(Span::styled(
                format!(" +{added}{mark}"),
                Style::default().fg(theme::success()),
            ));
        }
        if removed > 0 {
            spans.push(Span::styled(
                format!(" −{removed}{mark}"),
                Style::default().fg(theme::error()),
            ));
        }
        if file.truncated {
            spans.push(muted(" · preview truncated"));
        }
    }
    out.extend(prefixed_wrapped_line(
        Span::raw("  "),
        Line::from(spans),
        width as u16,
    ));
}

fn diagnostic_lines(items: &[DiagnosticItem], width: usize, out: &mut Vec<Line<'static>>) {
    out.extend(prefixed_wrapped_line(
        Span::raw("  "),
        Line::styled(
            "Diagnostics:",
            Style::default()
                .fg(theme::secondary())
                .add_modifier(Modifier::BOLD),
        ),
        width as u16,
    ));
    for item in items {
        let color = match item.severity.to_ascii_lowercase().as_str() {
            "error" => theme::error_subtle(),
            "warning" | "warn" => theme::warning(),
            _ => theme::muted(),
        };
        let place = match (&item.path, item.line, item.column) {
            (Some(path), Some(line), Some(column)) => format!("{path}:{line}:{column} "),
            (Some(path), Some(line), None) => format!("{path}:{line} "),
            (Some(path), None, _) => format!("{path} "),
            _ => String::new(),
        };
        out.extend(prefixed_wrapped_line(
            Span::raw("  "),
            Line::styled(
                sanitize(&format!("{} {place}{}", item.severity, item.message)),
                Style::default().fg(color),
            ),
            width as u16,
        ));
    }
}

// ---- terminal ---------------------------------------------------------------

fn terminal_lines(terminal: &TerminalDetail, width: usize, out: &mut Vec<Line<'static>>) {
    let panel = Some(theme::surface_color());
    let inner = width.saturating_sub(INDENT + 2).max(1);
    let mut push = |spans: Vec<Span<'static>>, out: &mut Vec<Line<'static>>| {
        let mut row = vec![Span::raw(" ".repeat(INDENT)), Span::raw(" ")];
        row.extend(spans);
        out.push(banded(row, width, panel));
    };
    let styled = |text: String, color: Color| Span::styled(text, Style::default().fg(color));
    if let Some(directory) = &terminal.working_directory {
        for row in wrap_cells(&format!("cwd {}", sanitize(directory)), inner) {
            push(vec![styled(row, theme::muted())], out);
        }
    }
    if let Some(command) = &terminal.command {
        for (index, line) in logical_lines(&sanitize(command)).into_iter().enumerate() {
            let prompt = if index == 0 { "$ " } else { "> " };
            for (row, segment) in wrap_cells(&line, inner.saturating_sub(2))
                .into_iter()
                .enumerate()
            {
                push(
                    vec![
                        Span::styled(
                            if row == 0 { prompt } else { "  " }.to_owned(),
                            Style::default()
                                .fg(theme::accent())
                                .add_modifier(Modifier::BOLD),
                        ),
                        styled(segment, theme::text()),
                    ],
                    out,
                );
            }
        }
    }
    let streams: Vec<(&str, &OutputStream, Color)> = [
        ("stdout", terminal.stdout.as_ref(), theme::text()),
        ("stderr", terminal.stderr.as_ref(), theme::secondary()),
    ]
    .into_iter()
    .filter_map(|(name, stream, color)| Some((name, stream?, color)))
    .filter(|(_, stream, _)| !stream.text.trim().is_empty())
    .collect();
    let live = matches!(
        terminal.state,
        TerminalState::Running | TerminalState::BackgroundJob
    );
    let mut any_output = false;
    for (name, stream, color) in &streams {
        if streams.len() > 1 || *name == "stderr" {
            push(vec![styled(format!("── {name}"), theme::muted())], out);
        }
        any_output = true;
        output_rows(&stream.text, *color, inner, live, &mut push, out);
        if stream.tail {
            let of = stream
                .bytes
                .map(|bytes| format!(" of {bytes} bytes"))
                .unwrap_or_default();
            push(
                vec![styled(format!("latest {name} output{of}"), theme::muted())],
                out,
            );
        } else if stream.truncated {
            let of = stream
                .bytes
                .map(|bytes| format!(" ({bytes} bytes written)"))
                .unwrap_or_default();
            push(
                vec![styled(format!("{name} truncated{of}"), theme::muted())],
                out,
            );
        }
    }
    if let Some(text) = &terminal.combined {
        any_output = true;
        output_rows(text, theme::text(), inner, live, &mut push, out);
    }
    if streams.len() > 1 {
        for row in wrap_cells(
            "streams are shown separately; their interleaving is not recorded",
            inner,
        ) {
            push(vec![styled(row, theme::muted())], out);
        }
    }
    if terminal.windowed && !live {
        let text = artifact_note(
            "output is a window onto a larger transcript",
            &terminal.artifact_id,
        );
        for row in wrap_cells(&text, inner) {
            push(vec![styled(row, theme::muted())], out);
        }
    } else if terminal.windowed {
        let text = artifact_note(
            "output so far; the command keeps running",
            &terminal.artifact_id,
        );
        for row in wrap_cells(&text, inner) {
            push(vec![styled(row, theme::muted())], out);
        }
    }
    if !any_output
        && matches!(
            terminal.state,
            TerminalState::Succeeded | TerminalState::Failed
        )
    {
        push(vec![styled("(no output)".into(), theme::muted())], out);
    }
    let duration = terminal
        .duration_ms
        .map(|ms| format!(" · {}", format_elapsed(u128::from(ms))))
        .unwrap_or_default();
    let job = terminal
        .job_id
        .as_ref()
        .map(|id| format!(" · job {id}"))
        .unwrap_or_default();
    let exit = terminal.exit_code.map(|code| format!("exit {code}"));
    let (icon, label, color) = match terminal.state {
        TerminalState::Waiting => ("…", "waiting".to_owned(), theme::muted()),
        TerminalState::Running => ("●", "running".to_owned(), theme::accent()),
        TerminalState::BackgroundJob => ("↗", "running in background".to_owned(), theme::accent()),
        TerminalState::Succeeded => (
            "✓",
            exit.unwrap_or_else(|| "completed".into()),
            theme::success(),
        ),
        TerminalState::Failed => (
            "✗",
            exit.unwrap_or_else(|| "failed".into()),
            theme::error_subtle(),
        ),
        TerminalState::Cancelled => ("⊘", "cancelled".to_owned(), theme::muted()),
        TerminalState::TimedOut => ("✗", "timed out".to_owned(), theme::error_subtle()),
        TerminalState::Interrupted => ("⊘", "interrupted".to_owned(), theme::muted()),
    };
    for row in wrap_cells(&format!("{icon} {label}{duration}{job}"), inner) {
        push(vec![styled(row, color)], out);
    }
}

fn output_rows(
    text: &str,
    color: Color,
    inner: usize,
    live: bool,
    push: &mut impl FnMut(Vec<Span<'static>>, &mut Vec<Line<'static>>),
    out: &mut Vec<Line<'static>>,
) {
    let mut lines = logical_lines(&sanitize(text));
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let hidden_note = |hidden: usize| format!("⋯ {hidden} lines hidden; see the raw output");
    let (head, tail): (&[String], &[String]) = if live && lines.len() > LIVE_OUTPUT_ROWS {
        (&[], &lines[lines.len() - LIVE_OUTPUT_ROWS..])
    } else if lines.len() > MAX_OUTPUT_ROWS {
        let half = MAX_OUTPUT_ROWS / 2;
        (&lines[..half], &lines[lines.len() - half..])
    } else {
        (&lines[..], &[])
    };
    let hidden = lines.len() - head.len() - tail.len();
    let rows = |lines: &[String]| -> Vec<String> {
        lines
            .iter()
            .flat_map(|line| wrap_cells(line, inner))
            .collect()
    };
    for segment in rows(head) {
        push(vec![Span::styled(segment, Style::default().fg(color))], out);
    }
    if hidden > 0 {
        push(vec![muted(hidden_note(hidden))], out);
    }
    for segment in rows(tail) {
        push(vec![Span::styled(segment, Style::default().fg(color))], out);
    }
}

// ---- source -----------------------------------------------------------------

fn source_lines(source: &SourceDetail, width: usize, out: &mut Vec<Line<'static>>) {
    for (index, file) in source.files.iter().enumerate() {
        if index > 0 {
            out.push(Line::default());
        }
        let mut spans = vec![Span::styled(
            sanitize(&file.path),
            Style::default()
                .fg(theme::text())
                .add_modifier(Modifier::BOLD),
        )];
        let shown = file
            .lines
            .first()
            .zip(file.lines.last())
            .map(|(first, last)| (first.number, last.number));
        let range = match (
            shown.or(file.start_line.zip(file.end_line)),
            file.total_lines,
        ) {
            (Some((start, end)), Some(total)) if start <= 1 && end >= total => {
                Some(format!("{total} lines"))
            }
            (Some((start, end)), Some(total)) => Some(format!("lines {start}–{end} of {total}")),
            (Some((start, end)), None) => Some(format!("lines {start}–{end}")),
            (None, Some(total)) => Some(format!("{total} lines")),
            (None, None) => None,
        };
        if let Some(range) = range {
            spans.push(muted(format!(" · {range}")));
        }
        out.extend(prefixed_wrapped_line(
            Span::raw("  "),
            Line::from(spans),
            width as u16,
        ));
        if let Some(error) = &file.error {
            out.extend(prefixed_wrapped_line(
                Span::raw("  "),
                Line::styled(sanitize(error), Style::default().fg(theme::error_subtle())),
                width as u16,
            ));
        }
        let last = file.lines.last().map_or(0, |line| line.number);
        let digit_width = digits(last).max(2);
        let gutter = INDENT + digit_width + 3;
        let text_width = width.saturating_sub(gutter).max(1);
        let take = file.lines.len().min(MAX_SOURCE_ROWS);
        for line in &file.lines[..take] {
            let body = logical_lines(&sanitize(&line.text))
                .into_iter()
                .next()
                .unwrap_or_default();
            for (row, segment) in wrap_cells(&body, text_width).into_iter().enumerate() {
                let number = if row == 0 {
                    format!("{:>digit_width$}", line.number)
                } else {
                    " ".repeat(digit_width)
                };
                out.push(Line::from(vec![
                    Span::raw(" ".repeat(INDENT)),
                    muted(format!("{number} │ ")),
                    Span::styled(segment, Style::default().fg(theme::text())),
                ]));
            }
        }
        if take < file.lines.len() {
            note(
                &format!(
                    "⋯ {} more lines; see the raw output",
                    file.lines.len() - take
                ),
                width,
                out,
            );
        }
        match file.availability {
            SourceAvailability::Preview => note(
                &artifact_note(
                    "preview of an externalized read; remaining lines are not included",
                    &file.artifact_id,
                ),
                width,
                out,
            ),
            SourceAvailability::Pending => note("no source lines recorded", width, out),
            _ => {}
        }
        if let Some(next) = file.next_start_line {
            note(&format!("more from line {next}"), width, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::model::{ConversationEntry, ConversationKind, ConversationToolCall};
    use crate::shared_ui::conversation::{ConversationAction, project_tool};
    use ratatui::widgets::{Paragraph, Wrap};
    use serde_json::{Value, json};

    fn tool(
        name: &str,
        args: Value,
        status: &str,
        raw: Option<Value>,
        result: Option<&str>,
    ) -> ConversationToolCall {
        let mut value =
            json!({"id": "t1", "name": name, "status": status, "arguments": args.to_string()});
        if let Some(raw) = raw {
            value["rawResult"] = json!(raw.to_string());
        }
        if let Some(result) = result {
            value["result"] = json!(result);
        }
        serde_json::from_value(value).unwrap()
    }
    fn render(tool: &ConversationToolCall, width: u16) -> Vec<Line<'static>> {
        rich_lines(&project_tool(tool).rich, width)
    }
    fn plain(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|line| line.to_string().trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn edit() -> ConversationToolCall {
        tool(
            "apply_file_edits",
            json!({"changes": [{"path": "src/a.rs"}]}),
            "completed",
            Some(
                json!({"files": [{"path": "src/a.rs", "operation": "update", "diff": {"hunks": [
                    {"oldStart": 8, "oldEnd": 11, "newStart": 8, "newEnd": 12, "truncated": false,
                     "lines": [" fn run() {", "-    let a = 1;", "+    let a = 2;", "+    let b = \"漢字漢字漢字 long long long long long long\";", "     tail()"]},
                    {"oldStart": 120, "oldEnd": 121, "newStart": 121, "newEnd": 122, "truncated": false,
                     "lines": ["-gone", "+here"]}
                ]}}]}),
            ),
            None,
        )
    }

    #[test]
    fn diffs_tint_changes_align_numbers_and_never_lose_wrapped_text() {
        let tool = edit();
        let wide = render(&tool, 80);
        let text = plain(&wide);
        assert!(text.contains("src/a.rs +3 −2"), "{text}");
        assert!(
            text.contains("@@ -8,4 +8,5 @@") && text.contains("@@ -120,2 +121,2 @@"),
            "{text}"
        );
        // Old/new columns line up: " 9      - ..." removed, "     9 + ..." added.
        assert!(text.contains("  9     − "), "{text}");
        assert!(text.contains("     9 + "), "{text}");
        for line in &wide {
            let body = line.to_string();
            if body.contains('−') && body.contains("let a = 1") || body.contains("let a = 2") {
                assert!(
                    line.width() == 80 && line.spans.iter().all(|s| s.style.bg.is_some()),
                    "{body:?}"
                );
            }
        }
        assert!(
            wide.iter()
                .any(|l| l.spans.iter().any(|s| s.style.fg == Some(theme::success())))
        );
        assert!(
            wide.iter()
                .any(|l| l.spans.iter().any(|s| s.style.fg == Some(theme::error())))
        );
        for width in [1u16, 2, 8, 20, 33, 50] {
            let lines = render(&tool, width);
            assert!(
                lines.iter().all(|l| l.width() <= usize::from(width)),
                "width {width}"
            );
            if width >= 20 {
                // Wrapped continuation rows keep the band and every character.
                let joined: String = lines
                    .iter()
                    .filter(|l| {
                        l.spans
                            .iter()
                            .any(|s| s.style.fg == Some(theme::success()) && s.style.bg.is_some())
                    })
                    .flat_map(|l| {
                        l.spans
                            .iter()
                            .rev()
                            .take(2)
                            .map(|s| s.content.to_string())
                            .collect::<Vec<_>>()
                    })
                    .collect();
                assert!(joined.contains("long") || width < 30);
                for line in lines
                    .iter()
                    .filter(|l| l.spans.iter().any(|s| s.style.bg.is_some()))
                {
                    assert_eq!(line.width(), usize::from(width), "{line:?}");
                    assert_eq!(
                        Paragraph::new(line.clone())
                            .wrap(Wrap { trim: false })
                            .line_count(width),
                        1
                    );
                }
            }
        }
        let rows: String = render(&tool, 40).iter().map(ToString::to_string).collect();
        let squashed: String = rows
            .chars()
            .filter(|c| !c.is_whitespace() && *c != PAD)
            .collect();
        assert!(
            squashed.contains("漢字漢字漢字longlonglonglonglonglong"),
            "{squashed}"
        );
    }

    #[test]
    fn diffs_mark_truncation_externalization_and_missing_hunks() {
        let truncated = tool(
            "apply_file_edits",
            json!({}),
            "completed",
            Some(json!({"files": [{"path": "x.rs", "diff": {"hunks": [
            {"oldStart": 1, "oldEnd": 300, "newStart": 1, "newEnd": 300, "truncated": true,
             "lines": [" a", "-b", "+c", " ... diff preview truncated ...", " z"]}]}}]})),
            None,
        );
        let text = plain(&render(&truncated, 80));
        assert!(
            text.contains("preview truncated") && text.contains("omitted"),
            "{text}"
        );
        let external = tool(
            "apply_file_edits",
            json!({"changes": [{"path": "big.rs"}]}),
            "completed",
            Some(json!({"externalized": true, "artifactId": "art-1", "bytes": 5})),
            None,
        );
        let text = plain(&render(&external, 80));
        assert!(
            text.contains("big.rs") && text.contains("artifact art-1") && !text.contains("@@"),
            "{text}"
        );
        let mut failed = tool(
            "apply_file_edits",
            json!({"changes": [{"path": "a.rs"}]}),
            "failed",
            None,
            None,
        );
        failed.error = Some("stale snapshot".into());
        let text = plain(&rich_lines(&project_tool(&failed).rich, 80));
        assert!(
            text.contains("no diff captured")
                && text.contains("Error:")
                && text.contains("stale snapshot"),
            "{text}"
        );
    }

    #[test]
    fn terminal_shows_session_streams_status_and_never_invents_output() {
        let args = json!({"command": "cargo test\n  --quiet", "workingDirectory": "/w"});
        let done = tool(
            "run_shell",
            args.clone(),
            "completed",
            Some(json!({
                "workingDirectory": "/w", "exitCode": 0, "durationMilliseconds": 1500,
                "stdout": "ok\n\u{1b}[31mred\u{1b}[0m\u{1b}]0;title\u{7}\n", "stderr": "warning: noisy\n"
            })),
            None,
        );
        let lines = render(&done, 60);
        let text = plain(&lines);
        assert!(
            text.contains("cwd /w")
                && text.contains("$ cargo test")
                && text.contains(">   --quiet"),
            "{text}"
        );
        assert!(
            text.contains("── stdout")
                && text.contains("── stderr")
                && text.contains("warning: noisy"),
            "{text}"
        );
        assert!(
            text.contains("✓ exit 0 · 1.5s") && text.contains("interleaving is not"),
            "{text}"
        );
        // Escape sequences never reach the terminal.
        assert!(
            text.contains("red") && !text.contains('\u{1b}') && !text.contains("title"),
            "{text:?}"
        );
        let stderr_color = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.contains("noisy"))
            .unwrap()
            .style
            .fg;
        assert_ne!(stderr_color, Some(theme::error()));
        assert!(
            lines
                .iter()
                .filter(|l| l.width() > 0)
                .all(|l| l.width() == 60)
        );

        let failed = tool(
            "run_shell",
            args.clone(),
            "completed",
            Some(json!({"exitCode": 2, "stderr": "boom\n"})),
            None,
        );
        let lines = render(&failed, 60);
        assert!(plain(&lines).contains("✗ exit 2"));
        let status = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.contains("✗ exit 2"))
            .unwrap();
        assert_eq!(status.style.fg, Some(theme::error_subtle()));

        let running = tool("run_shell", args.clone(), "running", None, None);
        let text = plain(&render(&running, 60));
        assert!(
            text.contains("● running") && !text.contains("(no output)") && !text.contains("stdout"),
            "{text}"
        );
        let live = tool("run_shell", args.clone(), "running", None, Some(&json!({
            "status": "running", "elapsedMilliseconds": 2100,
            "progress": {"stdoutBytes": 70000, "stderrBytes": 0, "stdoutTail": "line a\nline b\n", "stderrTail": null}
        }).to_string()));
        let text = plain(&render(&live, 60));
        assert!(
            text.contains("line b")
                && text.contains("latest stdout output of 70000 bytes")
                && text.contains("● running · 2.1s"),
            "{text}"
        );
        let handoff = tool(
            "run_shell",
            args,
            "completed",
            Some(json!({
                "jobId": "job-7", "status": "running", "outputHandedOff": true, "artifactId": "art-9",
                "progress": {"stdoutBytes": 4, "stdoutTail": "so far"}
            })),
            None,
        );
        let text = plain(&render(&handoff, 70));
        assert!(
            text.contains("↗ running in background")
                && text.contains("job job-7")
                && text.contains("artifact art-9"),
            "{text}"
        );
        for width in [1u16, 2, 8, 24] {
            assert!(
                render(&done, width)
                    .iter()
                    .all(|l| l.width() <= usize::from(width))
            );
        }
    }

    #[test]
    fn terminal_output_is_bounded_but_says_what_is_hidden() {
        let output: String = (1..=1000).map(|n| format!("row {n}\n")).collect();
        let done = tool(
            "run_shell",
            json!({"command": "seq"}),
            "completed",
            Some(json!({"exitCode": 0, "stdout": output})),
            None,
        );
        let lines = render(&done, 40);
        let text = plain(&lines);
        assert!(
            lines.len() < 300
                && text.contains("row 1\n")
                && text.contains("row 1000")
                && text.contains("760 lines hidden"),
            "{}",
            lines.len()
        );
        let live = tool(
            "run_shell",
            json!({"command": "seq"}),
            "running",
            None,
            Some(
                &json!({
            "status": "running", "progress": {"stdoutBytes": 9, "stdoutTail": output}})
                .to_string(),
            ),
        );
        let text = plain(&render(&live, 40));
        assert!(
            text.contains("row 1000") && !text.contains("row 1\n") && text.contains("lines hidden")
        );
    }

    #[test]
    fn source_preview_aligns_gutter_and_reports_range_and_partial_reads() {
        let read = tool(
            "read_file",
            json!({"path": "src/main.rs", "startLine": 1, "endLine": 500}),
            "completed",
            Some(json!({
                "path": "src/main.rs", "startLine": 98, "endLine": 101, "totalLines": 640,
                "lines": "98:aa|fn main() {\n99:bb|\n100:cc|\tprintln!(\"héllo 漢字\");\n101:dd|}", "nextStartLine": 102
            })),
            None,
        );
        let text = plain(&render(&read, 60));
        assert!(text.contains("src/main.rs · lines 98–101 of 640"), "{text}");
        assert!(
            text.contains("  98 │ fn main() {")
                && text.contains("  99 │\n")
                && text.contains(" 100 │     println!(\"héllo 漢字\");"),
            "{text}"
        );
        assert!(
            text.contains("more from line 102") && !text.contains("aa|"),
            "{text}"
        );
        for width in [1u16, 2, 8, 10, 24] {
            assert!(
                render(&read, width)
                    .iter()
                    .all(|l| l.width() <= usize::from(width)),
                "width {width}"
            );
        }
        let narrow = plain(&render(&read, 12));
        let squashed: String = narrow
            .chars()
            .filter(|c| !c.is_whitespace() && !"│".contains(*c))
            .collect();
        assert!(squashed.contains("héllo漢字"), "{narrow}");
        let preview = tool(
            "read_file",
            json!({"path": "big.rs"}),
            "completed",
            Some(json!({
            "path": "big.rs", "externalized": true, "artifactId": "art-3", "startLine": 1, "endLine": 900,
            "totalLines": 900, "preview": "1:aa|one\n2:bb|two\n"})),
            None,
        );
        let text = plain(&render(&preview, 70));
        assert!(
            text.contains("preview of an externalized read")
                && text.contains("artifact art-3")
                && text.contains(" 2 │ two"),
            "{text}"
        );
        let pending = tool(
            "read_file",
            json!({"path": "later.rs"}),
            "running",
            None,
            None,
        );
        assert!(plain(&render(&pending, 70)).contains("no source lines recorded"));
    }

    #[test]
    fn expanding_keeps_stable_ids_raw_toggle_geometry_and_cache_correctness() {
        let call = edit();
        let id = call.id.clone();
        let entry = |tool: &ConversationToolCall| ConversationEntry {
            id: tool.id.clone(),
            kind: ConversationKind::ToolCall {
                tool_call: tool.clone(),
            },
        };
        let mut app = App::default();
        app.input_focused = false;
        app.conversation = vec![entry(&call)];
        let collapsed = super::super::shared::content(
            &app,
            &app.application
                .conversation_state()
                .view(&app.conversation, &app.state),
            60,
        );
        assert_eq!(collapsed.0.lines.len(), 1);
        let key = format!("tool:{id}");
        app.apply_conversation_action(ConversationAction::Select(Some(key.clone())));
        app.apply_conversation_action(ConversationAction::Toggle(key.clone()));
        let view = app.application.conversation_projection().view;
        let (text, rows, ids) = super::super::shared::content(&app, &view, 60);
        assert_eq!(ids, view.work_ids());
        assert_eq!(ids, [key.clone(), format!("raw:{key}")]);
        assert!(text.to_string().contains("src/a.rs") && !text.to_string().contains("\"changes\""));
        assert!(
            text.lines[rows[1]]
                .to_string()
                .contains("Raw input and output")
        );
        for width in [0u16, 1, 2, 8, 24, 60, 200] {
            let (text, ..) = super::super::shared::content(&app, &view, width);
            assert!(
                text.lines.iter().all(|l| l.width() <= usize::from(width)),
                "width {width}"
            );
        }
        // Selection survives the raw toggle; raw sections appear only when opened.
        app.apply_conversation_action(ConversationAction::Select(Some(format!("raw:{key}"))));
        app.apply_conversation_action(ConversationAction::Toggle(format!("raw:{key}")));
        let view = app.application.conversation_projection().view;
        let (text, _, ids) = super::super::shared::content(&app, &view, 60);
        assert_eq!(
            app.application.conversation_state().selected.as_deref(),
            Some(format!("raw:{key}").as_str())
        );
        assert_eq!(ids, view.work_ids());
        assert!(text.to_string().contains("Raw result") && text.to_string().contains("\"files\""));
        // Collapsing the tool drops its raw row and clears a selection that vanished.
        app.apply_conversation_action(ConversationAction::Toggle(key));
        let view = app.application.conversation_projection().view;
        assert_eq!(view.work_ids().len(), 1);
    }

    #[test]
    fn sanitizer_strips_escapes_controls_and_rewrites_carriage_returns() {
        let dirty = "a\u{1b}[2J\u{1b}[1;31mb\u{1b}]8;;http://x\u{7}c\u{8}\u{7f}\u{202e}d\r\ne";
        assert_eq!(sanitize(dirty), "abcd\ne");
        assert_eq!(
            logical_lines(&sanitize("10%\r50%\r100%\ndone")),
            ["100%", "done"]
        );
        assert_eq!(logical_lines(&sanitize("abcdef\rXY\r")), ["XYcdef"]);
        assert_eq!(expand_tabs("a\tb"), "a   b");
        assert_eq!(wrap_cells("漢字ab", 3), ["漢", "字a", "b"]);
    }
}
