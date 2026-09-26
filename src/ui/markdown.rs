//! Markdown parsing into styled terminal lines, independent of application state.
use super::theme;
use ratatui::prelude::{Line, Modifier, Span, Style};

pub(super) fn markdown_lines(content: &str) -> Vec<Line<'static>> {
    // Preserve every source row so intentional Markdown line breaks survive in
    // the TUI instead of streamed reasoning collapsing into one paragraph.
    let source: Vec<&str> = content.split('\n').collect();
    let mut lines = Vec::new();
    let mut code_fence: Option<(char, usize)> = None;
    let mut index = 0;

    while index < source.len() {
        let line = source[index];

        if let Some((marker, minimum_len)) = code_fence {
            if is_closing_fence(line, marker, minimum_len) {
                lines.push(Line::from(Span::styled(
                    "╰─",
                    Style::default()
                        .fg(theme::border_dim())
                        .bg(theme::code_background()),
                )));
                code_fence = None;
            } else {
                lines.push(Line::from(vec![
                    Span::styled(
                        "│ ",
                        Style::default()
                            .fg(theme::border_dim())
                            .bg(theme::code_background()),
                    ),
                    Span::styled(
                        line.to_owned(),
                        Style::default()
                            .fg(theme::text_dim())
                            .bg(theme::code_background()),
                    ),
                ]));
            }
            index += 1;
            continue;
        }

        if let Some((marker, length, info)) = opening_fence(line) {
            let label = fence_label(info);
            lines.push(Line::from(vec![
                Span::styled(
                    "╭─ ",
                    Style::default()
                        .fg(theme::border_dim())
                        .bg(theme::code_background()),
                ),
                Span::styled(
                    label,
                    Style::default()
                        .fg(theme::accent())
                        .bg(theme::code_background())
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            code_fence = Some((marker, length));
            index += 1;
            continue;
        }

        if index + 1 < source.len() {
            if let Some(level) = setext_heading_level(source[index + 1]) {
                lines.push(heading_line(level, line.trim()));
                index += 2;
                continue;
            }

            if is_table_separator(source[index + 1]) && looks_like_table_row(line) {
                lines.push(table_line(line, true));
                index += 2;
                while index < source.len() && looks_like_table_row(source[index]) {
                    lines.push(table_line(source[index], false));
                    index += 1;
                }
                continue;
            }
        }

        if let Some((level, heading)) = atx_heading(line) {
            lines.push(heading_line(level, heading));
            index += 1;
            continue;
        }

        if is_horizontal_rule(line) {
            lines.push(Line::from(vec![
                Span::styled("◇ ", Style::default().fg(theme::accent_warm())),
                Span::styled(
                    "──────────────────────",
                    Style::default().fg(theme::border_dim()),
                ),
            ]));
            index += 1;
            continue;
        }

        if let Some((depth, quote)) = block_quote(line) {
            let mut spans = vec![Span::styled(
                "▎ ".repeat(depth),
                Style::default().fg(theme::accent_warm()),
            )];
            spans.extend(inline_spans(quote, Style::default().fg(theme::text_dim())));
            lines.push(Line::from(spans));
            index += 1;
            continue;
        }

        if let Some((indent, marker, body)) = list_item(line) {
            let mut spans = vec![Span::styled(
                format!("{}{marker} ", " ".repeat(indent)),
                Style::default().fg(theme::border()),
            )];
            spans.extend(inline_spans(body, Style::default()));
            lines.push(Line::from(spans));
            index += 1;
            continue;
        }

        lines.push(Line::from(inline_spans(line, Style::default())));
        index += 1;
    }

    lines
}

fn opening_fence(line: &str) -> Option<(char, usize, &str)> {
    let trimmed = line.trim_start();
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let length = trimmed.chars().take_while(|value| *value == marker).count();
    if length < 3 {
        return None;
    }
    Some((marker, length, trimmed[length..].trim()))
}

fn fence_label(info: &str) -> String {
    let label = info
        .split_whitespace()
        .next()
        .unwrap_or("code")
        .trim_matches(|character| matches!(character, '{' | '}' | '.'));
    if label.is_empty() {
        "code".to_owned()
    } else {
        label.to_owned()
    }
}

fn is_closing_fence(line: &str, marker: char, minimum_len: usize) -> bool {
    let trimmed = line.trim();
    let length = trimmed.chars().take_while(|value| *value == marker).count();
    length >= minimum_len && trimmed.chars().skip(length).all(char::is_whitespace)
}

fn atx_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|value| *value == '#').count();
    if !(1..=6).contains(&level) || !trimmed[level..].starts_with(' ') {
        return None;
    }
    let heading = trimmed[level..].trim().trim_end_matches('#').trim_end();
    Some((level, heading))
}

fn setext_heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().all(|value| value == '=') {
        Some(1)
    } else if trimmed.chars().all(|value| value == '-') && trimmed.len() >= 3 {
        Some(2)
    } else {
        None
    }
}

fn heading_style(level: usize) -> Style {
    let color = match level {
        1 => theme::accent_hot(),
        2 => theme::accent_warm(),
        3 => theme::accent(),
        _ => theme::text_dim(),
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

fn heading_line(level: usize, heading: &str) -> Line<'static> {
    let marker = match level {
        1 => "◆ ",
        2 => "◇ ",
        3 => "› ",
        _ => "· ",
    };
    let marker_color = match level {
        1 => theme::accent_hot(),
        2 => theme::accent_warm(),
        3 => theme::accent(),
        _ => theme::border(),
    };
    let mut spans = vec![Span::styled(
        marker,
        Style::default()
            .fg(marker_color)
            .add_modifier(Modifier::BOLD),
    )];
    spans.extend(inline_spans(heading, heading_style(level)));
    Line::from(spans)
}

fn is_horizontal_rule(line: &str) -> bool {
    let compact: String = line
        .chars()
        .filter(|value| !value.is_whitespace())
        .collect();
    if compact.len() < 3 {
        return false;
    }
    let Some(marker) = compact.chars().next() else {
        return false;
    };
    matches!(marker, '-' | '*' | '_') && compact.chars().all(|value| value == marker)
}

fn block_quote(line: &str) -> Option<(usize, &str)> {
    let mut rest = line.trim_start();
    let mut depth = 0;
    while let Some(value) = rest.strip_prefix('>') {
        depth += 1;
        rest = value.strip_prefix(' ').unwrap_or(value);
    }
    (depth > 0).then_some((depth, rest))
}

fn list_item(line: &str) -> Option<(usize, String, &str)> {
    let indent = line.len().saturating_sub(line.trim_start().len());
    let trimmed = line.trim_start();

    for prefix in ["- ", "* ", "+ "] {
        if let Some(body) = trimmed.strip_prefix(prefix) {
            if let Some(body) = body.strip_prefix("[ ] ") {
                return Some((indent, "☐".to_owned(), body));
            }
            if let Some(body) = body
                .strip_prefix("[x] ")
                .or_else(|| body.strip_prefix("[X] "))
            {
                return Some((indent, "☑".to_owned(), body));
            }
            return Some((indent, "•".to_owned(), body));
        }
    }

    let marker_end = trimmed
        .char_indices()
        .take_while(|(_, value)| value.is_ascii_digit())
        .last()
        .map(|(index, value)| index + value.len_utf8())?;
    if marker_end == 0 {
        return None;
    }
    let suffix = &trimmed[marker_end..];
    let suffix_length = if suffix.starts_with(". ") || suffix.starts_with(") ") {
        2
    } else {
        0
    };
    if suffix_length == 0 {
        return None;
    }
    Some((
        indent,
        trimmed[..marker_end + 1].to_owned(),
        &trimmed[marker_end + suffix_length..],
    ))
}

fn looks_like_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains('|') && !trimmed.is_empty()
}

fn is_table_separator(line: &str) -> bool {
    let cells = table_cells(line);
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let value = cell
                .trim()
                .trim_start_matches(':')
                .trim_end_matches(':')
                .trim();
            value.len() >= 3 && value.chars().all(|character| character == '-')
        })
}

fn table_cells(line: &str) -> Vec<&str> {
    let trimmed = line.trim().trim_start_matches('|').trim_end_matches('|');
    trimmed.split('|').collect()
}

fn table_line(line: &str, header: bool) -> Line<'static> {
    let cells = table_cells(line);
    let mut spans = Vec::new();
    let base = if header {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" │ ", Style::default().fg(theme::border())));
        }
        spans.extend(inline_spans(cell.trim(), base));
    }
    Line::from(spans)
}

fn inline_spans(content: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut cursor = 0;
    let mut plain_start = 0;

    while cursor < content.len() {
        let rest = &content[cursor..];

        if rest.starts_with('\\') {
            let slash_len = '\\'.len_utf8();
            if let Some(next) = content[cursor + slash_len..].chars().next()
                && next.is_ascii_punctuation()
            {
                flush_inline_text(&mut spans, &content[plain_start..cursor], base);
                spans.push(Span::styled(next.to_string(), base));
                cursor += slash_len + next.len_utf8();
                plain_start = cursor;
                continue;
            }
        }

        if rest.starts_with('`') {
            let ticks = rest.chars().take_while(|value| *value == '`').count();
            let marker = "`".repeat(ticks);
            let body_start = cursor + ticks;
            if let Some(relative_end) = content[body_start..].find(&marker) {
                flush_inline_text(&mut spans, &content[plain_start..cursor], base);
                let body_end = body_start + relative_end;
                spans.push(Span::styled(
                    content[body_start..body_end].to_owned(),
                    base.fg(theme::accent_hot()),
                ));
                cursor = body_end + ticks;
                plain_start = cursor;
                continue;
            }
        }

        if rest.starts_with("**") || rest.starts_with("__") {
            let marker = &rest[..2];
            if marker != "__" || !underscore_is_in_word(content, cursor, 2) {
                let body_start = cursor + 2;
                if let Some(relative_end) = content[body_start..].find(marker) {
                    flush_inline_text(&mut spans, &content[plain_start..cursor], base);
                    let body_end = body_start + relative_end;
                    spans.extend(inline_spans(
                        &content[body_start..body_end],
                        base.add_modifier(Modifier::BOLD),
                    ));
                    cursor = body_end + 2;
                    plain_start = cursor;
                    continue;
                }
            }
        }

        if rest.starts_with("~~") {
            let body_start = cursor + 2;
            if let Some(relative_end) = content[body_start..].find("~~") {
                flush_inline_text(&mut spans, &content[plain_start..cursor], base);
                let body_end = body_start + relative_end;
                spans.extend(inline_spans(
                    &content[body_start..body_end],
                    base.add_modifier(Modifier::CROSSED_OUT),
                ));
                cursor = body_end + 2;
                plain_start = cursor;
                continue;
            }
        }

        if (rest.starts_with('*') && !rest.starts_with("**"))
            || (rest.starts_with('_') && !rest.starts_with("__"))
        {
            let marker = rest.chars().next().unwrap_or('*');
            if marker != '_' || !underscore_is_in_word(content, cursor, 1) {
                let body_start = cursor + marker.len_utf8();
                if let Some(relative_end) = content[body_start..].find(marker) {
                    flush_inline_text(&mut spans, &content[plain_start..cursor], base);
                    let body_end = body_start + relative_end;
                    spans.extend(inline_spans(
                        &content[body_start..body_end],
                        base.add_modifier(Modifier::ITALIC),
                    ));
                    cursor = body_end + marker.len_utf8();
                    plain_start = cursor;
                    continue;
                }
            }
        }

        if rest.starts_with('[')
            && let Some(label_end) = content[cursor + 1..].find("](")
        {
            let label_end = cursor + 1 + label_end;
            let url_start = label_end + 2;
            if let Some(url_end) = content[url_start..].find(')') {
                let url_end = url_start + url_end;
                flush_inline_text(&mut spans, &content[plain_start..cursor], base);
                spans.extend(inline_spans(
                    &content[cursor + 1..label_end],
                    base.fg(theme::accent()).add_modifier(Modifier::UNDERLINED),
                ));
                let url = &content[url_start..url_end];
                if !url.is_empty() {
                    spans.push(Span::styled(format!(" ({url})"), base.fg(theme::muted())));
                }
                cursor = url_end + 1;
                plain_start = cursor;
                continue;
            }
        }

        if rest.starts_with('<')
            && let Some(relative_end) = content[cursor + 1..].find('>')
        {
            let end = cursor + 1 + relative_end;
            let target = &content[cursor + 1..end];
            if target.starts_with("https://") || target.starts_with("http://") {
                flush_inline_text(&mut spans, &content[plain_start..cursor], base);
                spans.push(Span::styled(
                    target.to_owned(),
                    base.fg(theme::accent()).add_modifier(Modifier::UNDERLINED),
                ));
                cursor = end + 1;
                plain_start = cursor;
                continue;
            }
        }

        cursor += rest.chars().next().map(char::len_utf8).unwrap_or(1);
    }

    flush_inline_text(&mut spans, &content[plain_start..], base);
    spans
}

fn flush_inline_text(spans: &mut Vec<Span<'static>>, content: &str, style: Style) {
    if !content.is_empty() {
        spans.push(Span::styled(content.to_owned(), style));
    }
}

fn underscore_is_in_word(content: &str, offset: usize, marker_len: usize) -> bool {
    let before = content[..offset].chars().next_back();
    let after = content[offset + marker_len..].chars().next();
    before.is_some_and(char::is_alphanumeric) && after.is_some_and(char::is_alphanumeric)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fenced_code_renders_as_a_labeled_terminal_panel() {
        let lines = markdown_lines("~~~rust\nlet value = 42;\n~~~");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].to_string(), "╭─ rust");
        assert_eq!(lines[1].to_string(), "│ let value = 42;");
        assert_eq!(lines[2].to_string(), "╰─");
        assert!(
            lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .all(|span| { span.style.bg == Some(theme::code_background()) })
        );
        assert_eq!(lines[0].spans[1].style.fg, Some(theme::accent()));
    }

    #[test]
    fn unfinished_fence_preserves_streaming_code_without_fake_closure() {
        let lines = markdown_lines("~~~{.swift}\n**not markdown yet**");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].to_string(), "╭─ swift");
        assert_eq!(lines[1].to_string(), "│ **not markdown yet**");
    }

    #[test]
    fn headings_quotes_and_rules_share_the_yeet_visual_grammar() {
        let lines = markdown_lines("# Surface\n## Rhythm\n### Motion\n> restraint\n***");
        assert_eq!(lines[0].to_string(), "◆ Surface");
        assert_eq!(lines[1].to_string(), "◇ Rhythm");
        assert_eq!(lines[2].to_string(), "› Motion");
        assert_eq!(lines[3].to_string(), "▎ restraint");
        assert!(lines[4].to_string().starts_with("◇ ─"));
        assert_eq!(lines[0].spans[0].style.fg, Some(theme::accent_hot()));
        assert_eq!(lines[1].spans[0].style.fg, Some(theme::accent_warm()));
    }
}
