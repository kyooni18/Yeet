//! Session-scoped explicit skill attachment behavior.

use super::*;

impl ToolRegistry {
    pub fn activate_explicit_skill(&mut self, name: &str) -> Result<String> {
        let id = format!("skill:{name}");
        if self.catalog.disabled_capabilities.contains(&id) {
            bail!("Capability {id} is disabled for this session");
        }
        self.activate_skill(name, true)
    }

    pub fn enable_skill_attachment(&mut self, name: &str) {
        self.catalog
            .disabled_capabilities
            .remove(&format!("skill:{name}"));
    }

    pub fn deactivate_skill(&mut self, name: &str) {
        let tool_names = self
            .catalog
            .skill_tool_map
            .iter()
            .filter_map(|(tool, skill)| (skill == name).then_some(tool.clone()))
            .chain(
                self.catalog
                    .skill_script_tool_map
                    .iter()
                    .filter_map(|(tool, skill)| (skill == name).then_some(tool.clone())),
            )
            .collect::<Vec<_>>();
        for tool_name in tool_names {
            self.catalog.skill_tool_map.remove(&tool_name);
            self.catalog.skill_script_tool_map.remove(&tool_name);
            self.catalog.active_tools.remove(&tool_name);
        }
        self.catalog.active_skills.remove(name);
    }

    pub fn skill_tools_for(&self, skills: &HashSet<String>) -> Vec<String> {
        let mut tools = self
            .catalog
            .skill_tool_map
            .iter()
            .filter_map(|(tool, skill)| skills.contains(skill).then_some(tool.clone()))
            .chain(
                self.catalog
                    .skill_script_tool_map
                    .iter()
                    .filter_map(|(tool, skill)| skills.contains(skill).then_some(tool.clone())),
            )
            .collect::<Vec<_>>();
        tools.sort();
        tools.dedup();
        tools
    }
}
