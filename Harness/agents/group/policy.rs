//! User-configurable Agent Group behavior policy.

use crate::model::AgentGroupSettings;

use super::WritePolicy;

#[derive(Debug, Clone)]
pub(crate) struct AgentGroupPolicy {
    pub write_policy: WritePolicy,
    pub auto_deploy: bool,
}

impl Default for AgentGroupPolicy {
    fn default() -> Self {
        Self::from(&AgentGroupSettings::default())
    }
}

impl From<&AgentGroupSettings> for AgentGroupPolicy {
    fn from(settings: &AgentGroupSettings) -> Self {
        Self {
            write_policy: if settings.write_policy == "primary_only" {
                WritePolicy::PrimaryOnly
            } else {
                WritePolicy::SingleWriter
            },
            auto_deploy: settings.auto_deploy,
        }
    }
}

impl AgentGroupPolicy {
    /// Main-Agent group guidance, or `None` when delegation is off.
    pub(crate) fn group_guidance(&self) -> Option<String> {
        self.auto_deploy.then(|| {
            "Delegate independent or parallelizable work to Group Agents when doing so improves the result.".into()
        })
    }
}
