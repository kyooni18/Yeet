//! Serializable application concepts for the Agent Group view.
use crate::harness::HarnessCommand;
use crate::model::{AgentGroupItem, AgentMemberItem};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum AgentAction {
    Open,
    Close,
    Select(Option<String>),
    Steer,
    Stop,
    CreateGroup,
    RunGroup,
    CancelGroup,
    StopGroup,
    Remove,
    AgentGroup,
    InspectGroup,
    SubmitDraft(String),
    CancelDraft,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentState {
    pub open: bool,
    pub selected: Option<String>,
    pub creating_group: bool,
    pub input_focused: bool,
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AgentEffect {
    pub command: Option<HarnessCommand>,
    pub open_group_settings: bool,
    /// Present only after successful command construction; adapters may clear the draft.
    pub submitted_text: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentIcon {
    Add,
    Play,
    Stop,
    Remove,
    Settings,
    Refresh,
    Message,
    Tool,
    Complete,
    Error,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    Active,
    Error,
    Waiting,
    Complete,
    Idle,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberStatus {
    pub key: String,
    pub label: String,
    pub tone: Tone,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberView {
    pub member: AgentMemberItem,
    pub status: MemberStatus,
    pub selected: bool,
    pub select_action: AgentAction,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentControl {
    pub label: String,
    pub icon: AgentIcon,
    pub enabled: bool,
    pub visible: bool,
    pub action: AgentAction,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedEntry {
    pub id: String,
    pub sequence: Option<u64>,
    pub at: String,
    pub kind: String,
    pub detail: String,
    pub member_id: Option<String>,
    pub actor: String,
    pub icon: AgentIcon,
    pub running: bool,
    pub tool: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetView {
    pub output_limit: u64,
    pub output_used: u64,
    pub output_percent: f64,
    pub cost_limit: f64,
    pub cost_used: Option<f64>,
    pub cost_percent: f64,
    pub tasks: Vec<TaskBudgetView>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSection {
    Objective,
    Result,
    Members,
    Activity,
    Budget,
    Findings,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionLayout {
    FullWidth,
    Column,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionView {
    pub kind: AgentSection,
    pub layout: SectionLayout,
    pub label: String,
    pub visible: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingView {
    pub member_id: String,
    pub task_id: String,
    pub at: String,
    pub summary: String,
    pub actor: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskBudgetView {
    pub task_id: String,
    pub label: String,
    pub used: u64,
    pub allocated: u64,
    pub remaining: u64,
    pub context_window_tokens: u64,
    pub percent: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsView {
    pub state: AgentState,
    pub group: AgentGroupItem,
    pub group_status: MemberStatus,
    pub title: String,
    pub accessible_label: String,
    pub empty_title: String,
    pub headline: String,
    pub empty_message: String,
    pub composer_hint: String,
    pub create_hint: String,
    pub create_control: AgentControl,
    pub findings: Vec<FindingView>,
    pub result: Option<String>,
    pub members: Vec<MemberView>,
    pub feed: Vec<FeedEntry>,
    pub budget: BudgetView,
    pub active_count: usize,
    pub waiting_count: usize,
    pub settings_control: AgentControl,
    pub group_controls: Vec<AgentControl>,
    pub selection_controls: Vec<AgentControl>,
    pub sections: Vec<SectionView>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentUiEffect {
    pub open_group_settings: bool,
    pub submitted_text: Option<String>,
}
