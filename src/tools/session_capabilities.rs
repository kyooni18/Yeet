//! Session-scoped Skyline and explicit skill attachment behavior.

use super::*;

impl ToolRegistry {
    pub fn attach_skyline(&mut self) -> Result<String> {
        if self.skyline_handle.is_some() {
            self.active_tools
                .entry("skyline".into())
                .or_insert_with(crate::skyline::tui_tool_definition);
            return Ok(json!({
                "activated": crate::skyline::CAPABILITY_ID,
                "alreadyActive": true,
                "tools": ["skyline"]
            })
            .to_string());
        }

        let mut arguments = Map::new();
        arguments.insert(
            "capability".into(),
            Value::String(crate::skyline::CAPABILITY_ID.into()),
        );
        arguments.insert("explicitUserInvocation".into(), Value::Bool(true));
        let activation = crate::skyline::activate(&self.workspace_root, arguments)?;
        let mut parsed: Value = serde_json::from_str(&activation)?;
        let handle = parsed
            .get("handle")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Skyline activation returned no handle"))?
            .to_owned();
        self.skyline_handle = Some(handle);
        self.active_tools
            .insert("skyline".into(), crate::skyline::tui_tool_definition());
        if let Some(object) = parsed.as_object_mut() {
            object.insert("tools".into(), json!(["skyline"]));
        }
        Ok(parsed.to_string())
    }

    pub fn detach_skyline(&mut self) {
        self.skyline_handle = None;
        self.active_tools.remove("skyline");
    }

    pub fn skyline_tool_names(&self) -> Vec<String> {
        self.skyline_handle
            .as_ref()
            .map(|_| vec!["skyline".into()])
            .unwrap_or_default()
    }

    pub(super) fn execute_skyline(&self, object: &Map<String, Value>) -> Result<String> {
        let handle = self
            .skyline_handle
            .as_deref()
            .ok_or_else(|| anyhow!("Skyline is not attached to this TUI session"))?;
        let operation = string_arg(object, "operation")?;
        let mut arguments = Map::new();
        arguments.insert("handle".into(), Value::String(handle.to_owned()));
        arguments.insert("operation".into(), Value::String(operation.to_owned()));
        if let Some(value) = object.get("arguments") {
            if !value.is_object() {
                bail!("skyline.arguments must be an object");
            }
            arguments.insert("arguments".into(), value.clone());
        }
        crate::skyline::invoke(&self.workspace_root, arguments)
    }

    pub fn activate_explicit_skill(&mut self, name: &str) -> Result<String> {
        let id = format!("skill:{name}");
        if self.disabled_capabilities.contains(&id) {
            bail!("Capability {id} is disabled for this session");
        }
        self.activate_skill(name, true)
    }

    pub fn enable_skill_attachment(&mut self, name: &str) {
        self.disabled_capabilities.remove(&format!("skill:{name}"));
    }

    pub fn deactivate_skill(&mut self, name: &str) {
        let tool_names = self
            .skill_tool_map
            .iter()
            .filter_map(|(tool, skill)| (skill == name).then_some(tool.clone()))
            .chain(
                self.skill_script_tool_map
                    .iter()
                    .filter_map(|(tool, skill)| (skill == name).then_some(tool.clone())),
            )
            .collect::<Vec<_>>();
        for tool_name in tool_names {
            self.skill_tool_map.remove(&tool_name);
            self.skill_script_tool_map.remove(&tool_name);
            self.active_tools.remove(&tool_name);
        }
        self.active_skills.remove(name);
    }

    pub fn skill_tools_for(&self, skills: &HashSet<String>) -> Vec<String> {
        let mut tools = self
            .skill_tool_map
            .iter()
            .filter_map(|(tool, skill)| skills.contains(skill).then_some(tool.clone()))
            .chain(
                self.skill_script_tool_map
                    .iter()
                    .filter_map(|(tool, skill)| skills.contains(skill).then_some(tool.clone())),
            )
            .collect::<Vec<_>>();
        tools.sort();
        tools.dedup();
        tools
    }
}
