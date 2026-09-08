//! Character wrapping shared by composer measurement, rendering, and cursor placement.
use ratatui::text::Span;

pub(super) struct InputLayout {
    pub lines: Vec<String>,
    pub row: usize,
    pub column: usize,
}

pub(super) fn layout(input: &str, cursor: usize, width: u16) -> InputLayout {
    let width = usize::from(width.max(1));
    let mut lines = vec![String::new()];
    let mut column = 0;
    let mut position = None;
    for (index, character) in input.chars().enumerate() {
        let text = if character == '\t' {
            "    ".to_owned()
        } else {
            character.to_string()
        };
        let cells = Span::raw(text.clone()).width().min(width);
        if character != '\n' && cells > 0 && column + cells > width {
            lines.push(String::new());
            column = 0;
        }
        if index == cursor {
            position = Some((lines.len() - 1, column));
        }
        if character == '\n' {
            lines.push(String::new());
            column = 0;
        } else {
            lines.last_mut().unwrap().push_str(&text);
            column += cells;
        }
    }
    if column >= width {
        lines.push(String::new());
        column = 0;
    }
    let (row, column) = position.unwrap_or((lines.len() - 1, column));
    InputLayout { lines, row, column }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_tracks_wrapped_korean_and_explicit_newlines() {
        let wrapped = layout("가나다abc", 4, 4);
        assert_eq!(wrapped.lines, ["가나", "다ab", "c"]);
        assert_eq!((wrapped.row, wrapped.column), (1, 3));
        let multiline = layout("ab\n", 3, 8);
        assert_eq!((multiline.row, multiline.column), (1, 0));
        let edge = layout("abcd", 4, 4);
        assert_eq!((edge.row, edge.column), (1, 0));
    }
}
