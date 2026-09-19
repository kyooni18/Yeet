//! Terminal-aware text layout helpers shared by the composer renderer and editor controls.

use ratatui::prelude::Span;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputLayout {
    pub lines: Vec<String>,
    pub row: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CursorStop {
    index: usize,
    row: usize,
    column: usize,
}

pub(crate) fn layout(input: &str, cursor: usize, width: u16) -> InputLayout {
    let width = usize::from(width.max(1));
    let mut lines = vec![String::new()];
    let mut column = 0;
    let mut position = None;
    let mut char_index = 0;

    for grapheme in UnicodeSegmentation::graphemes(input, true) {
        let grapheme_chars = grapheme.chars().count();
        let is_newline = grapheme == "\n" || grapheme == "\r\n";
        let text = display_text(grapheme);
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

pub(crate) fn move_cursor_vertical(input: &str, cursor: usize, width: u16, direction: i8) -> usize {
    let stops = cursor_stops(input, width);
    let Some(current) = nearest_stop(&stops, cursor) else {
        return 0;
    };
    let max_row = stops.last().map(|stop| stop.row).unwrap_or(0);
    let target_row = if direction < 0 {
        current.row.saturating_sub(1)
    } else if direction > 0 {
        current.row.saturating_add(1).min(max_row)
    } else {
        current.row
    };
    if target_row == current.row {
        return current.index;
    }
    closest_stop_in_row(&stops, target_row, current.column)
        .map(|stop| stop.index)
        .unwrap_or(current.index)
}

pub(crate) fn visual_line_edge(input: &str, cursor: usize, width: u16, end: bool) -> usize {
    let stops = cursor_stops(input, width);
    let Some(current) = nearest_stop(&stops, cursor) else {
        return 0;
    };
    let iter = stops.iter().filter(|stop| stop.row == current.row);
    if end {
        iter.max_by_key(|stop| stop.index)
            .map(|stop| stop.index)
            .unwrap_or(current.index)
    } else {
        iter.min_by_key(|stop| stop.index)
            .map(|stop| stop.index)
            .unwrap_or(current.index)
    }
}

pub(crate) fn cursor_for_point(input: &str, width: u16, row: usize, column: usize) -> usize {
    let stops = cursor_stops(input, width);
    let Some(last) = stops.last().copied() else {
        return 0;
    };
    let target_row = row.min(last.row);
    closest_stop_in_row(&stops, target_row, column)
        .map(|stop| stop.index)
        .unwrap_or(last.index)
}

fn cursor_stops(input: &str, width: u16) -> Vec<CursorStop> {
    let width = usize::from(width.max(1));
    let mut stops = vec![CursorStop {
        index: 0,
        row: 0,
        column: 0,
    }];
    let mut row = 0;
    let mut column = 0;
    let mut char_index = 0;

    for grapheme in UnicodeSegmentation::graphemes(input, true) {
        let grapheme_chars = grapheme.chars().count();
        let is_newline = grapheme == "\n" || grapheme == "\r\n";
        let text = display_text(grapheme);
        let cells = Span::raw(&text).width().min(width);

        if !is_newline && cells > 0 && column + cells > width {
            row += 1;
            column = 0;
            replace_or_push_stop(&mut stops, char_index, row, column);
        } else {
            replace_or_push_stop(&mut stops, char_index, row, column);
        }

        char_index += grapheme_chars;
        if is_newline {
            row += 1;
            column = 0;
        } else {
            column += cells;
        }
        replace_or_push_stop(&mut stops, char_index, row, column);
    }

    if column >= width {
        row += 1;
        column = 0;
        replace_or_push_stop(&mut stops, char_index, row, column);
    }

    stops
}

fn display_text(grapheme: &str) -> String {
    if grapheme == "\t" {
        "    ".to_owned()
    } else {
        grapheme.to_owned()
    }
}

fn replace_or_push_stop(stops: &mut Vec<CursorStop>, index: usize, row: usize, column: usize) {
    if let Some(last) = stops.last_mut()
        && last.index == index
    {
        last.row = row;
        last.column = column;
        return;
    }
    stops.push(CursorStop { index, row, column });
}

fn nearest_stop(stops: &[CursorStop], cursor: usize) -> Option<CursorStop> {
    stops
        .iter()
        .copied()
        .min_by_key(|stop| (stop.index.abs_diff(cursor), stop.index > cursor))
}

fn closest_stop_in_row(stops: &[CursorStop], row: usize, column: usize) -> Option<CursorStop> {
    stops
        .iter()
        .copied()
        .filter(|stop| stop.row == row)
        .min_by_key(|stop| (stop.column.abs_diff(column), stop.column > column))
}

pub(crate) fn previous_grapheme_cursor(value: &str, cursor: usize) -> usize {
    let cursor = cursor.min(value.chars().count());
    let mut char_index = 0;
    let mut previous = 0;
    for grapheme in UnicodeSegmentation::graphemes(value, true) {
        if char_index >= cursor {
            break;
        }
        previous = char_index;
        char_index += grapheme.chars().count();
        if char_index >= cursor {
            return previous;
        }
    }
    previous
}

pub(crate) fn next_grapheme_cursor(value: &str, cursor: usize) -> usize {
    let cursor = cursor.min(value.chars().count());
    let mut char_index = 0;
    for grapheme in UnicodeSegmentation::graphemes(value, true) {
        char_index += grapheme.chars().count();
        if char_index > cursor {
            return char_index;
        }
    }
    value.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_preserves_unicode_graphemes_and_exact_width_wraps() {
        let wrapped = layout("가나다abc", 4, 4);
        assert_eq!(wrapped.lines, ["가나", "다ab", "c"]);
        assert_eq!((wrapped.row, wrapped.column), (1, 3));

        let family = "👨‍👩‍👧‍👦";
        let laid_out = layout(family, family.chars().count(), 4);
        assert_eq!(laid_out.lines, [family]);
        assert_eq!((laid_out.row, laid_out.column), (0, 2));

        let edge = layout("abcd", 4, 4);
        assert_eq!((edge.row, edge.column), (1, 0));
    }

    #[test]
    fn vertical_navigation_follows_wrapped_visual_rows() {
        let input = "abcdefghij";
        assert_eq!(move_cursor_vertical(input, 2, 4, 1), 6);
        assert_eq!(move_cursor_vertical(input, 6, 4, -1), 2);

        let korean = "가나다라";
        assert_eq!(move_cursor_vertical(korean, 1, 4, 1), 3);
        assert_eq!(move_cursor_vertical(korean, 3, 4, -1), 1);
    }

    #[test]
    fn visual_edges_and_pointer_mapping_follow_wrapping() {
        let input = "abcdefghij";
        assert_eq!(visual_line_edge(input, 6, 4, false), 4);
        assert_eq!(visual_line_edge(input, 6, 4, true), 7);
        assert_eq!(cursor_for_point(input, 4, 1, 2), 6);
        assert_eq!(cursor_for_point(input, 4, 99, 99), input.chars().count());
    }
}
