//! Markdown parsing into styled terminal lines, independent of application state.
use super::theme;
use ratatui::prelude::{Line, Modifier, Span, Style};

pub(in crate::tui::ui) fn markdown_lines(content: &str) -> Vec<Line<'static>> {
    markdown_lines_fit(content, usize::MAX)
}

/// Like [`markdown_lines`], but code blocks are laid out as solid panels that
/// never exceed `width` cells, so their background stays rectangular after
/// the transcript wraps them.
pub(in crate::tui::ui) fn markdown_lines_fit(content: &str, width: usize) -> Vec<Line<'static>> {
    // Preserve every source row so intentional Markdown line breaks survive in
    // the TUI instead of streamed reasoning collapsing into one paragraph.
    let source: Vec<&str> = content.split('\n').collect();
    let mut lines = Vec::new();
    let mut index = 0;

    while index < source.len() {
        let line = source[index];

        if let Some((marker, length, info)) = opening_fence(line) {
            let indent = line.len() - line.trim_start().len();
            index += 1;
            let body_start = index;
            while index < source.len() && !is_closing_fence(source[index], marker, length) {
                index += 1;
            }
            let body = &source[body_start..index];
            // Skip the closing fence; an unfinished (streaming) block simply ends.
            index += 1;
            lines.extend(code_block_lines(indent, &fence_label(info), body, width));
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

const CODE_PADDING: usize = 2;

fn code_block_lines(
    fence_indent: usize,
    label: &str,
    body: &[&str],
    width: usize,
) -> Vec<Line<'static>> {
    let panel_style = Style::default().bg(theme::code_background());
    let body: Vec<String> = body
        .iter()
        .map(|line| strip_indent(line, fence_indent).replace('\t', "    "))
        .collect();

    // Keep the panel nested under its list item while leaving room for code.
    let indent = fence_indent.min(width.saturating_sub(12) / 2);
    let available = width.saturating_sub(indent).max(1);
    let padding = if available >= 16 { CODE_PADDING } else { 0 };
    let natural = body
        .iter()
        .map(|line| cell_width(line))
        .chain([cell_width(label)])
        .max()
        .unwrap_or(0)
        + padding * 2;
    let panel = natural.min(available);
    let inner = panel.saturating_sub(padding * 2).max(1);

    let row = |content: Vec<Span<'static>>, used: usize| {
        let mut spans = Vec::with_capacity(content.len() + 3);
        if indent > 0 {
            spans.push(Span::raw(" ".repeat(indent)));
        }
        spans.push(Span::styled(" ".repeat(padding), panel_style));
        spans.extend(content);
        spans.push(Span::styled(
            " ".repeat(panel.saturating_sub(padding + used)),
            panel_style,
        ));
        Line::from(spans)
    };

    let mut lines = vec![row(
        vec![Span::styled(
            label.to_owned(),
            panel_style
                .fg(theme::muted())
                .add_modifier(Modifier::ITALIC),
        )],
        cell_width(label).min(inner),
    )];
    let syntax = Syntax::for_label(label);
    for line in &body {
        for (content, used) in hard_wrap(highlight_code(line, &syntax, panel_style), inner) {
            lines.push(row(content, used));
        }
    }
    lines.push(row(Vec::new(), 0));
    lines
}

fn strip_indent(line: &str, indent: usize) -> &str {
    let removable = line
        .bytes()
        .take(indent)
        .take_while(|byte| *byte == b' ')
        .count();
    &line[removable..]
}

fn cell_width(value: &str) -> usize {
    Span::raw(value).width()
}

/// Splits styled spans into rows of at most `width` cells, returning each row
/// with the cells it occupies so the caller can pad it to a solid panel.
fn hard_wrap(spans: Vec<Span<'static>>, width: usize) -> Vec<(Vec<Span<'static>>, usize)> {
    let mut rows = vec![(Vec::new(), 0usize)];
    for span in spans {
        let mut chunk = String::new();
        for character in span.content.chars() {
            let cells = cell_width(character.encode_utf8(&mut [0; 4]));
            let (row, used) = rows.last_mut().expect("rows starts non-empty");
            if *used + cells > width && *used > 0 {
                if !chunk.is_empty() {
                    row.push(Span::styled(std::mem::take(&mut chunk), span.style));
                }
                rows.push((Vec::new(), 0));
            }
            chunk.push(character);
            rows.last_mut().expect("rows starts non-empty").1 += cells;
        }
        if !chunk.is_empty() {
            rows.last_mut()
                .expect("rows starts non-empty")
                .0
                .push(Span::styled(chunk, span.style));
        }
    }
    rows
}

struct Syntax {
    enabled: bool,
    line_comments: &'static [&'static str],
    single_quote_strings: bool,
    backtick_strings: bool,
    macros: bool,
}

impl Syntax {
    fn for_label(label: &str) -> Self {
        let label = label.to_ascii_lowercase();
        let plain = matches!(
            label.as_str(),
            "code"
                | "text"
                | "txt"
                | "plain"
                | "plaintext"
                | "output"
                | "console"
                | "log"
                | "diff"
                | "markdown"
                | "md"
        );
        let hash = matches!(
            label.as_str(),
            "py" | "python"
                | "sh"
                | "bash"
                | "zsh"
                | "shell"
                | "fish"
                | "toml"
                | "yaml"
                | "yml"
                | "rb"
                | "ruby"
                | "r"
                | "perl"
                | "make"
                | "makefile"
                | "dockerfile"
                | "nix"
                | "elixir"
                | "ex"
                | "conf"
        );
        let dash = matches!(label.as_str(), "sql" | "lua" | "haskell" | "hs" | "elm");
        let rust = matches!(label.as_str(), "rust" | "rs");
        Self {
            enabled: !plain,
            line_comments: if hash {
                &["#"]
            } else if dash {
                &["--"]
            } else {
                &["//"]
            },
            single_quote_strings: !rust,
            backtick_strings: matches!(
                label.as_str(),
                "js" | "javascript"
                    | "jsx"
                    | "ts"
                    | "typescript"
                    | "tsx"
                    | "go"
                    | "sh"
                    | "bash"
                    | "zsh"
                    | "shell"
            ),
            macros: rust,
        }
    }
}

const KEYWORDS: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "crate",
    "def",
    "default",
    "defer",
    "del",
    "do",
    "elif",
    "else",
    "enum",
    "export",
    "extends",
    "extern",
    "false",
    "final",
    "finally",
    "fn",
    "for",
    "from",
    "func",
    "function",
    "go",
    "guard",
    "if",
    "impl",
    "import",
    "in",
    "interface",
    "is",
    "lambda",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "new",
    "nil",
    "None",
    "not",
    "null",
    "or",
    "and",
    "package",
    "pass",
    "private",
    "protected",
    "pub",
    "public",
    "raise",
    "ref",
    "return",
    "self",
    "Self",
    "static",
    "struct",
    "super",
    "switch",
    "this",
    "throw",
    "throws",
    "trait",
    "true",
    "True",
    "False",
    "try",
    "type",
    "typeof",
    "unsafe",
    "use",
    "var",
    "void",
    "where",
    "while",
    "with",
    "yield",
    "then",
    "fi",
    "done",
    "esac",
    "local",
    "echo",
    "select",
    "insert",
    "update",
    "delete",
    "into",
    "values",
    "end",
    "struct",
    "protocol",
    "extension",
    "init",
    "override",
    "some",
    "any",
];

fn highlight_code(line: &str, syntax: &Syntax, panel: Style) -> Vec<Span<'static>> {
    let plain = panel.fg(theme::text());
    if !syntax.enabled {
        return if line.is_empty() {
            Vec::new()
        } else {
            vec![Span::styled(line.to_owned(), plain)]
        };
    }

    let keyword = panel.fg(theme::accent_hot());
    let string = panel.fg(theme::success());
    let number = panel.fg(theme::accent_warm());
    let comment = panel.fg(theme::muted()).add_modifier(Modifier::ITALIC);
    let function = panel.fg(theme::accent());
    let type_name = panel.fg(theme::warning());

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut push = |text: &str, style: Style| {
        if text.is_empty() {
            return;
        }
        match spans.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push_str(text),
            _ => spans.push(Span::styled(text.to_owned(), style)),
        }
    };

    let bytes = line.as_bytes();
    let mut cursor = 0;
    while cursor < line.len() {
        let rest = &line[cursor..];
        let previous_is_word = line[..cursor]
            .chars()
            .next_back()
            .is_some_and(|value| value.is_alphanumeric() || value == '_');

        if syntax
            .line_comments
            .iter()
            .any(|marker| rest.starts_with(marker))
            && !(rest.starts_with('#') && previous_is_word)
            && !rest.starts_with("#[")
            && !rest.starts_with("#!")
        {
            push(rest, comment);
            break;
        }

        let quote = bytes[cursor];
        let is_string_quote = quote == b'"'
            || (quote == b'\'' && syntax.single_quote_strings)
            || (quote == b'`' && syntax.backtick_strings);
        // Rust char literals such as 'a' or '\n', without catching lifetimes.
        let rust_char = quote == b'\''
            && !syntax.single_quote_strings
            && (rest.get(2..3) == Some("'")
                || (rest.get(1..2) == Some("\\") && rest.get(3..4) == Some("'")));
        if is_string_quote || rust_char {
            let mut end = cursor + 1;
            while end < line.len() {
                match bytes[end] {
                    b'\\' => end += 2,
                    value if value == quote => {
                        end += 1;
                        break;
                    }
                    _ => end += 1,
                }
            }
            let end = end.min(line.len());
            push(&line[cursor..end], string);
            cursor = end;
            continue;
        }

        let first = rest.chars().next().unwrap_or(' ');
        if first.is_ascii_digit() && !previous_is_word {
            let length = rest
                .find(|value: char| {
                    !(value.is_ascii_alphanumeric() || value == '_' || value == '.')
                })
                .unwrap_or(rest.len());
            push(&rest[..length], number);
            cursor += length;
            continue;
        }

        if first.is_alphabetic() || first == '_' {
            let length = rest
                .find(|value: char| !(value.is_alphanumeric() || value == '_'))
                .unwrap_or(rest.len());
            let word = &rest[..length];
            let after = rest[length..].chars().next();
            let style = if KEYWORDS.contains(&word) {
                keyword
            } else if after == Some('(') || (syntax.macros && after == Some('!')) {
                function
            } else if first.is_uppercase() {
                type_name
            } else {
                plain
            };
            push(word, style);
            cursor += length;
            continue;
        }

        let length = first.len_utf8();
        push(&rest[..length], plain);
        cursor += length;
    }
    spans
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
    fn fenced_code_renders_as_a_solid_panel_nested_under_its_list_item() {
        let source = "- Tuple struct:\n  ```rust\n  let p = Point(3, 4);\n  let long_name = 1;\n  ```\nafter";
        let lines = markdown_lines_fit(source, 40);
        let panel: Vec<String> = lines[1..5].iter().map(|line| line.to_string()).collect();
        assert_eq!(panel[0].trim_end(), "    rust");
        assert_eq!(panel[1].trim_end(), "    let p = Point(3, 4);");
        // Every panel row is padded to the same width so the background is a rectangle.
        assert!(
            panel
                .iter()
                .all(|row| cell_width(row) == cell_width(&panel[0]))
        );
        assert_eq!(lines[5].to_string(), "after");
    }

    #[test]
    fn long_code_lines_wrap_inside_the_panel_and_unfinished_fences_stream() {
        let lines = markdown_lines_fit(
            "~~~{.swift}\n**not markdown yet** and a fairly long tail",
            24,
        );
        assert!(lines.iter().all(|line| line.width() <= 24));
        assert!(lines[1].to_string().contains("**not markdown"));
        assert!(
            lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .all(|span| span.style.bg == Some(theme::code_background()))
        );
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
