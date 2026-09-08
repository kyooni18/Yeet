//! Render terminal buffers as escaped, styled HTML.
use ratatui::{
    buffer::{Buffer, Cell},
    style::{Color, Modifier},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CellStyle {
    fg: Color,
    bg: Color,
    modifier: Modifier,
}

impl From<&Cell> for CellStyle {
    fn from(cell: &Cell) -> Self {
        Self {
            fg: cell.fg,
            bg: cell.bg,
            modifier: cell.modifier,
        }
    }
}

pub(super) fn render_buffer(buffer: &Buffer) -> String {
    let mut html =
        String::with_capacity(buffer.area.width as usize * buffer.area.height as usize * 2);
    let mut in_code_block = false;

    for y in buffer.area.top()..buffer.area.bottom() {
        let mut current_style: Option<CellStyle> = None;
        let mut run = String::new();
        let mut row = String::new();
        let mut is_code_row = false;

        for x in buffer.area.left()..buffer.area.right() {
            let Some(cell) = buffer.cell((x, y)) else {
                continue;
            };
            let style = CellStyle::from(cell);
            is_code_row |= is_code_style(style);
            if current_style != Some(style) {
                if let Some(previous) = current_style {
                    append_run(&mut row, previous, &run);
                    run.clear();
                }
                current_style = Some(style);
            }
            run.push_str(cell.symbol());
        }
        if let Some(style) = current_style {
            append_run(&mut row, style, &run);
        }

        if is_code_row && !in_code_block {
            html.push_str("<div class=\"code-block\"><button class=\"copy-code\" type=\"button\">Copy</button><pre>");
            in_code_block = true;
        } else if !is_code_row && in_code_block {
            html.push_str("</pre></div>");
            in_code_block = false;
        }
        html.push_str(&row);
        if y + 1 < buffer.area.bottom() {
            html.push('\n');
        }
    }
    if in_code_block {
        html.push_str("</pre></div>");
    }
    html
}

fn is_code_style(style: CellStyle) -> bool {
    style.bg == Color::Rgb(30, 34, 42)
}

fn append_run(output: &mut String, style: CellStyle, text: &str) {
    if text.is_empty() {
        return;
    }
    let css = style_css(style);
    if css.is_empty() {
        escape_html_into(output, text);
        return;
    }
    output.push_str("<span style=\"");
    output.push_str(&css);
    output.push_str("\">");
    escape_html_into(output, text);
    output.push_str("</span>");
}

fn escape_html_into(output: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            _ => output.push(character),
        }
    }
}

fn style_css(mut style: CellStyle) -> String {
    let reversed = style.modifier.contains(Modifier::REVERSED);
    if reversed {
        std::mem::swap(&mut style.fg, &mut style.bg);
    }

    let mut css = String::new();
    if reversed && style.fg == Color::Reset {
        css.push_str("color:#101218;");
    } else if let Some(color) = color_css(style.fg) {
        css.push_str("color:");
        css.push_str(&color);
        css.push(';');
    }
    if reversed && style.bg == Color::Reset {
        css.push_str("background:#d8dee9;");
    } else if let Some(color) = color_css(style.bg) {
        css.push_str("background:");
        css.push_str(&color);
        css.push(';');
    }
    if style.modifier.contains(Modifier::BOLD) {
        css.push_str("font-weight:700;");
    }
    if style.modifier.contains(Modifier::DIM) {
        css.push_str("opacity:.65;");
    }
    if style.modifier.contains(Modifier::ITALIC) {
        css.push_str("font-style:italic;");
    }
    let underline = style.modifier.contains(Modifier::UNDERLINED);
    let crossed = style.modifier.contains(Modifier::CROSSED_OUT);
    match (underline, crossed) {
        (true, true) => css.push_str("text-decoration:underline line-through;"),
        (true, false) => css.push_str("text-decoration:underline;"),
        (false, true) => css.push_str("text-decoration:line-through;"),
        (false, false) => {}
    }
    if style.modifier.contains(Modifier::HIDDEN) {
        css.push_str("visibility:hidden;");
    }
    css
}

fn color_css(color: Color) -> Option<String> {
    let value = match color {
        Color::Reset => return None,
        Color::Black => "#1b1d23".into(),
        Color::Red => "#e06c75".into(),
        Color::Green => "#98c379".into(),
        Color::Yellow => "#e5c07b".into(),
        Color::Blue => "#61afef".into(),
        Color::Magenta => "#c678dd".into(),
        Color::Cyan => "#56b6c2".into(),
        Color::Gray => "#abb2bf".into(),
        Color::DarkGray => "#5c6370".into(),
        Color::LightRed => "#ff7a85".into(),
        Color::LightGreen => "#b3df90".into(),
        Color::LightYellow => "#f2d08c".into(),
        Color::LightBlue => "#78bdf8".into(),
        Color::LightMagenta => "#d38ee8".into(),
        Color::LightCyan => "#6fd1dc".into(),
        Color::White => "#e6e9ef".into(),
        Color::Rgb(red, green, blue) => format!("rgb({red},{green},{blue})"),
        Color::Indexed(index) => indexed_color(index),
    };
    Some(value)
}

fn indexed_color(index: u8) -> String {
    const ANSI: [&str; 16] = [
        "#000000", "#800000", "#008000", "#808000", "#000080", "#800080", "#008080", "#c0c0c0",
        "#808080", "#ff0000", "#00ff00", "#ffff00", "#0000ff", "#ff00ff", "#00ffff", "#ffffff",
    ];
    if index < 16 {
        return ANSI[index as usize].into();
    }
    if index >= 232 {
        let level = 8 + (index - 232) * 10;
        return format!("rgb({level},{level},{level})");
    }
    let value = index - 16;
    let component = |step: u8| if step == 0 { 0 } else { 55 + step * 40 };
    let red = component(value / 36);
    let green = component((value % 36) / 6);
    let blue = component(value % 6);
    format!("rgb({red},{green},{blue})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{layout::Rect, style::Style};

    #[test]
    fn render_buffer_escapes_content_and_preserves_style() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 1));
        buffer.set_string(
            0,
            0,
            "<ab>",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        );
        let html = render_buffer(&buffer);
        assert!(html.contains("&lt;ab&gt;"));
        assert!(html.contains("font-weight:700"));
        assert!(!html.contains("<ab>"));
    }
}
