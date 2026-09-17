//! Tests for the parent module.

use super::*;

#[test]
fn debate_form_selects_each_role_independently_and_restores_saved_models() {
    let mut app = App::default();
    app.state.active_model = "p/default".into();
    app.state.available_models = vec![
        "p/default".into(),
        "p/pro".into(),
        "p/con".into(),
        "p/jury".into(),
    ];
    app.open_debate();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    app.edit_debate_form(key(KeyCode::Char('T')));
    app.edit_debate_form(key(KeyCode::Tab));
    app.edit_debate_form(key(KeyCode::Down));
    app.edit_debate_form(key(KeyCode::Tab));
    app.edit_debate_form(key(KeyCode::Down));
    app.edit_debate_form(key(KeyCode::Down));
    app.edit_debate_form(key(KeyCode::Tab));
    app.edit_debate_form(key(KeyCode::Up));
    assert_eq!(app.popup_filter, "T");
    assert_eq!(app.debate_models.pro, "p/pro");
    assert_eq!(app.debate_models.con, "p/con");
    assert_eq!(app.debate_models.jury, "p/jury");
    let saved = app.debate_models.clone();
    let mut debate = crate::debate::DebateState::default();
    debate.models = saved.clone();
    app.state.debate = Some(debate);
    app.close_popup();
    app.open_debate();
    assert_eq!(app.debate_models, saved);
    app.debate_field = 3;
    app.edit_debate_form(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    app.edit_debate_form(key(KeyCode::Char('x')));
    assert_eq!(app.debate_models.jury, "x");
    assert_eq!(app.debate_models.pro, "p/pro");
}

#[test]
fn model_filter_uses_structured_catalog_and_catalog_fallback() {
    let mut app = App::default();
    app.state.model_catalog = vec![
        ModelCatalogItem {
            id: "openai/gpt-5.6-sol".into(),
            provider: "OpenAI Platform".into(),
            model: "gpt-5.6-sol".into(),
            context_length: Some(128_000),
        },
        ModelCatalogItem {
            id: "anthropic/claude-sonnet".into(),
            provider: "Anthropic".into(),
            model: "Claude Sonnet".into(),
            context_length: Some(200_000),
        },
    ];
    app.state.available_models = vec!["legacy/unstructured-model".into()];

    app.popup_filter = "platform".into();
    assert_eq!(app.filtered_models(), vec!["openai/gpt-5.6-sol"]);
    app.popup_filter = "sonnet".into();
    assert_eq!(app.filtered_models(), vec!["anthropic/claude-sonnet"]);

    app.state.model_catalog.clear();
    app.popup_filter = "legacy".into();
    assert_eq!(app.filtered_models(), vec!["legacy/unstructured-model"]);
}

#[test]
fn model_picker_prefers_the_active_model_and_falls_back_to_first() {
    let mut app = App::default();
    app.state.model_catalog = vec![
        ModelCatalogItem {
            id: "openai/first".into(),
            provider: "OpenAI".into(),
            model: "first".into(),
            context_length: None,
        },
        ModelCatalogItem {
            id: "anthropic/current".into(),
            provider: "Anthropic".into(),
            model: "current".into(),
            context_length: None,
        },
        ModelCatalogItem {
            id: "gemini/last".into(),
            provider: "Gemini".into(),
            model: "last".into(),
            context_length: None,
        },
    ];
    app.state.active_model = "anthropic/current".into();

    assert_eq!(app.active_model_picker_index(), 1);

    app.state.active_model = "missing/model".into();
    assert_eq!(app.active_model_picker_index(), 0);
}

#[test]
fn model_picker_supports_paged_and_boundary_navigation() {
    let mut app = App::default();
    app.state.available_models = (0..20)
        .map(|index| format!("provider/model-{index}"))
        .collect();
    app.popup_index = 10;

    assert!(app.navigate_model_picker(KeyCode::Home));
    assert_eq!(app.popup_index, 0);
    assert!(app.navigate_model_picker(KeyCode::End));
    assert_eq!(app.popup_index, 19);
    assert!(app.navigate_model_picker(KeyCode::PageUp));
    assert_eq!(app.popup_index, 11);
    assert!(app.navigate_model_picker(KeyCode::PageDown));
    assert_eq!(app.popup_index, 19);
    assert!(app.navigate_model_picker(KeyCode::Up));
    assert_eq!(app.popup_index, 18);

    app.popup_filter = "model-1".into();
    app.popup_index = 0;
    assert!(app.navigate_model_picker(KeyCode::End));
    assert_eq!(app.popup_index, 10);
    assert!(!app.navigate_model_picker(KeyCode::Enter));
    assert_eq!(app.popup_index, 10);
}

#[test]
fn searchable_pickers_reserve_plain_j_and_k_for_filter_text() {
    let mut app = App::default();
    for mode in [Mode::Models, Mode::Capabilities] {
        app.mode = mode;
        app.popup_filter.clear();
        assert!(
            !app.vim_navigation_active(),
            "{mode:?} must not rewrite initial j/k into navigation"
        );
    }

    app.mode = Mode::Reasoning;
    assert!(app.vim_navigation_active());
}

#[test]
fn capability_picker_supports_paged_and_boundary_navigation() {
    let mut app = App::default();
    app.state.available_capabilities = (0..20)
        .map(|index| CapabilityToggleItem {
            id: format!("cap-{index}"),
            kind: "skill".into(),
            name: format!("Capability {index}"),
            description: format!("Capability description {index}"),
            enabled: index % 2 == 0,
        })
        .collect();
    app.popup_index = 10;

    assert!(app.navigate_capability_picker(KeyCode::Home));
    assert_eq!(app.popup_index, 0);
    assert!(app.navigate_capability_picker(KeyCode::End));
    assert_eq!(app.popup_index, 19);
    assert!(app.navigate_capability_picker(KeyCode::PageUp));
    assert_eq!(app.popup_index, 11);
    assert!(app.navigate_capability_picker(KeyCode::PageDown));
    assert_eq!(app.popup_index, 19);

    app.popup_filter = "Capability 1".into();
    app.popup_index = 0;
    assert!(app.navigate_capability_picker(KeyCode::End));
    assert_eq!(app.popup_index, 10);
    assert!(!app.navigate_capability_picker(KeyCode::Enter));
    assert_eq!(app.popup_index, 10);
}

#[test]
fn sandbox_preset_picker_prefers_the_active_policy() {
    fn sandbox_settings(preset: &str) -> crate::model::SandboxSettingsState {
        crate::model::SandboxSettingsState {
            preset: preset.into(),
            execution_mode: "sandboxed".into(),
            auto_approve: false,
            workspace_mode: "all".into(),
            workspace_paths: Vec::new(),
            scratch_writable: true,
            network_allow: Vec::new(),
            environment: Vec::new(),
            secret_ids: Vec::new(),
            limits: crate::model::SandboxLimitsState {
                wall_time_seconds: 30,
                max_stdout_bytes: 1024,
                max_stderr_bytes: 1024,
                max_memory_bytes: 0,
                max_processes: 0,
            },
        }
    }

    let mut app = App::default();
    assert_eq!(app.sandbox_preset_picker_index(), 0);

    for (preset, expected) in [
        ("safe", 0),
        ("balanced", 1),
        ("unlimited", 2),
        ("custom", 3),
    ] {
        app.state.sandbox_settings = Some(sandbox_settings(preset));
        assert_eq!(app.sandbox_preset_picker_index(), expected, "{preset}");
    }
}

#[test]
fn provider_delete_requires_the_same_selected_provider_twice() {
    let mut app = App::default();
    app.state.provider_configurations = vec![
        ProviderConfigurationItem {
            id: "first".into(),
            base_url: "https://first.example".into(),
            require_api_key: false,
            header_count: 0,
        },
        ProviderConfigurationItem {
            id: "second".into(),
            base_url: "https://second.example".into(),
            require_api_key: true,
            header_count: 1,
        },
    ];

    assert!(app.confirm_provider_delete().is_none());
    assert_eq!(app.pending_provider_delete_id.as_deref(), Some("first"));
    assert_eq!(app.confirm_provider_delete().as_deref(), Some("first"));
    assert!(app.pending_provider_delete_id.is_none());

    assert!(app.confirm_provider_delete().is_none());
    app.popup_index = 1;
    assert!(app.confirm_provider_delete().is_none());
    assert_eq!(app.pending_provider_delete_id.as_deref(), Some("second"));
}

#[test]
fn sandbox_reset_requires_a_second_confirmation() {
    let mut app = App::default();

    assert!(!app.confirm_sandbox_reset());
    assert!(app.pending_sandbox_reset);
    assert!(app.confirm_sandbox_reset());
    assert!(!app.pending_sandbox_reset);
}

#[test]
fn bridge_state_changes_cancel_stale_destructive_confirmations() {
    let sandbox = crate::model::SandboxSettingsState {
        preset: "custom".into(),
        execution_mode: "sandboxed".into(),
        auto_approve: false,
        workspace_mode: "all".into(),
        workspace_paths: Vec::new(),
        scratch_writable: true,
        network_allow: Vec::new(),
        environment: Vec::new(),
        secret_ids: Vec::new(),
        limits: crate::model::SandboxLimitsState {
            wall_time_seconds: 30,
            max_stdout_bytes: 1024,
            max_stderr_bytes: 1024,
            max_memory_bytes: 0,
            max_processes: 0,
        },
    };
    let provider = ProviderConfigurationItem {
        id: "custom".into(),
        base_url: "https://custom.example".into(),
        require_api_key: false,
        header_count: 0,
    };
    let mut app = App::default();
    app.state.provider_configurations = vec![provider.clone()];
    app.state.sandbox_settings = Some(sandbox.clone());
    app.pending_provider_delete_id = Some("custom".into());
    app.pending_sandbox_reset = true;

    app.merge_state(BridgeState {
        provider_configurations: vec![provider],
        sandbox_settings: Some(sandbox.clone()),
        ..BridgeState::default()
    });
    assert_eq!(app.pending_provider_delete_id.as_deref(), Some("custom"));
    assert!(app.pending_sandbox_reset);

    let mut changed_sandbox = sandbox;
    changed_sandbox.preset = "safe".into();
    app.merge_state(BridgeState {
        provider_configurations: Vec::new(),
        sandbox_settings: Some(changed_sandbox),
        ..BridgeState::default()
    });
    assert!(app.pending_provider_delete_id.is_none());
    assert!(!app.pending_sandbox_reset);
}

#[test]
fn streaming_clock_follows_state_transitions() {
    let mut app = App::default();
    app.merge_state(BridgeState {
        is_streaming: true,
        ..BridgeState::default()
    });
    let started = app.stream_started_at.expect("stream should start a clock");

    app.merge_state(BridgeState {
        is_streaming: true,
        ..BridgeState::default()
    });
    assert_eq!(app.stream_started_at, Some(started));

    app.merge_state(BridgeState::default());
    assert!(app.stream_started_at.is_none());
    assert!(app.last_stream_duration.is_some());
    assert_eq!(app.latest_turn_duration(), app.last_stream_duration);
}

#[test]
fn new_command_is_suggested() {
    let app = App {
        input: "/n".into(),
        ..App::default()
    };

    assert!(
        app.command_suggestions()
            .iter()
            .any(|(name, _)| *name == "/new")
    );
}

#[test]
fn goal_command_is_suggested() {
    let app = App {
        input: "/goa".into(),
        ..App::default()
    };

    assert!(
        app.command_suggestions()
            .iter()
            .any(|(name, _)| *name == "/goal")
    );
}

#[test]
fn clear_command_describes_the_notice_it_actually_dismisses() {
    let app = App {
        input: "/clear".into(),
        ..App::default()
    };

    assert!(app.command_suggestions().iter().any(|(name, description)| {
        name == "/clear" && description == "Dismiss latest system notice"
    }));
}

#[test]
fn extension_command_is_suggested_and_parsed() {
    let mut app = App::default();
    app.state.extension_commands = vec![crate::model::ExtensionCommandItem {
        extension_id: "sample-tools".into(),
        command: "/sample".into(),
        description: "Open sample extension".into(),
    }];
    app.input = "/sam".into();

    assert!(
        app.command_suggestions()
            .iter()
            .any(|(name, _)| name == "/sample")
    );
    assert_eq!(
        app.extension_command_invocation("/sample menu"),
        Some(("sample".into(), vec!["menu".into()]))
    );
}

#[test]
fn destructive_command_edits_reset_the_suggestion_selection() {
    let mut app = App {
        input: "/st".into(),
        cursor: 3,
        command_index: 2,
        ..App::default()
    };

    app.backspace();
    assert_eq!(app.input, "/s");
    assert_eq!(app.command_index, 0);

    app.input = "/st".into();
    app.cursor = 2;
    app.command_index = 2;
    app.delete();
    assert_eq!(app.input, "/s");
    assert_eq!(app.command_index, 0);
}

#[test]
fn bracketed_paste_inserts_multiline_chat_text_at_the_cursor() {
    let mut app = App {
        input: "left🙂right".into(),
        cursor: 4,
        command_index: 3,
        history_index: Some(0),
        history_draft: "stale".into(),
        ..App::default()
    };

    app.handle_paste("one\r\ntwo");

    assert_eq!(app.input, "leftone\ntwo🙂right");
    assert_eq!(app.cursor, 11);
    assert_eq!(app.command_index, 0);
    assert!(app.history_index.is_none());
}

#[test]
fn bracketed_paste_uses_literal_text_only_in_editable_popup_fields() {
    let mut app = App {
        mode: Mode::Models,
        popup_filter: "gpt-".into(),
        popup_index: 4,
        ..App::default()
    };
    app.handle_paste("5\r\n.6\t");
    assert_eq!(app.popup_filter, "gpt-5.6");
    assert_eq!(app.popup_index, 0);

    app.mode = Mode::Sessions;
    app.popup_filter = "foreign-".into();
    app.popup_index = 3;
    app.handle_paste("session\r\nid");
    assert_eq!(app.popup_filter, "foreign-sessionid");
    assert_eq!(app.popup_index, 0);

    app.mode = Mode::AuthKey;
    app.editor_fields = vec!["sk-".into()];
    app.handle_paste("project\n");
    assert_eq!(app.editor_fields, vec!["sk-project"]);

    app.mode = Mode::Reasoning;
    app.popup_filter = "unchanged".into();
    app.handle_paste("high");
    assert_eq!(app.popup_filter, "unchanged");
}

#[test]
fn permission_prompt_ctrl_c_interrupts_only_an_active_run() {
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(
        App::permission_prompt_action(&ctrl_c, true),
        Some(PermissionPromptAction::Interrupt)
    );
    assert_eq!(App::permission_prompt_action(&ctrl_c, false), None);

    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let escape = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(
        App::permission_prompt_action(&enter, true),
        Some(PermissionPromptAction::Allow)
    );
    assert_eq!(
        App::permission_prompt_action(&escape, true),
        Some(PermissionPromptAction::Deny)
    );
}

#[test]
fn ctrl_c_interrupts_streaming_non_chat_modes_without_stealing_chat_copy() {
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    for mode in [
        Mode::Models,
        Mode::Sessions,
        Mode::Capabilities,
        Mode::CapabilityDetail,
        Mode::Reasoning,
        Mode::Goal,
        Mode::Auth,
        Mode::AuthKey,
        Mode::Providers,
        Mode::ProviderEdit,
        Mode::Settings,
        Mode::SandboxPresets,
        Mode::SandboxPolicy,
        Mode::SettingsEdit,
        Mode::Status,
        Mode::Help,
        Mode::Debate,
    ] {
        assert!(
            App::should_interrupt_active_non_chat(&ctrl_c, true, mode),
            "Ctrl+C should interrupt a streaming run while {mode:?} is open"
        );
        assert!(!App::should_interrupt_active_non_chat(&ctrl_c, false, mode));
    }

    assert!(!App::should_interrupt_active_non_chat(
        &ctrl_c,
        true,
        Mode::Chat
    ));
    let shifted_ctrl_c = KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert!(!App::should_interrupt_active_non_chat(
        &shifted_ctrl_c,
        true,
        Mode::Help,
    ));
}

#[test]
fn capability_toggle_is_locked_only_while_streaming() {
    let mut app = App::default();
    assert!(app.capability_toggle_available());
    app.state.is_streaming = true;
    assert!(!app.capability_toggle_available());
}

#[test]
fn current_session_activation_does_not_require_reload() {
    let mut app = App::default();
    app.state.current_session_id = Some("session-a".into());
    assert!(!app.session_requires_load("session-a"));
    assert!(app.session_requires_load("session-b"));

    app.state.current_session_id = None;
    assert!(app.session_requires_load("session-a"));
}

#[test]
fn bracketed_paste_is_ignored_while_permission_decisions_are_pending() {
    let mut app = App {
        input: "keep this draft".into(),
        cursor: 4,
        ..App::default()
    };
    app.state.pending_shell_permission = Some(crate::model::ShellPermission {
        id: "permission-1".into(),
        kind: "shell".into(),
        command: "echo safe".into(),
        operation: "run".into(),
        reason: "confirm".into(),
    });

    app.handle_paste("y\nnew text");

    assert_eq!(app.input, "keep this draft");
    assert_eq!(app.cursor, 4);
    assert!(app.state.pending_shell_permission.is_some());
}

#[test]
fn input_history_restores_a_draft_and_deduplicates_adjacent_entries() {
    let mut app = App::default();
    app.record_input_history("first");
    app.record_input_history("second");
    app.record_input_history("second");
    app.input = "unfinished draft".into();
    app.cursor = app.input.chars().count();

    app.history_up();
    assert_eq!(app.input, "second");
    app.history_up();
    assert_eq!(app.input, "first");
    app.history_down();
    assert_eq!(app.input, "second");
    app.history_down();
    assert_eq!(app.input, "unfinished draft");
    assert_eq!(app.cursor, app.input.chars().count());
    assert_eq!(app.input_history, ["first", "second"]);
}

#[test]
fn terminal_editing_word_motion_and_kill_are_unicode_safe() {
    let mut app = App {
        input: "alpha  베타 gamma".into(),
        ..App::default()
    };
    app.cursor = app.input.chars().count();

    app.move_word_left();
    assert_eq!(&app.input[byte_index(&app.input, app.cursor)..], "gamma");
    app.move_word_left();
    assert_eq!(
        &app.input[byte_index(&app.input, app.cursor)..],
        "베타 gamma"
    );
    app.move_word_right();
    assert_eq!(
        &app.input[..byte_index(&app.input, app.cursor)],
        "alpha  베타"
    );

    app.cursor = app.input.chars().count();
    app.delete_word_before_cursor();
    assert_eq!(app.input, "alpha  베타 ");
    assert_eq!(app.cursor, "alpha  베타 ".chars().count());

    app.delete_word_before_cursor();
    assert_eq!(app.input, "alpha  ");
    assert_eq!(app.cursor, "alpha  ".chars().count());
}

#[test]
fn terminal_editing_line_kills_preserve_the_other_side_of_cursor() {
    let mut app = App {
        input: "before after".into(),
        cursor: "before".chars().count(),
        ..App::default()
    };
    app.delete_before_cursor();
    assert_eq!(app.input, " after");
    assert_eq!(app.cursor, 0);

    app.input = "before after".into();
    app.cursor = "before".chars().count();
    app.delete_after_cursor();
    assert_eq!(app.input, "before");
    assert_eq!(app.cursor, "before".chars().count());

    app.input = "first\nsecond\nthird".into();
    app.cursor = "first\nsec".chars().count();
    app.delete_before_cursor();
    assert_eq!(app.input, "first\nond\nthird");
    assert_eq!(app.cursor, "first\n".chars().count());

    app.input = "first\nsecond\nthird".into();
    app.cursor = "first\nsec".chars().count();
    app.delete_after_cursor();
    assert_eq!(app.input, "first\nsec\nthird");
    assert_eq!(app.cursor, "first\nsec".chars().count());
}

#[test]
fn terminal_editing_chords_map_without_stealing_empty_input_scroll_keys() {
    let ctrl = |character| KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL);
    let alt = |code| KeyEvent::new(code, KeyModifiers::ALT);
    let mut app = App {
        input: "one two".into(),
        cursor: "one two".chars().count(),
        ..App::default()
    };

    assert!(app.handle_chat_editing_key(&ctrl('a')));
    assert_eq!(app.cursor, 0);
    assert!(app.handle_chat_editing_key(&ctrl('e')));
    assert_eq!(app.cursor, 7);
    assert!(app.handle_chat_editing_key(&ctrl('b')));
    assert_eq!(app.cursor, 6);
    assert!(app.handle_chat_editing_key(&ctrl('f')));
    assert_eq!(app.cursor, 7);
    assert!(app.handle_chat_editing_key(&alt(KeyCode::Left)));
    assert_eq!(app.cursor, 4);
    assert!(app.handle_chat_editing_key(&alt(KeyCode::Right)));
    assert_eq!(app.cursor, 7);
    assert!(app.handle_chat_editing_key(&ctrl('w')));
    assert_eq!(app.input, "one ");
    assert_eq!(app.cursor, 4);

    app.input = "one two".into();
    app.cursor = 3;
    assert!(app.handle_chat_editing_key(&ctrl('k')));
    assert_eq!(app.input, "one");
    app.input = "one two".into();
    app.cursor = 3;
    assert!(app.handle_chat_editing_key(&ctrl('u')));
    assert_eq!(app.input, " two");
    assert_eq!(app.cursor, 0);

    app.input.clear();
    app.cursor = 0;
    assert!(!app.handle_chat_editing_key(&ctrl('b')));
    assert!(!app.handle_chat_editing_key(&ctrl('f')));
    assert!(!app.handle_chat_editing_key(&alt(KeyCode::Up)));
}

#[test]
fn terminal_multiline_navigation_is_line_local_and_unicode_safe() {
    let plain = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut app = App {
        input: "one\n가나다\nxy".into(),
        cursor: "one\n가나".chars().count(),
        ..App::default()
    };

    assert!(app.handle_chat_editing_key(&plain(KeyCode::Home)));
    assert_eq!(app.cursor, "one\n".chars().count());

    app.cursor = "one\n가나".chars().count();
    assert!(app.handle_chat_editing_key(&plain(KeyCode::End)));
    assert_eq!(app.cursor, "one\n가나다".chars().count());

    app.cursor = "one\n가나".chars().count();
    assert!(app.handle_chat_editing_key(&plain(KeyCode::Up)));
    assert_eq!(app.cursor, "on".chars().count());
    assert!(app.handle_chat_editing_key(&plain(KeyCode::Down)));
    assert_eq!(app.cursor, "one\n가나".chars().count());

    app.input.clear();
    app.cursor = 0;
    assert!(!app.handle_chat_editing_key(&plain(KeyCode::Up)));

    app.input = "/mo".into();
    app.cursor = app.input.chars().count();
    assert!(!app.handle_chat_editing_key(&plain(KeyCode::Down)));
}

#[test]
fn mouse_wheel_scrolls_the_transcript_and_restores_tail_following() {
    let mut app = App {
        max_scroll: 30,
        follow_tail: true,
        scroll_y: 30,
        transcript_area: (0, 0, 80, 20),
        ..App::default()
    };

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 10,
        row: 4,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.scroll_y, 27);
    assert!(!app.follow_tail);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 10,
        row: 4,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.scroll_y, 30);
    assert!(app.follow_tail);
}

#[test]
fn direct_transcript_jumps_clear_screen_relative_selection() {
    let mut app = App {
        max_scroll: 30,
        follow_tail: true,
        scroll_y: 30,
        selection_start: Some((4, 2)),
        selection_end: Some((8, 3)),
        transcript_context_menu: Some(crate::app::TranscriptContextMenu { x: 8, y: 3 }),
        transcript_context_menu_area: (8, 3, 22, 4),
        ..App::default()
    };

    app.jump_to_transcript_start();
    assert_eq!(app.scroll_y, 0);
    assert!(!app.follow_tail);
    assert!(app.selection_start.is_none());
    assert!(app.selection_end.is_none());
    assert!(app.transcript_context_menu.is_none());
    assert_eq!(app.transcript_context_menu_area, (0, 0, 0, 0));

    app.selection_start = Some((5, 4));
    app.selection_end = Some((9, 4));
    app.transcript_context_menu = Some(crate::app::TranscriptContextMenu { x: 9, y: 4 });
    app.transcript_context_menu_area = (9, 4, 22, 4);

    app.jump_to_transcript_end();
    assert_eq!(app.scroll_y, 30);
    assert!(app.follow_tail);
    assert!(app.selection_start.is_none());
    assert!(app.selection_end.is_none());
    assert!(app.transcript_context_menu.is_none());
    assert_eq!(app.transcript_context_menu_area, (0, 0, 0, 0));
}

#[test]
fn transcript_selection_is_scoped_clamped_and_direction_independent() {
    let mut app = App {
        transcript_area: (10, 4, 8, 3),
        transcript_cells: vec![
            "abcdefgh".chars().map(|ch| ch.to_string()).collect(),
            "ijklmnop".chars().map(|ch| ch.to_string()).collect(),
            "qrstuvwx".chars().map(|ch| ch.to_string()).collect(),
        ],
        ..App::default()
    };

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 12,
        row: 4,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 15,
        row: 5,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.selected_transcript_text().as_deref(),
        Some("cdefgh\nijklmn")
    );

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 15,
        row: 5,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 12,
        row: 4,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.selected_transcript_text().as_deref(),
        Some("cdefgh\nijklmn")
    );

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 40,
        row: 40,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.selection_end, Some((17, 6)));
}

#[test]
fn sidebar_session_click_requests_load_without_starting_selection() {
    let mut app = App {
        transcript_area: (20, 5, 6, 2),
        transcript_cells: vec![
            "hello!".chars().map(|ch| ch.to_string()).collect(),
            "world!".chars().map(|ch| ch.to_string()).collect(),
        ],
        ..App::default()
    };
    app.follow_tail = false;
    app.set_sidebar_session_targets((1, 3, 18, 8), vec![(5, "other-session".into())]);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 5,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.selection_start.is_none());
    assert_eq!(
        app.take_sidebar_load_request().as_deref(),
        Some("other-session")
    );
    assert!(app.follow_tail);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 6,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.take_sidebar_load_request().is_none());

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 20,
        row: 5,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 24,
        row: 5,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: 22,
        row: 5,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.transcript_context_menu.is_some());

    app.transcript_context_menu_area = (22, 5, 22, 4);
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 23,
        row: 6,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.take_clipboard_request().as_deref(), Some("hello"));
    assert!(app.transcript_context_menu.is_none());
}
