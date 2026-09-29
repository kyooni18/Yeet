use super::App;

impl App {
    pub(super) fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.reset_history_navigation();
        let byte = byte_index(&self.input, self.cursor);
        self.input.insert_str(byte, text);
        self.cursor += text.chars().count();
        self.command_index = 0;
    }

    pub(super) fn insert_char(&mut self, character: char) {
        self.reset_history_navigation();
        let byte = byte_index(&self.input, self.cursor);
        self.input.insert(byte, character);
        self.cursor += 1;
    }

    pub(super) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.reset_history_navigation();
        let previous = crate::text_layout::previous_grapheme_cursor(&self.input, self.cursor);
        let start = byte_index(&self.input, previous);
        let end = byte_index(&self.input, self.cursor);
        self.input.replace_range(start..end, "");
        self.command_index = 0;
        self.cursor = previous;
    }

    pub(super) fn delete(&mut self) {
        if self.cursor >= self.input.chars().count() {
            return;
        }
        self.reset_history_navigation();
        let next = crate::text_layout::next_grapheme_cursor(&self.input, self.cursor);
        let start = byte_index(&self.input, self.cursor);
        let end = byte_index(&self.input, next);
        self.input.replace_range(start..end, "");
        self.command_index = 0;
    }

    pub(super) fn move_word_left(&mut self) {
        let characters = self.input.chars().collect::<Vec<_>>();
        let mut cursor = self.cursor.min(characters.len());
        while cursor > 0 && characters[cursor - 1].is_whitespace() {
            cursor -= 1;
        }
        while cursor > 0 && !characters[cursor - 1].is_whitespace() {
            cursor -= 1;
        }
        self.cursor = cursor;
    }

    pub(super) fn move_word_right(&mut self) {
        let characters = self.input.chars().collect::<Vec<_>>();
        let mut cursor = self.cursor.min(characters.len());
        while cursor < characters.len() && characters[cursor].is_whitespace() {
            cursor += 1;
        }
        while cursor < characters.len() && !characters[cursor].is_whitespace() {
            cursor += 1;
        }
        self.cursor = cursor;
    }

    pub(super) fn move_line_start(&mut self) {
        let characters = self.input.chars().collect::<Vec<_>>();
        let (start, _) = line_bounds(&characters, self.cursor);
        self.cursor = start;
    }

    pub(super) fn move_line_end(&mut self) {
        let characters = self.input.chars().collect::<Vec<_>>();
        let (_, end) = line_bounds(&characters, self.cursor);
        self.cursor = end;
    }

    pub(super) fn move_line_up(&mut self) {
        let characters = self.input.chars().collect::<Vec<_>>();
        let cursor = self.cursor.min(characters.len());
        let (start, _) = line_bounds(&characters, cursor);
        if start == 0 {
            return;
        }
        let column = cursor - start;
        let previous_end = start - 1;
        let (previous_start, _) = line_bounds(&characters, previous_end);
        self.cursor = previous_start + column.min(previous_end - previous_start);
    }

    pub(super) fn move_line_down(&mut self) {
        let characters = self.input.chars().collect::<Vec<_>>();
        let cursor = self.cursor.min(characters.len());
        let (start, end) = line_bounds(&characters, cursor);
        if end >= characters.len() {
            return;
        }
        let column = cursor - start;
        let next_start = end + 1;
        let (_, next_end) = line_bounds(&characters, next_start);
        self.cursor = next_start + column.min(next_end - next_start);
    }

    pub(super) fn delete_word_before_cursor(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.reset_history_navigation();
        let end_cursor = self.cursor.min(self.input.chars().count());
        let end = byte_index(&self.input, end_cursor);
        self.move_word_left();
        let start = byte_index(&self.input, self.cursor);
        self.input.replace_range(start..end, "");
        self.command_index = 0;
    }

    pub(super) fn delete_word_after_cursor(&mut self) {
        let start_cursor = self.cursor.min(self.input.chars().count());
        if start_cursor >= self.input.chars().count() {
            return;
        }
        self.reset_history_navigation();
        self.move_word_right();
        let end_cursor = self.cursor;
        let start = byte_index(&self.input, start_cursor);
        let end = byte_index(&self.input, end_cursor);
        self.input.replace_range(start..end, "");
        self.cursor = start_cursor;
        self.command_index = 0;
    }

    pub(super) fn delete_before_cursor(&mut self) {
        let characters = self.input.chars().collect::<Vec<_>>();
        let cursor = self.cursor.min(characters.len());
        let (line_start, _) = line_bounds(&characters, cursor);
        if cursor == line_start {
            return;
        }
        self.reset_history_navigation();
        let start = byte_index(&self.input, line_start);
        let end = byte_index(&self.input, cursor);
        self.input.replace_range(start..end, "");
        self.cursor = line_start;
    }

    pub(super) fn delete_after_cursor(&mut self) {
        let characters = self.input.chars().collect::<Vec<_>>();
        let cursor = self.cursor.min(characters.len());
        let (_, line_end) = line_bounds(&characters, cursor);
        if cursor >= line_end {
            return;
        }
        self.reset_history_navigation();
        let start = byte_index(&self.input, cursor);
        let end = byte_index(&self.input, line_end);
        self.input.replace_range(start..end, "");
        self.cursor = cursor;
    }

    pub(super) fn record_input_history(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if self
            .input_history
            .last()
            .is_some_and(|previous| previous == text)
        {
            self.reset_history_navigation();
            return;
        }
        self.input_history.push(text.to_owned());
        const MAX_INPUT_HISTORY: usize = 100;
        if self.input_history.len() > MAX_INPUT_HISTORY {
            let excess = self.input_history.len() - MAX_INPUT_HISTORY;
            self.input_history.drain(..excess);
        }
        self.reset_history_navigation();
    }

    pub(super) fn history_up(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        if self.history_index.is_none() {
            self.history_draft = self.input.clone();
            self.history_index = Some(self.input_history.len() - 1);
        } else if let Some(index) = self.history_index {
            self.history_index = Some(index.saturating_sub(1));
        }
        if let Some(index) = self.history_index {
            self.input = self.input_history[index].clone();
            self.cursor = self.input.chars().count();
        }
    }

    pub(super) fn history_down(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.input_history.len() {
            self.history_index = Some(index + 1);
            self.input = self.input_history[index + 1].clone();
            self.cursor = self.input.chars().count();
        } else {
            let draft = std::mem::take(&mut self.history_draft);
            self.reset_history_navigation();
            self.input = draft;
            self.cursor = self.input.chars().count();
        }
    }

    pub(super) fn reset_history_navigation(&mut self) {
        self.history_index = None;
        self.history_draft.clear();
    }
}

pub(super) fn byte_index(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(value.len())
}

pub(super) fn line_bounds(characters: &[char], cursor: usize) -> (usize, usize) {
    let cursor = cursor.min(characters.len());
    let start = characters[..cursor]
        .iter()
        .rposition(|character| *character == '\n')
        .map_or(0, |index| index + 1);
    let end = characters[cursor..]
        .iter()
        .position(|character| *character == '\n')
        .map_or(characters.len(), |offset| cursor + offset);
    (start, end)
}
