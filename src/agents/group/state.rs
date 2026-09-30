//! Mutable bookkeeping for the current delegation generation.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, atomic::AtomicBool},
};

use crate::{agents::AgentId, core::Usage};

use super::snapshot::AgentTaskSnapshot;

#[derive(Default)]
pub(super) struct AdaptiveAgentState {
    pub generation: u64,
    pub parent_agent: Option<AgentId>,
    pub total_started: usize,
    pub active: HashSet<String>,
    pub worker_cancels: HashMap<String, Arc<AtomicBool>>,
    pub tasks: Vec<AgentTaskSnapshot>,
    pub usage: Usage,
}
