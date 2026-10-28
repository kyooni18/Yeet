//! Chat focus, keyboard commands, and composer editing.

use super::*;

impl App {
    /// Focus transitions belong to the session view, never to global shortcuts.
    pub(super) fn handle_chat_focus_key(&mut self, event: &KeyEvent) -> bool {
        if event.code == KeyCode::Esc {
            self.input_focused = false;
            if self.transcript_context_menu.is_some() {
                self.transcript_context_menu = None;
                return true;
            }
            if self.selection_start.is_some() || self.selection_end.is_some() {
                self.clear_transcript_selection();
                return true;
            }
        }
        if event.code == KeyCode::Esc {
            self.sidebar_focus = false;
            return true;
        }
        if (!self.input_focused || self.sidebar_focus)
            && event.code == KeyCode::Char('i')
            && event.modifiers.is_empty()
        {
            self.input_focused = true;
            self.sidebar_focus = false;
            return true;
        }
        false
    }

    pub(super) fn handle_chat_key(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        if self.handle_chat_focus_key(&event) {
            return Ok(());
        }
        match self.handle_sidebar_key(&event) {
            sidebar::SidebarKey::Ignored => {}
            sidebar::SidebarKey::Handled => return Ok(()),
            sidebar::SidebarKey::NewSession => {
                self.send_composer_action(
                    backend,
                    crate::shared_ui::composer::ComposerAction::NewSession,
                )?;
                self.home_override = Some(false);
                self.follow_tail = true;
                return Ok(());
            }
        }
        if self.input_focused && !self.sidebar_focus && self.handle_chat_editing_key(&event) {
            return Ok(());
        }
        if event.modifiers.contains(KeyModifiers::CONTROL) {
            match event.code {
                KeyCode::Char('x')
                    if self.input_focused
                        && !self.sidebar_focus
                        && !self.attachments.is_empty() =>
                {
                    self.attachments.pop();
                    self.sync_composer();
                    return Ok(());
                }
                KeyCode::Char('c') => {
                    if self.copy_transcript_selection() {
                        return Ok(());
                    }
                    if self.state.is_streaming {
                        self.send_composer_action(
                            backend,
                            crate::shared_ui::composer::ComposerAction::Interrupt,
                        )?;
                    } else {
                        self.quit = true;
                    }
                    return Ok(());
                }
                KeyCode::Char('d') if self.input.is_empty() => {
                    self.quit = true;
                    return Ok(());
                }
                KeyCode::Char('n') => {
                    self.send_composer_action(
                        backend,
                        crate::shared_ui::composer::ComposerAction::NewSession,
                    )?;
                    self.home_override = Some(false);
                    self.follow_tail = true;
                    return Ok(());
                }
                KeyCode::Char('b') if self.input.is_empty() => {
                    self.scroll_up(10);
                    return Ok(());
                }
                KeyCode::Char('f') if self.input.is_empty() => {
                    self.scroll_down(10);
                    return Ok(());
                }
                KeyCode::Char('p') if self.input_focused => {
                    self.history_up();
                    return Ok(());
                }
                _ => {}
            }
        }

        if self.handle_work_selection_key(&event) {
            return Ok(());
        }

        // Unfocused navigation must not fall through to draft editing or submission.
        if !self.input_focused {
            match event.code {
                KeyCode::Char('j' | 'k' | 'e' | 'g' | 'G' | 's')
                    if !event
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {}
                KeyCode::Up
                | KeyCode::Down
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::F(_) => {}
                KeyCode::Char('m' | 's' | 'f' | 'r' | 'k')
                    if event.modifiers.contains(KeyModifiers::ALT) => {}
                _ => return Ok(()),
            }
        }

        match event.code {
            KeyCode::Esc => {}
            KeyCode::Up if self.input_focused && event.modifiers.contains(KeyModifiers::ALT) => {
                self.history_up()
            }
            KeyCode::Down if self.input_focused && event.modifiers.contains(KeyModifiers::ALT) => {
                self.history_down()
            }
            KeyCode::F(2) | KeyCode::Char('m')
                if event.code == KeyCode::F(2) || event.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.activate_toolbar_control(backend, "models")?;
            }
            KeyCode::Char('s')
                if event.modifiers.contains(KeyModifiers::ALT)
                    || (!self.input_focused && event.modifiers.is_empty()) =>
            {
                self.activate_toolbar_control(backend, "sessions")?;
            }
            KeyCode::Char('f') if event.modifiers.contains(KeyModifiers::ALT) => {
                self.activate_toolbar_control(backend, "files")?;
            }
            KeyCode::F(4) | KeyCode::Char('r')
                if event.code == KeyCode::F(4) || event.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.activate_toolbar_control(backend, "reasoning")?;
            }
            KeyCode::F(5) | KeyCode::Char('k')
                if event.code == KeyCode::F(5) || event.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.activate_toolbar_control(backend, "capabilities")?;
            }
            KeyCode::PageUp => self.scroll_up(10),
            KeyCode::PageDown => self.scroll_down(10),
            KeyCode::Char('k') if !self.input_focused => self.scroll_up(3),
            KeyCode::Char('j') if !self.input_focused => self.scroll_down(3),
            KeyCode::Up if !self.input_focused => self.scroll_up(3),
            KeyCode::Down if !self.input_focused => self.scroll_down(3),
            KeyCode::Char('e') if !self.input_focused => {
                let expanded = !self.application.conversation_state().expand_all;
                self.apply_conversation_action(
                    crate::shared_ui::conversation::ConversationAction::SetExpandAll(expanded),
                );
            }
            KeyCode::Char('g') if !self.input_focused => self.jump_to_transcript_start(),
            KeyCode::Char('G') if !self.input_focused => self.jump_to_transcript_end(),
            KeyCode::End if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.jump_to_transcript_end();
            }
            KeyCode::Home if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.jump_to_transcript_start();
            }
            KeyCode::Up if !self.command_suggestions().is_empty() => {
                self.command_index = self.command_index.saturating_sub(1);
            }
            KeyCode::Down if !self.command_suggestions().is_empty() => {
                let count = self.command_suggestions().len();
                self.command_index = cmp::min(self.command_index + 1, count.saturating_sub(1));
            }
            KeyCode::Tab if !self.command_suggestions().is_empty() => {
                let suggestions = self.command_suggestions();
                let index = cmp::min(self.command_index, suggestions.len() - 1);
                self.input = format!("{} ", suggestions[index].0);
                self.cursor = self.input.chars().count();
                self.command_index = 0;
                self.reset_history_navigation();
            }
            KeyCode::Enter if event.modifiers.contains(KeyModifiers::SHIFT) => {
                self.insert_char('\n');
            }
            KeyCode::Enter => {
                let editor = self.composer_editor_snapshot();
                self.send_composer_action(
                    backend,
                    crate::shared_ui::composer::ComposerAction::Submit(editor),
                )?;
            }
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => {
                self.cursor = crate::text_layout::previous_grapheme_cursor(&self.input, self.cursor)
            }
            KeyCode::Right => {
                self.cursor = crate::text_layout::next_grapheme_cursor(&self.input, self.cursor);
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.chars().count(),
            KeyCode::Char(character)
                if !event.modifiers.contains(KeyModifiers::CONTROL)
                    && !event.modifiers.contains(KeyModifiers::SUPER) =>
            {
                self.insert_char(character);
                self.command_index = 0;
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn handle_chat_editing_key(&mut self, event: &KeyEvent) -> bool {
        if event.modifiers.contains(KeyModifiers::CONTROL) {
            match event.code {
                KeyCode::Char('a') => self.cursor = 0,
                KeyCode::Char('e') => self.cursor = self.input.chars().count(),
                KeyCode::Char('b') if !self.input.is_empty() => {
                    self.cursor =
                        crate::text_layout::previous_grapheme_cursor(&self.input, self.cursor)
                }
                KeyCode::Char('f') if !self.input.is_empty() => {
                    self.cursor = crate::text_layout::next_grapheme_cursor(&self.input, self.cursor)
                }
                KeyCode::Left if !self.input.is_empty() => self.move_word_left(),
                KeyCode::Right if !self.input.is_empty() => self.move_word_right(),
                KeyCode::Char('u') => {
                    self.delete_before_cursor();
                    self.command_index = 0;
                }
                KeyCode::Char('k') => {
                    self.delete_after_cursor();
                    self.command_index = 0;
                }
                KeyCode::Char('w') => {
                    self.delete_word_before_cursor();
                    self.command_index = 0;
                }
                _ => return false,
            }
            return true;
        }

        if event.modifiers.contains(KeyModifiers::ALT) {
            match event.code {
                KeyCode::Left => self.move_word_left(),
                KeyCode::Right => self.move_word_right(),
                KeyCode::Backspace => self.delete_word_before_cursor(),
                KeyCode::Delete => self.delete_word_after_cursor(),
                _ => return false,
            }
            return true;
        }
        if event.modifiers.is_empty() {
            match event.code {
                KeyCode::Home if !self.input.is_empty() => {
                    if self.composer_width > 0 {
                        self.cursor = crate::text_layout::visual_line_edge(
                            &self.input,
                            self.cursor,
                            self.composer_width,
                            false,
                        );
                    } else {
                        self.move_line_start();
                    }
                }
                KeyCode::End if !self.input.is_empty() => {
                    if self.composer_width > 0 {
                        self.cursor = crate::text_layout::visual_line_edge(
                            &self.input,
                            self.cursor,
                            self.composer_width,
                            true,
                        );
                    } else {
                        self.move_line_end();
                    }
                }
                KeyCode::Up if !self.input.is_empty() && self.command_suggestions().is_empty() => {
                    if self.composer_width > 0 {
                        self.cursor = crate::text_layout::move_cursor_vertical(
                            &self.input,
                            self.cursor,
                            self.composer_width,
                            -1,
                        );
                    } else if self.input.contains('\n') {
                        self.move_line_up();
                    }
                }
                KeyCode::Down
                    if !self.input.is_empty() && self.command_suggestions().is_empty() =>
                {
                    if self.composer_width > 0 {
                        self.cursor = crate::text_layout::move_cursor_vertical(
                            &self.input,
                            self.cursor,
                            self.composer_width,
                            1,
                        );
                    } else if self.input.contains('\n') {
                        self.move_line_down();
                    }
                }
                _ => return false,
            }
            return true;
        }

        false
    }
}
