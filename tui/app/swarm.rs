//! Agent swarm panel (`Mode::Swarm`): turns the session's swarm on or off and
//! edits the persisted swarm limits (parallel agents, pool size, per-turn
//! budgets, writers, auto-deploy).
use super::{App, Mode};
use crate::model::{AgentMode, FrontendCommand, SwarmSettings};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwarmRow {
    Enabled,
    AutoDeploy,
    Parallel,
    Pool,
    Tokens,
    Cost,
    Writers,
    OpenAgents,
}

pub const SWARM_ROWS: [SwarmRow; 8] = [
    SwarmRow::Enabled,
    SwarmRow::AutoDeploy,
    SwarmRow::Parallel,
    SwarmRow::Pool,
    SwarmRow::Tokens,
    SwarmRow::Cost,
    SwarmRow::Writers,
    SwarmRow::OpenAgents,
];

const TOKEN_STEPS: [u64; 8] = [
    50_000, 100_000, 200_000, 500_000, 1_000_000, 2_000_000, 5_000_000, 10_000_000,
];
const COST_CENT_STEPS: [u64; 9] = [25, 50, 100, 200, 500, 1_000, 2_000, 5_000, 10_000];

/// The next preset after `current` in `forward` direction, clamped at the ends.
fn step(steps: &[u64], current: u64, forward: bool) -> u64 {
    if forward {
        steps.iter().copied().find(|value| *value > current)
    } else {
        steps.iter().rev().copied().find(|value| *value < current)
    }
    .unwrap_or(current)
}

impl App {
    pub(crate) fn swarm_enabled(&self) -> bool {
        self.state.agent_mode == AgentMode::Adaptive
    }

    /// Opens the panel; `Esc` returns to the mode it was opened from.
    pub(crate) fn open_swarm(&mut self) -> FrontendCommand {
        self.agents.swarm_origin = Some(self.mode);
        self.mode = Mode::Swarm;
        self.popup_index = 0;
        self.input_focused = false;
        FrontendCommand::RequestSettings
    }

    fn close_swarm(&mut self) {
        match self.agents.swarm_origin.take() {
            Some(Mode::Agents) => self.open_agents(),
            Some(Mode::Settings) => {
                self.mode = Mode::Settings;
                self.popup_index = self.swarm_settings_row_index();
            }
            _ => self.close_popup(),
        }
    }

    pub(crate) fn handle_swarm_key(&mut self, event: KeyEvent) -> Vec<FrontendCommand> {
        if event.modifiers.contains(KeyModifiers::CONTROL) {
            return Vec::new();
        }
        let last = SWARM_ROWS.len() - 1;
        match event.code {
            KeyCode::Esc | KeyCode::Char('q') => self.close_swarm(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.popup_index = self.popup_index.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.popup_index = (self.popup_index + 1).min(last)
            }
            KeyCode::Left | KeyCode::Char('h' | '-') => return self.adjust_swarm(false),
            KeyCode::Right | KeyCode::Char('l' | '+' | '=') => return self.adjust_swarm(true),
            KeyCode::Enter | KeyCode::Char(' ') => {
                if SWARM_ROWS[self.popup_index.min(last)] == SwarmRow::OpenAgents {
                    self.agents.swarm_origin = None;
                    self.open_agents();
                    return Vec::new();
                }
                return self.adjust_swarm(true);
            }
            _ => {}
        }
        Vec::new()
    }

    /// Changes the selected row. Settings are applied locally right away so
    /// repeated presses build on each other before the backend echoes them.
    fn adjust_swarm(&mut self, forward: bool) -> Vec<FrontendCommand> {
        let row = SWARM_ROWS[self.popup_index.min(SWARM_ROWS.len() - 1)];
        let mut settings = self.state.runtime_settings.swarm.clone();
        let mut commands = Vec::new();
        match row {
            SwarmRow::Enabled => {
                if self.state.is_streaming {
                    self.state.settings_notice =
                        Some("The swarm can be switched after the current response.".into());
                } else {
                    let mode = if self.swarm_enabled() {
                        AgentMode::Single
                    } else {
                        AgentMode::Adaptive
                    };
                    commands.push(FrontendCommand::SetAgentMode { mode });
                }
                return commands;
            }
            SwarmRow::AutoDeploy => {
                settings.auto_deploy = !settings.auto_deploy;
                // Auto-deploy is pointless without a swarm in this session.
                if settings.auto_deploy && !self.swarm_enabled() && !self.state.is_streaming {
                    commands.push(FrontendCommand::SetAgentMode {
                        mode: AgentMode::Adaptive,
                    });
                }
            }
            SwarmRow::Parallel => {
                settings.max_concurrent = if forward {
                    settings.max_concurrent.saturating_add(1)
                } else {
                    settings.max_concurrent.saturating_sub(1)
                };
            }
            SwarmRow::Pool => {
                let floor = settings.max_concurrent;
                settings.max_members = if forward {
                    settings.max_members.saturating_add(1)
                } else {
                    settings.max_members.saturating_sub(1).max(floor)
                };
            }
            SwarmRow::Tokens => {
                settings.max_tokens = step(&TOKEN_STEPS, settings.max_tokens, forward)
            }
            SwarmRow::Cost => {
                settings.max_cost_cents = step(&COST_CENT_STEPS, settings.max_cost_cents, forward)
            }
            SwarmRow::Writers => {
                settings.write_policy = if settings.write_policy == "primary_only" {
                    "single_writer".into()
                } else {
                    "primary_only".into()
                };
            }
            SwarmRow::OpenAgents => return commands,
        }
        let settings = settings.normalized();
        if settings == self.state.runtime_settings.swarm {
            return commands;
        }
        self.state.runtime_settings.swarm = settings.clone();
        commands.insert(0, FrontendCommand::SetSwarmSettings { settings });
        commands
    }

    /// One-line summary for the Agents rail and Settings: "on · 4× · auto".
    pub(crate) fn swarm_summary(&self) -> String {
        let swarm: &SwarmSettings = &self.state.runtime_settings.swarm;
        let mut summary = format!(
            "{} · {}×",
            if self.swarm_enabled() { "on" } else { "off" },
            swarm.max_concurrent
        );
        if swarm.auto_deploy {
            summary.push_str(" · auto");
        }
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn swarm_panel_edits_limits_and_auto_deploy_turns_the_swarm_on() {
        let mut app = App::default();
        app.open_agents();
        assert!(matches!(
            app.handle_agents_key(key(KeyCode::Char('w'))),
            Some(FrontendCommand::RequestSettings)
        ));
        assert_eq!(app.mode, Mode::Swarm);

        // Auto-deploy also enables the swarm for this session.
        app.popup_index = 1;
        let sent = app.handle_swarm_key(key(KeyCode::Enter));
        assert!(matches!(
            sent.as_slice(),
            [
                FrontendCommand::SetSwarmSettings { settings },
                FrontendCommand::SetAgentMode { mode: AgentMode::Adaptive },
            ] if settings.auto_deploy
        ));

        // Raising parallel agents past the pool grows the pool with it.
        app.popup_index = 2;
        for _ in 0..6 {
            app.handle_swarm_key(key(KeyCode::Right));
        }
        let swarm = &app.state.runtime_settings.swarm;
        assert_eq!((swarm.max_concurrent, swarm.max_members), (10, 10));
        app.popup_index = 3;
        app.handle_swarm_key(key(KeyCode::Left));
        assert_eq!(app.state.runtime_settings.swarm.max_members, 10);

        app.popup_index = 4;
        app.handle_swarm_key(key(KeyCode::Right));
        assert_eq!(app.state.runtime_settings.swarm.max_tokens, 500_000);

        app.handle_swarm_key(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Agents);
    }
}
