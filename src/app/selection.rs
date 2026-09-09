//! Conversation-view mouse selection, scoped context menu, and clipboard requests.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use super::{App, Mode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptContextMenu {
    pub x: u16,
    pub y: u16,
}

impl App {
    pub fn handle_mouse(&mut self, event: MouseEvent) {
        if self.mode != Mode::Chat {
            return;
        }
        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
            && self.point_in_transcript_context_menu(event.column, event.row)
        {
            self.activate_transcript_context_menu(event.row);
            return;
        }
        let inside = self.point_in_transcript(event.column, event.row);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if inside => {
                self.transcript_context_menu = None;
                let point = self.clamp_to_transcript(event.column, event.row);
                self.selection_start = Some(point);
                self.selection_end = self.selection_start;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.selection_start.is_some() => {
                self.selection_end = Some(self.clamp_to_transcript(event.column, event.row));
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.selection_start.is_some() {
                    self.selection_end = Some(self.clamp_to_transcript(event.column, event.row));
                }
            }
            MouseEventKind::Down(MouseButton::Right) if inside => {
                self.transcript_context_menu = Some(TranscriptContextMenu {
                    x: event.column,
                    y: event.row,
                });
            }
            MouseEventKind::ScrollUp if inside => self.scroll_up(3),
            MouseEventKind::ScrollDown if inside => self.scroll_down(3),
            MouseEventKind::Down(MouseButton::Left) => {
                self.transcript_context_menu = None;
            }
            _ => {}
        }
    }

    pub fn selected_transcript_text(&self) -> Option<String> {
        let (start, end) = self.normalized_selection()?;
        let (x, y, width, height) = self.transcript_area;
        if width == 0 || height == 0 || self.transcript_cells.is_empty() {
            return None;
        }

        let mut lines = Vec::new();
        for row in start.0..=end.0 {
            let row_index = row.saturating_sub(y) as usize;
            let Some(cells) = self.transcript_cells.get(row_index) else {
                continue;
            };
            let first_column = if row == start.0 { start.1 } else { x };
            let last_column = if row == end.0 {
                end.1
            } else {
                x.saturating_add(width).saturating_sub(1)
            };
            let first = first_column.saturating_sub(x) as usize;
            let last = last_column.saturating_sub(x) as usize;
            if first >= cells.len() {
                lines.push(String::new());
                continue;
            }
            let mut line = String::new();
            for symbol in cells
                .iter()
                .take(last.min(cells.len().saturating_sub(1)) + 1)
                .skip(first)
            {
                line.push_str(symbol);
            }
            lines.push(line.trim_end().to_owned());
        }

        let text = lines.join("\n");
        (!text.trim().is_empty()).then_some(text)
    }

    pub fn take_clipboard_request(&mut self) -> Option<String> {
        self.clipboard_request.take()
    }

    pub(super) fn copy_transcript_selection(&mut self) -> bool {
        let Some(text) = self.selected_transcript_text() else {
            return false;
        };
        self.clipboard_request = Some(text);
        self.backend_message = Some("Copied selected conversation text".into());
        self.transcript_context_menu = None;
        true
    }

    pub fn clear_transcript_selection(&mut self) {
        self.selection_start = None;
        self.selection_end = None;
        self.transcript_context_menu = None;
        self.transcript_context_menu_area = (0, 0, 0, 0);
    }

    fn point_in_transcript(&self, column: u16, row: u16) -> bool {
        let (x, y, width, height) = self.transcript_area;
        width > 0
            && height > 0
            && column >= x
            && column < x.saturating_add(width)
            && row >= y
            && row < y.saturating_add(height)
    }

    fn clamp_to_transcript(&self, column: u16, row: u16) -> (u16, u16) {
        let (x, y, width, height) = self.transcript_area;
        if width == 0 || height == 0 {
            return (column, row);
        }
        (
            column.clamp(x, x.saturating_add(width).saturating_sub(1)),
            row.clamp(y, y.saturating_add(height).saturating_sub(1)),
        )
    }

    fn normalized_selection(&self) -> Option<((u16, u16), (u16, u16))> {
        let start = self.selection_start?;
        let end = self.selection_end?;
        let start = (start.1, start.0);
        let end = (end.1, end.0);
        Some((start.min(end), start.max(end)))
    }

    fn point_in_transcript_context_menu(&self, column: u16, row: u16) -> bool {
        let (x, y, width, height) = self.transcript_context_menu_area;
        width > 0
            && height > 0
            && column >= x
            && column < x.saturating_add(width)
            && row >= y
            && row < y.saturating_add(height)
    }

    fn activate_transcript_context_menu(&mut self, row: u16) {
        let (_, y, _, height) = self.transcript_context_menu_area;
        if height < 4 {
            return;
        }
        match row.saturating_sub(y) {
            1 => {
                self.copy_transcript_selection();
            }
            2 => self.clear_transcript_selection(),
            _ => {}
        }
    }
}
