//! Conversation-view mouse selection, scoped context menu, and clipboard requests.

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::{App, Mode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptContextMenu {
    pub x: u16,
    pub y: u16,
    pub entry_id: Option<String>,
}

impl App {
    pub(crate) fn apply_conversation_action(
        &mut self,
        action: crate::shared_ui::conversation::ConversationAction,
    ) {
        let prepared =
            self.application
                .prepare_conversation_entries(action, &self.conversation, &self.state);
        assert!(
            prepared.effect.command.is_none(),
            "Harness transcript actions require send_conversation_action"
        );
        let (_, effect) =
            self.application
                .commit_conversation_entries(prepared, &self.conversation, &self.state);
        if let Some(copy) = effect.copy {
            self.clipboard_request = Some(copy.text);
        }
        if let Some(draft) = effect.edit_draft {
            self.cursor = draft.chars().count();
            self.input = draft;
            self.history_index = None;
            self.history_draft.clear();
            self.input_focused = true;
        }
    }

    /// Message control adapter: commit shared intent after successful Harness delivery.
    pub(crate) fn send_conversation_action(
        &mut self,
        backend: &mut crate::backend::Backend,
        action: crate::shared_ui::conversation::ConversationAction,
    ) -> anyhow::Result<()> {
        let prepared =
            self.application
                .prepare_conversation_entries(action, &self.conversation, &self.state);
        if let Some(command) = prepared.effect.command.clone() {
            backend.send(command)?;
        }
        let (_, effect) =
            self.application
                .commit_conversation_entries(prepared, &self.conversation, &self.state);
        if let Some(copy) = effect.copy {
            self.clipboard_request = Some(copy.text);
        }
        if let Some(draft) = effect.edit_draft {
            self.cursor = draft.chars().count();
            self.input = draft;
            self.history_index = None;
            self.history_draft.clear();
            self.input_focused = true;
        }
        Ok(())
    }
    /// Navigate rendered work headers, not the composer or hidden/suppressed entries.
    pub(crate) fn handle_work_selection_key(&mut self, event: &crossterm::event::KeyEvent) -> bool {
        use crossterm::event::KeyCode;
        if self.input_focused
            || self.sidebar_focus
            || self.home_visible()
            || event
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        match event.code {
            KeyCode::Up | KeyCode::Char('k') => self.select_work(-1),
            KeyCode::Down | KeyCode::Char('j') => self.select_work(1),
            KeyCode::Enter | KeyCode::Char(' ' | 'e') if self.selected_work.is_some() => {
                let index = self.selected_work.unwrap();
                if let Some(id) = self.work_ids.get(index).cloned() {
                    self.apply_conversation_action(
                        crate::shared_ui::conversation::ConversationAction::Toggle(id),
                    );
                }
                self.clear_transcript_selection();
            }
            _ => return false,
        }
        true
    }

    fn select_work(&mut self, direction: isize) {
        if self.work_rows.is_empty() {
            if direction < 0 {
                self.scroll_up(3);
            } else {
                self.scroll_down(3);
            }
            return;
        }
        let index = if let Some(current) = self.selected_work {
            current
                .saturating_add_signed(direction)
                .min(self.work_rows.len() - 1)
        } else if direction > 0 {
            self.work_rows
                .iter()
                .position(|row| *row >= self.scroll_y)
                .unwrap_or(self.work_rows.len() - 1)
        } else {
            let bottom = self.scroll_y.saturating_add(self.transcript_area.3);
            self.work_rows
                .iter()
                .rposition(|row| *row < bottom)
                .unwrap_or(0)
        };
        self.selected_work = Some(index);
        if let Some(id) = self.work_ids.get(index).cloned() {
            self.apply_conversation_action(
                crate::shared_ui::conversation::ConversationAction::Select(Some(id)),
            );
        }
        self.follow_tail = false;
        let row = self.work_rows[index];
        let height = self.transcript_area.3.max(1);
        if row < self.scroll_y {
            self.scroll_y = row;
        } else if row >= self.scroll_y.saturating_add(height) {
            self.scroll_y = row.saturating_sub(height - 1).min(self.max_scroll);
        }
        self.clear_transcript_selection();
    }

    pub fn handle_mouse(&mut self, event: MouseEvent) {
        let _ = self.handle_mouse_inner(event, None);
    }

    pub fn handle_mouse_with_backend(
        &mut self,
        event: MouseEvent,
        backend: &mut crate::backend::Backend,
    ) -> anyhow::Result<()> {
        self.handle_mouse_inner(event, Some(backend))
    }

    fn handle_mouse_inner(
        &mut self,
        event: MouseEvent,
        mut backend: Option<&mut crate::backend::Backend>,
    ) -> anyhow::Result<()> {
        if matches!(event.kind, MouseEventKind::Up(MouseButton::Left))
            && std::mem::take(&mut self.context_menu_click)
        {
            return Ok(());
        }
        // An overlay owns its clicks before the composer, sidebar, and tabs.
        if self.transcript_context_menu.is_some()
            && matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
        {
            self.context_menu_click = true;
            if self.point_in_transcript_context_menu(event.column, event.row) {
                self.activate_transcript_context_menu(event.row, backend.as_deref_mut())?;
            } else {
                self.transcript_context_menu = None;
                self.transcript_context_menu_area = (0, 0, 0, 0);
            }
            return Ok(());
        }
        if self.transcript_context_menu.is_some()
            && matches!(event.kind, MouseEventKind::Up(MouseButton::Left))
        {
            return Ok(());
        }
        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            let targets = if self.mode == Mode::Views {
                &self.view_targets
            } else if matches!(
                self.mode,
                Mode::Chat | Mode::Files | Mode::Diff | Mode::Agents
            ) {
                &self.tab_targets
            } else {
                return Ok(());
            };
            if let Some(tab) = targets
                .iter()
                .find(|(area, _)| area.contains((event.column, event.row).into()))
                .map(|(_, tab)| *tab)
            {
                self.activate_workbench_tab(tab);
                return Ok(());
            }
        }
        if self.mode == Mode::Diff {
            self.handle_diff_mouse(event);
            return Ok(());
        }
        if self.mode == Mode::Agents {
            if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
                && let Some(action) = self.agent_action_at(event.column, event.row)
            {
                self.workbench_command = self.apply_agent_action(action);
            } else if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
                && self.point_in_composer(event.column, event.row)
            {
                self.input_focused = true;
            }
            return Ok(());
        }
        if self.mode != Mode::Chat {
            return Ok(());
        }
        if self.home_visible() && !self.home_targets.is_empty() {
            if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
                && let Some(action) = self
                    .home_targets
                    .iter()
                    .find(|(area, _)| area.contains((event.column, event.row).into()))
                    .map(|(_, action)| action.clone())
            {
                self.apply_home_action(action);
                return Ok(());
            }
            if matches!(
                event.kind,
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
            ) {
                self.home
                    .select_next(if event.kind == MouseEventKind::ScrollUp {
                        -3
                    } else {
                        3
                    });
                if let Some(target) = self.home.selected.clone() {
                    self.apply_home_action(crate::shared_ui::home::HomeAction::Select(target));
                }
                self.input_focused = false;
                return Ok(());
            }
        }
        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
            && self.point_in_composer(event.column, event.row)
        {
            self.clear_transcript_selection();
            self.input_focused = true;
            self.sidebar_focus = false;
            let (x, y, _, _) = self.composer_area;
            let row = self
                .composer_scroll
                .saturating_add(event.row.saturating_sub(y) as usize);
            let column = event.column.saturating_sub(x) as usize;
            self.cursor =
                crate::text_layout::cursor_for_point(&self.input, self.composer_width, row, column);
            return Ok(());
        }
        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
            && let Some(session_id) = self.sidebar_session_at(event.column, event.row)
        {
            self.clear_transcript_selection();
            self.sidebar_load_request = Some(session_id);
            self.follow_tail = true;
            return Ok(());
        }
        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
            && self.point_in_transcript_context_menu(event.column, event.row)
        {
            self.activate_transcript_context_menu(event.row, backend.as_deref_mut())?;
            return Ok(());
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
                if let Some(start) = self.selection_start {
                    let end = self.clamp_to_transcript(event.column, event.row);
                    if end == start {
                        self.clear_transcript_selection();
                        if !self.input_focused {
                            let row = self
                                .scroll_y
                                .saturating_add(event.row.saturating_sub(self.transcript_area.1));
                            if let Some(index) =
                                self.work_rows.iter().position(|target| *target == row)
                            {
                                self.selected_work = Some(index);
                                if let Some(id) = self.work_ids.get(index).cloned() {
                                    self.apply_conversation_action(
                                        crate::shared_ui::conversation::ConversationAction::Select(
                                            Some(id),
                                        ),
                                    );
                                }
                                self.follow_tail = false;
                            }
                        }
                    } else {
                        self.selection_end = Some(end);
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) if inside => {
                if self.selected_transcript_text().is_none() {
                    let (x, _, width, _) = self.transcript_area;
                    self.selection_start = Some((x, event.row));
                    self.selection_end = Some((x + width.saturating_sub(1), event.row));
                }
                self.transcript_context_menu = Some(TranscriptContextMenu {
                    x: event.column,
                    y: event.row,
                    entry_id: self
                        .transcript_cache
                        .as_ref()
                        .and_then(|cache| {
                            let (_, transcript_y, _, _) = self.transcript_area;
                            let row = self
                                .scroll_y
                                .saturating_add(event.row.saturating_sub(transcript_y))
                                as usize;
                            cache.entry_at_row(row)
                        })
                        .map(str::to_owned),
                });
            }
            MouseEventKind::ScrollUp if inside => {
                self.scroll_up(mouse_scroll_amount(event.modifiers))
            }
            MouseEventKind::ScrollDown if inside => {
                self.scroll_down(mouse_scroll_amount(event.modifiers))
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.transcript_context_menu = None;
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn set_sidebar_session_targets(
        &mut self,
        area: (u16, u16, u16, u16),
        targets: Vec<(u16, String)>,
    ) {
        self.sidebar_area = area;
        self.sidebar_session_targets = targets;
    }

    pub fn take_sidebar_load_request(&mut self) -> Option<String> {
        self.sidebar_load_request.take()
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

        while lines.first().is_some_and(|line| line.is_empty()) {
            lines.remove(0);
        }
        while lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }
        let text = lines.join("\n");
        (!text.trim().is_empty()).then_some(text)
    }

    pub fn take_clipboard_paste_request(&mut self) -> bool {
        let requested = std::mem::take(&mut self.clipboard_paste_request);
        if requested && self.mode == Mode::Chat {
            self.input_focused = true;
            self.sidebar_focus = false;
        }
        requested
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

    fn sidebar_session_at(&self, column: u16, row: u16) -> Option<String> {
        let (x, y, width, height) = self.sidebar_area;
        if width == 0
            || height == 0
            || column < x
            || column >= x.saturating_add(width)
            || row < y
            || row >= y.saturating_add(height)
        {
            return None;
        }
        self.sidebar_session_targets
            .iter()
            .find_map(|(target_row, session_id)| (*target_row == row).then(|| session_id.clone()))
    }

    fn point_in_composer(&self, column: u16, row: u16) -> bool {
        let (x, y, width, height) = self.composer_area;
        width > 0
            && height > 0
            && column >= x
            && column < x.saturating_add(width)
            && row >= y
            && row < y.saturating_add(height)
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

    fn activate_transcript_context_menu(
        &mut self,
        row: u16,
        backend: Option<&mut crate::backend::Backend>,
    ) -> anyhow::Result<()> {
        let (_, y, _, height) = self.transcript_context_menu_area;
        if height < 5 {
            return Ok(());
        }
        match row.saturating_sub(y) {
            1 => {
                self.copy_transcript_selection();
            }
            2 => {
                self.clipboard_paste_request = true;
                self.clear_transcript_selection();
            }
            3 => self.clear_transcript_selection(),
            control_row @ 4.. => {
                let Some(entry_id) = self
                    .transcript_context_menu
                    .as_ref()
                    .and_then(|menu| menu.entry_id.as_deref())
                else {
                    return Ok(());
                };
                let view = self.application.conversation_projection().view;
                let action = view.items.into_iter().find_map(|item| match item {
                    crate::shared_ui::conversation::DisplayItem::Entry {
                        entry, controls, ..
                    } if entry.id == entry_id => controls
                        .into_iter()
                        .nth((control_row - 4) as usize)
                        .filter(|control| control.enabled)
                        .map(|control| control.action),
                    _ => None,
                });
                if let Some(action) = action {
                    if let Some(backend) = backend {
                        self.send_conversation_action(backend, action)?;
                    } else if matches!(
                        &action,
                        crate::shared_ui::conversation::ConversationAction::Copy(_)
                            | crate::shared_ui::conversation::ConversationAction::Edit(_)
                    ) {
                        self.apply_conversation_action(action);
                    }
                    self.transcript_context_menu = None;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn mouse_scroll_amount(modifiers: KeyModifiers) -> u16 {
    if modifiers.contains(KeyModifiers::CONTROL) {
        10
    } else if modifiers.contains(KeyModifiers::SHIFT) {
        1
    } else {
        3
    }
}
