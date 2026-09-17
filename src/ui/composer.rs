//! Character wrapping shared by composer measurement, rendering, and cursor placement.
use ratatui::text::Span;
use unicode_segmentation::UnicodeSegmentation;

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
    let mut char_index = 0;

    for grapheme in UnicodeSegmentation::graphemes(input, true) {
        let grapheme_chars = grapheme.chars().count();
        let is_newline = grapheme == "\n" || grapheme == "\r\n";
        let text = if grapheme == "\t" {
            "    ".to_owned()
        } else {
            grapheme.to_owned()
        };
        let cells = Span::raw(&text).width().min(width);

        if !is_newline && cells > 0 && column + cells > width {
            lines.push(String::new());
            column = 0;
        }

        if cursor >= char_index && cursor < char_index + grapheme_chars {
            let offset = cursor - char_index;
            let prefix = grapheme.chars().take(offset).collect::<String>();
            let prefix_width = Span::raw(prefix).width().min(cells);
            position = Some((lines.len() - 1, column + prefix_width));
        }

        if is_newline {
            lines.push(String::new());
            column = 0;
        } else {
            lines.last_mut().unwrap().push_str(&text);
            column += cells;
        }
        char_index += grapheme_chars;
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

    #[test]
    fn cursor_keeps_emoji_grapheme_clusters_on_one_visual_cell_run() {
        let family = "👨‍👩‍👧‍👦";
        assert_eq!(Span::raw(family).width(), 2);

        let laid_out = layout(family, family.chars().count(), 4);
        assert_eq!(laid_out.lines, [family]);
        assert_eq!((laid_out.row, laid_out.column), (0, 2));
    }
}
