use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolLoadingPolicy {
    Eager,
    Deferred,
    AlwaysLoad,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub server_name: String,
    pub tool_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolStub {
    pub server_name: String,
    pub tool_name: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub input_schema: Value,
    pub input_json_schema: Option<Value>,
    pub output_schema: Option<Value>,
    pub enabled: bool,
    pub loading_policy: ToolLoadingPolicy,
    pub mcp_info: Option<McpToolInfo>,
    pub mcp_stub: Option<McpToolStub>,
    pub estimated_token_cost: usize,
}

impl ToolDefinition {
    pub fn eager(name: impl Into<String>, input_schema: Value) -> Self {
        Self {
            name: name.into(),
            input_schema,
            input_json_schema: None,
            output_schema: None,
            enabled: true,
            loading_policy: ToolLoadingPolicy::Eager,
            mcp_info: None,
            mcp_stub: None,
            estimated_token_cost: 0,
        }
    }

    pub fn deferred_mcp_stub(
        name: impl Into<String>,
        input_schema: Value,
        mcp_info: McpToolInfo,
        reason: impl Into<String>,
        estimated_token_cost: usize,
    ) -> Self {
        let mcp_stub = McpToolStub {
            server_name: mcp_info.server_name.clone(),
            tool_name: mcp_info.tool_name.clone(),
            reason: reason.into(),
        };
        Self {
            name: name.into(),
            input_schema,
            input_json_schema: None,
            output_schema: None,
            enabled: true,
            loading_policy: ToolLoadingPolicy::Deferred,
            mcp_info: Some(mcp_info),
            mcp_stub: Some(mcp_stub),
            estimated_token_cost,
        }
    }

    pub fn model_input_schema(&self) -> &Value {
        self.input_json_schema
            .as_ref()
            .unwrap_or(&self.input_schema)
    }
}

#[derive(Debug, Default, Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<String, ToolDefinition>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: ToolDefinition) -> Option<ToolDefinition> {
        self.tools.insert(tool.name.clone(), tool)
    }

    pub fn get(&self, name: &str) -> Option<&ToolDefinition> {
        self.tools.get(name)
    }

    pub fn model_tool_definitions(&self) -> Vec<&ToolDefinition> {
        self.tools
            .values()
            .filter(|tool| {
                tool.enabled
                    && matches!(
                        tool.loading_policy,
                        ToolLoadingPolicy::Eager | ToolLoadingPolicy::AlwaysLoad
                    )
            })
            .collect()
    }

    pub fn model_tool_definitions_within_budget(
        &self,
        max_estimated_tokens: usize,
    ) -> Vec<&ToolDefinition> {
        let mut spent = 0;
        self.model_tool_definitions()
            .into_iter()
            .filter(|tool| {
                let next = spent + tool.estimated_token_cost;
                if next > max_estimated_tokens {
                    return false;
                }
                spent = next;
                true
            })
            .collect()
    }

    pub fn deferred_metadata(&self) -> Vec<&ToolDefinition> {
        self.tools
            .values()
            .filter(|tool| tool.enabled && tool.loading_policy == ToolLoadingPolicy::Deferred)
            .collect()
    }

    pub fn deferred_metadata_within_budget(
        &self,
        max_estimated_tokens: usize,
    ) -> Vec<&ToolDefinition> {
        let mut spent = 0;
        self.deferred_metadata()
            .into_iter()
            .filter(|tool| {
                let next = spent + tool.estimated_token_cost;
                if next > max_estimated_tokens {
                    return false;
                }
                spent = next;
                true
            })
            .collect()
    }
}
