//! Agent UI transitions and runtime command effects.
use super::*;
use crate::harness::{HarnessCommand, HarnessState};
use crate::model::{AgentGroupItem, AgentMemberItem};
impl AgentState {
    pub fn selected_member<'a>(&self, group: &'a AgentGroupItem) -> Option<&'a AgentMemberItem> {
        let id = self.selected.as_deref()?;
        group.members.iter().find(|member| member.id == id)
    }
    pub fn reconcile(&mut self, group: &AgentGroupItem) {
        if self.selected.is_some() && self.selected_member(group).is_none() {
            self.selected = None;
        }
    }
    pub fn move_selection(&mut self, group: &AgentGroupItem, delta: isize) {
        let current = self
            .selected_member(group)
            .and_then(|member| group.members.iter().position(|item| item.id == member.id))
            .map_or(0, |index| index + 1);
        let next = current
            .saturating_add_signed(delta)
            .min(group.members.len());
        self.selected = next
            .checked_sub(1)
            .map(|index| group.members[index].id.clone());
    }
    pub fn apply(&mut self, action: AgentAction, state: &HarnessState) -> AgentEffect {
        let group = &state.agent_group;
        self.reconcile(group);
        let mut effect = AgentEffect::default();
        effect.command = match action {
            AgentAction::Open => {
                self.open = true;
                self.input_focused = false;
                None
            }
            AgentAction::Close => {
                self.open = false;
                self.selected = None;
                None
            }
            AgentAction::Select(id) => {
                self.selected = id;
                self.reconcile(group);
                self.input_focused = false;
                None
            }
            AgentAction::Steer => {
                self.input_focused = true;
                None
            }
            AgentAction::CreateGroup => {
                if !matches!(group.status.as_str(), "running" | "paused") {
                    self.creating_group = true;
                    self.selected = None;
                    self.input_focused = true;
                }
                None
            }
            AgentAction::CancelDraft => {
                self.creating_group = false;
                None
            }
            AgentAction::RunGroup if group.status == "paused" => {
                Some(HarnessCommand::ResumeAgentGroup {
                    group_id: group.group_id.clone(),
                })
            }
            AgentAction::RunGroup
                if group.objective.is_some()
                    && matches!(group.status.as_str(), "created" | "completed" | "failed") =>
            {
                Some(HarnessCommand::StartAgentGroup {
                    group_id: group.group_id.clone(),
                })
            }
            AgentAction::CancelGroup if group.status == "running" => {
                Some(HarnessCommand::CancelAgentGroup {
                    group_id: group.group_id.clone(),
                })
            }
            AgentAction::StopGroup if matches!(group.status.as_str(), "running" | "paused") => {
                Some(HarnessCommand::StopAgentGroup {
                    group_id: group.group_id.clone(),
                })
            }
            AgentAction::InspectGroup if !group.group_id.is_empty() => {
                Some(HarnessCommand::InspectAgentGroup {
                    group_id: group.group_id.clone(),
                })
            }
            AgentAction::Stop => match self.selected_member(group) {
                Some(member) if member.status != "stopped" => Some(HarnessCommand::StopAgent {
                    agent_id: Some(member.id.clone()),
                }),
                Some(_) => None,
                None if group.status == "running" => Some(HarnessCommand::CancelAgentGroup {
                    group_id: group.group_id.clone(),
                }),
                None => None,
            },
            AgentAction::Remove => match self.selected_member(group) {
                Some(member) => {
                    let id = member.id.clone();
                    self.selected = None;
                    Some(HarnessCommand::RemoveAgent { agent_id: Some(id) })
                }
                None if group
                    .members
                    .iter()
                    .any(|member| member.status == "stopped") =>
                {
                    Some(HarnessCommand::RemoveAgent { agent_id: None })
                }
                None => None,
            },
            AgentAction::AgentGroup => {
                effect.open_group_settings = true;
                None
            }
            AgentAction::SubmitDraft(text) => {
                let text = text.trim().to_owned();
                if text.is_empty() {
                    return effect;
                }
                let command = if self.creating_group {
                    if matches!(group.status.as_str(), "running" | "paused") {
                        return effect;
                    }
                    self.creating_group = false;
                    HarnessCommand::CreateAgentGroup {
                        objective: text.clone(),
                    }
                } else {
                    match self.selected_member(group) {
                        Some(member) if member.status == "stopped" => return effect,
                        Some(member) => HarnessCommand::MessageAgent {
                            agent_id: member.id.clone(),
                            message: text.clone(),
                        },
                        None if state.is_streaming => return effect,
                        None => HarnessCommand::Submit {
                            text: text.clone(),
                            images: Vec::new(),
                            attachment_ids: Vec::new(),
                        },
                    }
                };
                effect.submitted_text = Some(text);
                Some(command)
            }
            _ => None,
        };
        effect
    }
}

impl From<&AgentEffect> for AgentUiEffect {
    fn from(effect: &AgentEffect) -> Self {
        Self {
            open_group_settings: effect.open_group_settings,
            submitted_text: effect.submitted_text.clone(),
        }
    }
}
