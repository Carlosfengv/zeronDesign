use crate::{
    permission::{PermissionReason, PermissionResult},
    tools::{
        registry::{McpToolInfo, ToolLoadingPolicy},
        runtime::{ProgressSink, Tool, ToolContext, ToolError, ToolResult},
        schema::{object_schema, string_schema},
    },
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn mcp_stub_tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(FigmaGetFileStubTool)]
}

struct FigmaGetFileStubTool;

#[async_trait]
impl Tool for FigmaGetFileStubTool {
    fn name(&self) -> &'static str {
        "mcp__figma__get_file"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "fileKey": string_schema("Figma file key"),
                "nodeId": string_schema("Optional Figma node id")
            }),
            &["fileKey"],
        )
    }

    fn input_json_schema(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "fileKey": { "type": "string", "description": "Figma file key" },
                "nodeId": { "type": "string", "description": "Optional Figma node id" }
            },
            "required": ["fileKey"]
        }))
    }

    fn tool_loading(&self) -> ToolLoadingPolicy {
        ToolLoadingPolicy::Deferred
    }

    fn mcp_info(&self) -> Option<McpToolInfo> {
        Some(McpToolInfo {
            server_name: "figma".to_string(),
            tool_name: "get_file".to_string(),
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        PermissionResult::Allow {
            updated_input: input.clone(),
            reason: PermissionReason::Other {
                reason: "MCP stub allowed through normal runtime permission pipeline".to_string(),
            },
        }
    }

    async fn call(
        &self,
        _input: Value,
        _ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::Recoverable(
            "MCP adapter not configured for figma.get_file in Phase A".to_string(),
        ))
    }
}
