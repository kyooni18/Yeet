//! Terminal cell sizing, truncation, and styled wrapping.
use super::theme;
use ratatui::prelude::{Line, Span, Style};

pub(in crate::ui) fn cell_width(value: &str) -> usize {
    Span::raw(value).width()
}

fn char_cell_width(value: char) -> usize {
    cell_width(&value.to_string())
}

pub(in crate::ui) fn truncate_end(value: &str, max_cells: usize) -> String {
    if max_cells == 0 {
        return String::new();
    }
    if cell_width(value) <= max_cells {
        return value.to_owned();
    }
    if max_cells == 1 {
        return "…".into();
    }

    let budget = max_cells - 1;
    let mut used = 0usize;
    let mut output = String::new();
    for ch in value.chars() {
        let width = char_cell_width(ch);
        if used + width > budget {
            break;
        }
        output.push(ch);
        used += width;
    }
    output.push('…');
    output
}

pub(in crate::ui) fn truncate_middle(value: &str, max_cells: usize) -> String {
    if max_cells == 0 {
        return String::new();
    }
    if cell_width(value) <= max_cells {
        return value.to_owned();
    }
    if max_cells <= 2 {
        return "…".into();
    }

    let chars = value.chars().collect::<Vec<_>>();
    let widths = chars
        .iter()
        .map(|ch| char_cell_width(*ch))
        .collect::<Vec<_>>();
    let payload = max_cells - 1;
    let left_target = payload / 2;
    let right_target = payload - left_target;

    let mut left_count = 0usize;
    let mut left_width = 0usize;
    while left_count < chars.len() {
        let width = widths[left_count];
        if left_width + width > left_target {
            break;
        }
        left_width += width;
        left_count += 1;
    }

    let mut right_start = chars.len();
    let mut right_width = 0usize;
    while right_start > left_count {
        let width = widths[right_start - 1];
        if right_width + width > right_target {
            break;
        }
        right_width += width;
        right_start -= 1;
    }

    let mut remaining = payload.saturating_sub(left_width + right_width);
    loop {
        let mut progressed = false;
        if left_count < right_start {
            let width = widths[left_count];
            if width <= remaining {
                left_count += 1;
                remaining -= width;
                progressed = true;
            }
        }
        if left_count < right_start {
            let width = widths[right_start - 1];
            if width <= remaining {
                right_start -= 1;
                remaining -= width;
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }

    format!(
        "{}…{}",
        chars[..left_count].iter().collect::<String>(),
        chars[right_start..].iter().collect::<String>()
    )
}

fn transcript_continuation_indent(line: &Line<'static>, content_width: usize) -> usize {
    let Some(first) = line.spans.first() else {
        return 0;
    };
    if first.style.bg == Some(theme::code_background()) {
        return 2.min(content_width.saturating_sub(1));
    }
    let marker = first.content.trim();
    let ordered = marker
        .strip_suffix('.')
        .or_else(|| marker.strip_suffix(')'))
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit()));
    let structural = marker.starts_with('│')
        || marker.starts_with('•')
        || marker.starts_with('☐')
        || marker.starts_with('☑')
        || ordered;
    if structural {
        first.width().min(content_width.saturating_sub(1))
    } else {
        0
    }
}

fn text_runs(value: &str) -> Vec<&str> {
    if value.is_empty() {
        return Vec::new();
    }
    let mut runs = Vec::new();
    let mut start = 0;
    let mut whitespace = None;
    for (index, ch) in value.char_indices() {
        let current = ch.is_whitespace();
        if whitespace.is_some_and(|previous| previous != current) {
            runs.push(&value[start..index]);
            start = index;
        }
        whitespace = Some(current);
    }
    runs.push(&value[start..]);
    runs
}

fn push_styled_text(row: &mut Vec<Span<'static>>, value: &str, style: Style) {
    if value.is_empty() {
        return;
    }
    if let Some(last) = row.last_mut().filter(|last| last.style == style) {
        last.content.to_mut().push_str(value);
    } else {
        row.push(Span::styled(value.to_owned(), style));
    }
}

pub(in crate::ui) fn prefixed_wrapped_line(
    prefix: Span<'static>,
    line: Line<'static>,
    width: u16,
) -> Vec<Line<'static>> {
    let line_style = line.style;
    let prefix_width = prefix.width();
    let width = width as usize;
    if prefix_width >= width {
        let mut visible = String::new();
        let mut used = 0usize;
        for ch in prefix.content.chars() {
            let cells = char_cell_width(ch);
            if used + cells > width {
                break;
            }
            visible.push(ch);
            used += cells;
        }
        return vec![Line::from(Span::styled(visible, prefix.style))];
    }
    let content_width = width - prefix_width;
    let continuation_indent = transcript_continuation_indent(&line, content_width);
    let mut rows = vec![vec![prefix.clone()]];
    let mut used = 0usize;
    let mut has_content = false;

    let new_row = |rows: &mut Vec<Vec<Span<'static>>>| {
        let mut row = vec![prefix.clone()];
        if continuation_indent > 0 {
            row.push(Span::raw(" ".repeat(continuation_indent)));
        }
        rows.push(row);
    };

    for span in line.spans {
        let style = line_style.patch(span.style);
        let text = span.content.into_owned();
        for run in text_runs(&text) {
            let whitespace = run.chars().all(char::is_whitespace);
            let run_width = Span::raw(run).width();
            if whitespace {
                if !has_content && rows.len() > 1 {
                    continue;
                }
                if used + run_width <= content_width {
                    push_styled_text(rows.last_mut().unwrap(), run, style);
                    used += run_width;
                    has_content = true;
                } else {
                    new_row(&mut rows);
                    used = continuation_indent;
                    has_content = false;
                }
                continue;
            }

            if has_content && used + run_width > content_width {
                new_row(&mut rows);
                used = continuation_indent;
                has_content = false;
            }
            if used + run_width <= content_width {
                push_styled_text(rows.last_mut().unwrap(), run, style);
                used += run_width;
                has_content = true;
                continue;
            }

            for ch in run.chars() {
                let value = ch.to_string();
                let cells = Span::raw(&value).width();
                if used + cells > content_width {
                    if has_content {
                        new_row(&mut rows);
                        used = continuation_indent;
                        has_content = false;
                    }
                    if used + cells > content_width {
                        push_styled_text(rows.last_mut().unwrap(), "…", style);
                        break;
                    }
                }
                push_styled_text(rows.last_mut().unwrap(), &value, style);
                used += cells;
                has_content = true;
            }
        }
    }

    rows.into_iter().map(Line::from).collect()
}

pub(in crate::ui) fn compact_number(value: u64) -> String {
    if value >= 1_000_000 {
        compact_scaled(value, 1_000_000, "M")
    } else if value >= 1_000 {
        compact_scaled(value, 1_000, "k")
    } else {
        value.to_string()
    }
}

fn compact_scaled(value: u64, divisor: u64, suffix: &str) -> String {
    let scaled = value as f64 / divisor as f64;
    if scaled >= 100.0 || (scaled.fract() * 10.0).round() == 0.0 {
        format!("{}{suffix}", scaled.round() as u64)
    } else {
        format!("{scaled:.1}{suffix}")
    }
}

pub(in crate::ui) fn format_elapsed(elapsed_ms: u128) -> String {
    if elapsed_ms < 60_000 {
        format!("{:.1}s", elapsed_ms as f64 / 1_000.0)
    } else {
        let seconds = elapsed_ms / 1_000;
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    }
}
