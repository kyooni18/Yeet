//! WebSocket handshake, message delivery, and reconnect handling.

use super::*;

async fn send_ws(socket: &mut WebSocket, message: Message) -> Result<()> {
    tokio::time::timeout(WEBSOCKET_SEND_TIMEOUT, socket.send(message))
        .await
        .map_err(|_| anyhow!("Remote WebSocket send timed out"))?
        .map_err(|error| anyhow!(error.to_string()))
}

async fn send_json(socket: &mut WebSocket, message: &ServerMessage) -> Result<()> {
    let fatal = matches!(message, ServerMessage::Error { fatal: true, .. });
    let text = serde_json::to_string(message)?;
    send_ws(socket, Message::Text(text.into())).await?;
    if fatal {
        send_ws(socket, Message::Close(None)).await?;
    }
    Ok(())
}

pub(crate) async fn serve_socket(mut socket: WebSocket, hub: Arc<RemoteHub>) {
    let first = match tokio::time::timeout(CLIENT_HELLO_TIMEOUT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(first)))) => first,
        Ok(_) => return,
        Err(_) => {
            let _ = send_json(
                &mut socket,
                &ServerMessage::error(
                    "hello_timeout",
                    "timed out waiting for the initial Remote hello message",
                    true,
                    None,
                ),
            )
            .await;
            return;
        }
    };
    let hello = match decode_client_message(&first) {
        Ok(ClientMessage::Hello {
            min_version,
            max_version,
            client_id,
            workspace,
            session_id,
            last_sequence,
            last_revision,
        }) => (
            min_version,
            max_version,
            client_id,
            workspace,
            session_id,
            last_sequence,
            last_revision,
        ),
        Ok(_) => {
            let _ = send_json(
                &mut socket,
                &ServerMessage::error(
                    "hello_required",
                    "the first Remote WebSocket message must be hello",
                    true,
                    None,
                ),
            )
            .await;
            return;
        }
        Err(error) => {
            let _ = send_json(
                &mut socket,
                &ServerMessage::error("malformed_message", error.to_string(), true, None),
            )
            .await;
            return;
        }
    };

    if negotiate_version(hello.0, hello.1).is_err() {
        let _ = send_json(
            &mut socket,
            &ServerMessage::error(
                "protocol_mismatch",
                format!(
                    "client supports Remote protocol {}..{}, server supports version {}",
                    hello.0, hello.1, REMOTE_PROTOCOL_VERSION
                ),
                true,
                None,
            ),
        )
        .await;
        return;
    }
    let workspace = match hub.resolve_workspace(hello.3.as_deref()) {
        Ok(workspace) => workspace,
        Err(error) => {
            let _ = send_json(
                &mut socket,
                &ServerMessage::error("workspace_invalid", error.to_string(), true, None),
            )
            .await;
            return;
        }
    };
    let workspace_display = workspace.to_string_lossy().into_owned();

    let client_id = match hello.2 {
        Some(client_id) if valid_client_id(&client_id) => client_id,
        Some(_) => {
            let _ = send_json(
                &mut socket,
                &ServerMessage::error(
                    "invalid_client_id",
                    "client_id must be 1-128 ASCII letters, digits, '.', '_' or '-'",
                    true,
                    None,
                ),
            )
            .await;
            return;
        }
        None => uuid::Uuid::new_v4().to_string(),
    };
    let runtime_hub = Arc::clone(&hub);
    let runtime_client_id = client_id.clone();
    let runtime_workspace = workspace.clone();
    let runtime = match tokio::task::spawn_blocking(move || {
        runtime_hub.runtime(&runtime_workspace, &runtime_client_id)
    })
    .await
    {
        Ok(Ok(runtime)) => runtime,
        Ok(Err(error)) => {
            let _ = send_json(
                &mut socket,
                &ServerMessage::error("backend_unavailable", format!("{error:#}"), true, None),
            )
            .await;
            return;
        }
        Err(error) => {
            let _ = send_json(
                &mut socket,
                &ServerMessage::error(
                    "backend_unavailable",
                    format!("failed to initialize Remote backend runtime: {error}"),
                    true,
                    None,
                ),
            )
            .await;
            return;
        }
    };
    if let Err(error) = runtime.ensure_session(hello.4.as_deref()).await {
        let _ = send_json(
            &mut socket,
            &ServerMessage::error("session_unavailable", error.to_string(), true, None),
        )
        .await;
        return;
    }

    let mut event_rx = runtime.subscribe();
    let plan = runtime.resume_plan(hello.5, hello.6);
    if send_json(
        &mut socket,
        &ServerMessage::Welcome {
            version: REMOTE_PROTOCOL_VERSION,
            client_id: client_id.clone(),
            workspace: workspace_display,
            session_id: plan.session_id.clone(),
            sequence: plan.sequence,
            revision: plan.revision,
            resumed: plan.resumed,
        },
    )
    .await
    .is_err()
    {
        return;
    }
    match runtime.ui_messages() {
        Ok(messages) => {
            for message in messages {
                if send_json(&mut socket, &message).await.is_err() {
                    return;
                }
            }
        }
        Err(error) => {
            let _ = send_json(
                &mut socket,
                &ServerMessage::error("ui_unavailable", error.to_string(), true, None),
            )
            .await;
            return;
        }
    }
    if let Some(state) = plan.snapshot {
        if send_json(
            &mut socket,
            &ServerMessage::Snapshot {
                version: REMOTE_PROTOCOL_VERSION,
                sequence: plan.sequence,
                revision: plan.revision,
                state,
            },
        )
        .await
        .is_err()
        {
            return;
        }
    } else {
        for message in plan.replay {
            if send_json(&mut socket, &message).await.is_err() {
                return;
            }
        }
    }

    loop {
        tokio::select! {
            inbound = socket.recv() => {
                let Some(inbound) = inbound else { break; };
                let Ok(inbound) = inbound else { break; };
                match inbound {
                    Message::Text(text) => {
                        let message = match decode_client_message(&text) {
                            Ok(message) => message,
                            Err(error) => {
                                let _ = send_json(&mut socket, &ServerMessage::error(
                                    "malformed_message", error.to_string(), false, None,
                                )).await;
                                continue;
                            }
                        };
                        if let Some(version) = message.exact_version()
                            && validate_exact_version(version).is_err()
                        {
                            let _ = send_json(&mut socket, &ServerMessage::error(
                                "protocol_mismatch",
                                format!("unsupported Remote protocol version {version}"),
                                true,
                                None,
                            )).await;
                            break;
                        }
                        match message {
                            ClientMessage::Hello { .. } => {
                                let _ = send_json(&mut socket, &ServerMessage::error(
                                    "already_initialized", "hello may only be sent once per WebSocket", false, None,
                                )).await;
                            }
                            ClientMessage::Ping { nonce, .. } => {
                                if send_json(&mut socket, &ServerMessage::Pong {
                                    version: REMOTE_PROTOCOL_VERSION,
                                    nonce,
                                }).await.is_err() {
                                    break;
                                }
                            }
                            ClientMessage::UiAction { action, request_id, .. } => {
                                if let Err(error) = runtime.apply_ui(action, request_id.clone()).await {
                                    let _ = send_json(&mut socket, &ServerMessage::error(
                                        "ui_unavailable", error.to_string(), false, request_id,
                                    )).await;
                                }
                            }
                            ClientMessage::UiAgentAction { action, request_id, .. } => {
                                if let Err(error) = runtime.apply_agents(action, request_id.clone()).await {
                                    let _ = send_json(&mut socket, &ServerMessage::error(
                                        "ui_action_failed", error.to_string(), false, request_id,
                                    )).await;
                                }
                            }
                            ClientMessage::UiConversationAction { action, request_id, .. } => {
                                if let Err(error) = runtime.apply_conversation(action, request_id.clone()).await {
                                    let _ = send_json(&mut socket, &ServerMessage::error(
                                        "ui_action_failed", error.to_string(), false, request_id,
                                    )).await;
                                }
                            }
                            ClientMessage::UiComposerAction { action, request_id, .. } => {
                                if let Err(error) = runtime.apply_composer(action, request_id.clone()).await {
                                    let _ = send_json(&mut socket, &ServerMessage::error(
                                        "ui_action_failed", error.to_string(), false, request_id,
                                    )).await;
                                }
                            }
                            ClientMessage::UiToolbarAction { action, request_id, .. } => {
                                runtime.touch();
                                if let Err(error) = super::super::ui_host::enqueue_toolbar(&runtime.commands,&runtime.wake,action,request_id.clone(),BACKEND_COMMAND_DELIVERY_TIMEOUT).await {
                                    let _ = send_json(&mut socket, &ServerMessage::error(
                                        "ui_action_failed", error.to_string(), false, request_id,
                                    )).await;
                                }
                            }
                            ClientMessage::UiSettingsAction { action, request_id, .. } => {
                                if let Err(error) = runtime.apply_settings(action, request_id.clone()).await {
                                    let _ = send_json(&mut socket, &ServerMessage::error(
                                        "ui_action_failed", error.to_string(), false, request_id,
                                    )).await;
                                }
                            }
                            ClientMessage::UiHomeAction { action, request_id, .. } => {
                                if let Err(error) = runtime.apply_home(action, request_id.clone()).await {
                                    let _ = send_json(&mut socket, &ServerMessage::error(
                                        "ui_action_failed", error.to_string(), false, request_id,
                                    )).await;
                                }
                            }
                            ClientMessage::UiDiffAction { action, request_id, .. } => {
                                if let Err(error) = runtime.apply_diff(action, request_id.clone()).await {
                                    let _ = send_json(&mut socket, &ServerMessage::error(
                                        "ui_action_failed", error.to_string(), false, request_id,
                                    )).await;
                                }
                            }
                            ClientMessage::WorkspaceFilesRequest { request_id, path, selected, .. } => {
                                let root = workspace.clone();
                                let result = tokio::task::spawn_blocking(move || {
                                    super::super::workspace_views::files_view(&root, &path, selected.as_deref())
                                }).await;
                                let reply = match result {
                                    Ok(Ok(view)) => ServerMessage::WorkspaceFiles {
                                        version: REMOTE_PROTOCOL_VERSION, request_id, view,
                                    },
                                    Ok(Err(error)) => ServerMessage::error(
                                        "workspace_view_failed", error.to_string(), false, request_id,
                                    ),
                                    Err(error) => ServerMessage::error(
                                        "workspace_view_failed", error.to_string(), false, request_id,
                                    ),
                                };
                                if send_json(&mut socket, &reply).await.is_err() {
                                    break;
                                }
                            }
                            ClientMessage::WorkspaceChangesRequest { request_id, file, full, .. } => {
                                let root = workspace.clone();
                                let result = tokio::task::spawn_blocking(move || {
                                    super::super::workspace_views::changes_view(&root, file.as_deref(), full)
                                }).await;
                                let reply = match result {
                                    Ok(Ok(view)) => ServerMessage::WorkspaceChanges {
                                        version: REMOTE_PROTOCOL_VERSION, request_id, view,
                                    },
                                    Ok(Err(error)) => ServerMessage::error(
                                        "workspace_view_failed", error.to_string(), false, request_id,
                                    ),
                                    Err(error) => ServerMessage::error(
                                        "workspace_view_failed", error.to_string(), false, request_id,
                                    ),
                                };
                                if send_json(&mut socket, &reply).await.is_err() {
                                    break;
                                }
                            }
                            ClientMessage::Command { request_id, mut command, .. } => {
                                if !command_allowed(&command) {
                                    let _ = send_json(&mut socket, &ServerMessage::error(
                                        "command_forbidden",
                                        "shutdown is not exposed through the Remote frontend protocol",
                                        false,
                                        request_id,
                                    )).await;
                                    continue;
                                }
                                if let Err(error) =
                                    resolve_remote_attachments(&hub, &workspace, &mut command)
                                {
                                    let _ = send_json(&mut socket, &ServerMessage::error(
                                        "attachment_invalid", error.to_string(), false, request_id,
                                    )).await;
                                    continue;
                                }
                                match runtime.send_command(command).await {
                                    Ok(()) => {
                                        if send_json(&mut socket, &ServerMessage::Ack {
                                            version: REMOTE_PROTOCOL_VERSION,
                                            request_id,
                                        }).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(error) => {
                                        let _ = send_json(&mut socket, &ServerMessage::error(
                                            "backend_unavailable", error.to_string(), false, request_id,
                                        )).await;
                                    }
                                }
                            }
                        }
                    }
                    Message::Ping(payload) => {
                        if send_ws(&mut socket, Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Message::Pong(_) => {}
                    Message::Close(_) => break,
                    Message::Binary(_) => {
                        let _ = send_json(&mut socket, &ServerMessage::error(
                            "malformed_message", "Remote protocol messages must be UTF-8 JSON text", false, None,
                        )).await;
                    }
                }
            }
            event = event_rx.recv() => {
                match event {
                    Ok(message) => {
                        if send_json(&mut socket, &message).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        match runtime.ui_messages() {
                            Ok(messages) => for message in messages {
                                if send_json(&mut socket, &message).await.is_err() { return; }
                            },
                            Err(_) => return,
                        }
                        let plan = runtime.resume_plan(None, None);
                        if let Some(state) = plan.snapshot
                            && send_json(&mut socket, &ServerMessage::Snapshot {
                                version: REMOTE_PROTOCOL_VERSION,
                                sequence: plan.sequence,
                                revision: plan.revision,
                                state,
                            }).await.is_err()
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    runtime.touch();
}
