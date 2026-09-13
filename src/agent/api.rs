//! Public request, outcome, and event types for the root agent coordinator.

use std::sync::{Arc, atomic::AtomicBool};

use serde_json::Value;

use crate::core::{ImageAttachment, ToolCall, Usage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRunOutcome {
    Completed,
    CompletedUnverified { reason: String },
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    ModelAttemptStarted {
        diagnostics: Value,
    },
    ModelAttemptFinished(Value, Option<Usage>),
    Start,
    ReasoningDelta(String),
    ReasoningSummaryDelta(String),
    TextDelta(String),
    DiscardAssistantText(String),
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: Option<String>,
    },
    ToolCall {
        index: usize,
        call: ToolCall,
    },
    ToolExecutionStarted(ToolCall),
    ToolExecutionFinished {
        call: ToolCall,
        succeeded: bool,
        result: String,
    },
    ToolExecutionSuppressed {
        call: ToolCall,
        reason: String,
    },
    AuxiliaryUsage(Usage),
    InfinityCheckpoint {
        epoch: u64,
        reason: String,
    },
    InfinityRetry {
        attempt: u32,
        delay_ms: u64,
        error: String,
    },
    Finished {
        reason: String,
        usage: Option<Usage>,
    },
}

pub struct AgentRunRequest<'a> {
    pub input: &'a str,
    pub images: Vec<ImageAttachment>,
    pub model: &'a str,
    pub reasoning_level: &'a str,
    pub attached_capabilities: Option<Vec<String>>,
    pub disabled_capabilities: Vec<String>,
    pub infinity_mode: Arc<AtomicBool>,
    pub cancel: Arc<AtomicBool>,
    pub continuation: bool,
}

pub(super) struct AgentTurnRequest<'a> {
    pub input: &'a str,
    pub images: Vec<ImageAttachment>,
    pub model: &'a str,
    pub reasoning_level: &'a str,
    pub attached_capabilities: Option<Vec<String>>,
    pub cancel: &'a AtomicBool,
    pub continuation: bool,
}
