//! Runtime activation and deactivation for Skills, MCP servers, and workers.

use super::*;

impl ToolRegistry {
    pub(super) fn activate(&mut self, id: &str) -> Result<String> {
        if self.catalog.disabled_capabilities.contains(id) {
            bail!("Capability {id} is disabled for this session");
        }
        if self.catalog.descriptors.is_empty() {
            self.refresh_capabilities();
        }
        if let Some(name) = id.strip_prefix("skill:") {
            return self.activate_skill(name, false);
        }
        if let Some(server) = id.strip_prefix("mcp:") {
            if self.catalog.active_mcp.contains(server) {
                return Ok(json!({"activated":id,"alreadyActive":true}).to_string());
            }
            let mut tools = self.bridge_client()?.list_mcp_tools(Some(server))?;
            // Provider-facing MCP names must not depend on tools/list order.
            // Sort by logical identity before allocating names so an unrelated
            // upstream reorder cannot churn the schema/order cache surface.
            tools.sort_by(|left, right| left.name.cmp(&right.name));
            let mut names = Vec::new();
            for tool in tools {
                if matches!(tool.name.as_str(), "web_search" | "web_read") {
                    continue;
                }
                let stable_identity = format!("mcp:{server}:{}", tool.name);
                let safe = allocate_stable_tool_name(
                    "mcp",
                    &[server, &tool.name],
                    &stable_identity,
                    self.catalog.active_tools.keys(),
                );
                let definition = ToolDefinition {
                    name: safe.clone(),
                    description: tool.description,
                    input_schema: tool.input_schema,
                };
                if tool
                    .annotations
                    .as_ref()
                    .and_then(|a| a.get("readOnlyHint"))
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    self.catalog.read_only_mcp_tools.insert(safe.clone());
                } else {
                    self.catalog.read_only_mcp_tools.remove(&safe);
                }
                self.catalog
                    .mcp_tool_map
                    .insert(safe.clone(), (server.to_owned(), tool.name));
                self.catalog.active_tools.insert(safe.clone(), definition);
                names.push(safe);
            }
            self.catalog.active_mcp.insert(server.to_owned());
            return Ok(json!({"activated":id,"tools":names}).to_string());
        }
        if let Some(worker) = id.strip_prefix("worker:") {
            if self.catalog.active_workers.contains(worker) {
                return Ok(json!({"activated":id,"alreadyActive":true}).to_string());
            }
            let mut definitions = self.workers.activate(worker)?;
            definitions.sort_by(|left, right| left.name.cmp(&right.name));
            let mut names = Vec::new();
            for definition in definitions {
                let stable_identity = format!("worker:{worker}:{}", definition.name);
                let safe = allocate_stable_tool_name(
                    "worker",
                    &[worker, &definition.name],
                    &stable_identity,
                    self.catalog.active_tools.keys(),
                );
                self.catalog
                    .worker_tool_map
                    .insert(safe.clone(), (worker.to_owned(), definition.name.clone()));
                self.catalog.active_tools.insert(
                    safe.clone(),
                    ToolDefinition {
                        name: safe.clone(),
                        ..definition
                    },
                );
                names.push(safe);
            }
            self.catalog.active_workers.insert(worker.to_owned());
            return Ok(json!({"activated":id,"tools":names}).to_string());
        }
        bail!("Unknown capability: {id}")
    }

    pub(super) fn activate_skill(&mut self, name: &str, explicit: bool) -> Result<String> {
        let id = format!("skill:{name}");
        if self.catalog.active_skills.contains(name) {
            let mut tools = self
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
            tools.sort();
            if explicit {
                let skill = self.bridge_client()?.load_skill(name)?;
                return Ok(json!({
                    "activated":id,
                    "alreadyActive":true,
                    "instructions":skill.instructions,
                    "tools":tools
                })
                .to_string());
            }
            return Ok(json!({"activated":id,"alreadyActive":true,"tools":tools}).to_string());
        }
        let skill = self.bridge_client()?.load_skill(name)?;
        if !explicit && skill.allow_implicit_invocation == Some(false) {
            bail!("Skill {name} requires explicit user invocation with ${name}");
        }
        let read_identity = format!("skill:{name}:read_file");
        let tool_name = allocate_stable_tool_name(
            "skill",
            &[name, "read_file"],
            &read_identity,
            self.catalog.active_tools.keys(),
        );
        let definition = ToolDefinition::new(
            &tool_name,
            format!("Read a supporting file from Skill {name}."),
            json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}),
        );
        self.catalog
            .active_tools
            .insert(tool_name.clone(), definition);
        self.catalog
            .skill_tool_map
            .insert(tool_name.clone(), name.to_owned());
        let script_tool_name = if skill.files.iter().any(|path| path.starts_with("scripts/")) {
            let script_identity = format!("skill:{name}:run_script");
            let script_tool_name = allocate_stable_tool_name(
                "skill",
                &[name, "run_script"],
                &script_identity,
                self.catalog.active_tools.keys(),
            );
            let script_definition = ToolDefinition::new(
                &script_tool_name,
                format!(
                    "Run a helper from Skill {name}'s scripts/ under normal sandbox/approval rules."
                ),
                json!({
                    "type":"object",
                    "properties":{
                        "path":{"type":"string","description":"Relative script path such as scripts/check.py"},
                        "args":{"type":"array","items":{"type":"string"},"maxItems":64},
                        "timeoutSeconds":{"type":"integer","minimum":1,"maximum":900}
                    },
                    "required":["path"],
                    "additionalProperties":false
                }),
            );
            self.catalog
                .active_tools
                .insert(script_tool_name.clone(), script_definition);
            self.catalog
                .skill_script_tool_map
                .insert(script_tool_name.clone(), name.to_owned());
            Some(script_tool_name)
        } else {
            None
        };
        self.catalog.active_skills.insert(name.to_owned());
        let mut tools = vec![tool_name.clone()];
        if let Some(script) = script_tool_name.as_ref() {
            tools.push(script.clone());
        }
        Ok(json!({
            "activated":id,
            "instructions":skill.instructions,
            "readTool":tool_name,
            "scriptTool":script_tool_name,
            "tools":tools
        })
        .to_string())
    }

    pub(super) fn deactivate_mcp(&mut self, server: &str) {
        let tool_names = self
            .catalog
            .mcp_tool_map
            .iter()
            .filter(|(_, (mapped_server, _))| mapped_server == server)
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        for name in tool_names {
            self.catalog.mcp_tool_map.remove(&name);
            self.catalog.active_tools.remove(&name);
            self.catalog.read_only_mcp_tools.remove(&name);
        }
        self.catalog.active_mcp.remove(server);
        self.catalog.active_mcp_identity.remove(server);
    }
}
