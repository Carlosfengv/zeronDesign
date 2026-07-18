use crate::{
    conversation::RuntimeStore,
    model_gateway::ToolCall,
    tools::runtime::{InterruptBehavior, ToolExecutor, ToolResult},
    types::AgentRunStatus,
};
use futures::future::join_all;
use serde_json::{json, Value};
use std::{collections::VecDeque, env, sync::Arc, time::Duration};
use tokio::time;

const DEFAULT_TOOL_CALL_DEADLINE_MS: u64 = 120_000;
const DEFAULT_BUILD_TOOL_CALL_DEADLINE_MS: u64 = 300_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Queued,
    Executing,
    Completed,
    Aborted,
}

#[derive(Debug, Clone)]
pub struct TrackedTool {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub status: ToolStatus,
    pub is_concurrency_safe: bool,
    pub interrupt_behavior: InterruptBehavior,
}

impl TrackedTool {
    fn new(
        call: ToolCall,
        is_concurrency_safe: bool,
        interrupt_behavior: InterruptBehavior,
    ) -> Self {
        Self {
            id: call.id,
            name: call.name,
            input: call.input,
            status: ToolStatus::Queued,
            is_concurrency_safe,
            interrupt_behavior,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StreamingToolResult {
    pub tool_use_id: String,
    pub tool_name: String,
    pub result: ToolResult,
    pub synthetic: bool,
}

impl StreamingToolResult {
    fn synthetic_error(
        tool_use_id: impl Into<String>,
        tool_name: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            tool_name: tool_name.into(),
            result: ToolResult::error(message),
            synthetic: true,
        }
    }

    fn synthetic_unrecoverable_error(
        tool_use_id: impl Into<String>,
        tool_name: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            tool_name: tool_name.into(),
            result: ToolResult::error_with_recoverable(message, false),
            synthetic: true,
        }
    }
}

#[derive(Clone)]
pub struct StreamingToolExecutor {
    tool_executor: ToolExecutor,
    tool_call_deadline: Duration,
    build_tool_call_deadline: Duration,
}

impl StreamingToolExecutor {
    pub fn new(tool_executor: ToolExecutor) -> Self {
        let deadline_ms = env::var("RUNTIME_TOOL_CALL_DEADLINE_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_TOOL_CALL_DEADLINE_MS);
        let build_deadline_ms = env::var("RUNTIME_BUILD_TOOL_CALL_DEADLINE_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_BUILD_TOOL_CALL_DEADLINE_MS);
        Self {
            tool_executor,
            tool_call_deadline: Duration::from_millis(deadline_ms),
            build_tool_call_deadline: Duration::from_millis(build_deadline_ms),
        }
    }

    pub fn with_tool_call_deadline(mut self, deadline: Duration) -> Self {
        let deadline = deadline.max(Duration::from_millis(1));
        self.tool_call_deadline = deadline;
        self.build_tool_call_deadline = deadline;
        self
    }

    pub fn with_build_tool_call_deadline(mut self, deadline: Duration) -> Self {
        self.build_tool_call_deadline = deadline.max(Duration::from_millis(1));
        self
    }

    fn deadline_for_tool(&self, tool_name: &str) -> Duration {
        if matches!(
            tool_name,
            "preview.publish" | "project.build" | "project.ensure_dependencies" | "package.install"
        ) {
            self.build_tool_call_deadline
        } else {
            self.tool_call_deadline
        }
    }

    pub fn track_calls(&self, calls: Vec<ToolCall>) -> Vec<TrackedTool> {
        calls
            .into_iter()
            .map(|call| {
                let is_concurrency_safe = self
                    .tool_executor
                    .is_concurrency_safe(&call.name, &call.input);
                let interrupt_behavior = self.tool_executor.interrupt_behavior(&call.name);
                TrackedTool::new(call, is_concurrency_safe, interrupt_behavior)
            })
            .collect()
    }

    pub async fn execute_calls(
        &self,
        store: RuntimeStore,
        run_id: &str,
        calls: Vec<ToolCall>,
    ) -> Vec<StreamingToolResult> {
        let mut queue = VecDeque::from(self.track_calls(calls));
        let mut results = Vec::new();
        let mut saw_continue_interrupt = false;

        while let Some(first) = queue.pop_front() {
            if run_is_cancelled(&store, run_id).await {
                results.push(Self::cancelled_result(first));
                while let Some(sibling) = queue.pop_front() {
                    results.push(Self::cancelled_result(sibling));
                }
                break;
            }
            if store.continue_interrupt_requested(run_id).await {
                saw_continue_interrupt = true;
                if first.interrupt_behavior == InterruptBehavior::Cancel {
                    results.push(Self::interrupted_result(first));
                    continue;
                }
            }

            let mut wave = vec![first];
            if wave[0].is_concurrency_safe {
                while queue.front().is_some_and(|tool| {
                    tool.is_concurrency_safe
                        && !(saw_continue_interrupt
                            && tool.interrupt_behavior == InterruptBehavior::Cancel)
                }) {
                    if let Some(tool) = queue.pop_front() {
                        wave.push(tool);
                    }
                }
            }

            let wave_results = self.execute_wave(store.clone(), run_id, wave).await;
            let shell_failed = wave_results
                .iter()
                .any(|result| result.tool_name == "shell.run" && result.result.is_error);
            results.extend(wave_results);

            if shell_failed {
                while let Some(mut sibling) = queue.pop_front() {
                    sibling.status = ToolStatus::Aborted;
                    results.push(StreamingToolResult::synthetic_error(
                        sibling.id,
                        sibling.name,
                        "Tool cancelled because shell.run failed",
                    ));
                }
                break;
            }

            if run_is_cancelled(&store, run_id).await {
                while let Some(sibling) = queue.pop_front() {
                    results.push(Self::cancelled_result(sibling));
                }
                break;
            }

            if store.continue_interrupt_requested(run_id).await {
                saw_continue_interrupt = true;
            }
        }

        if saw_continue_interrupt {
            store.clear_continue_interrupt(run_id).await;
        }
        results
    }

    fn cancelled_result(mut tool: TrackedTool) -> StreamingToolResult {
        tool.status = ToolStatus::Aborted;
        StreamingToolResult::synthetic_unrecoverable_error(
            tool.id,
            tool.name,
            "Tool interrupted because the run was cancelled",
        )
    }

    fn interrupted_result(mut tool: TrackedTool) -> StreamingToolResult {
        tool.status = ToolStatus::Aborted;
        StreamingToolResult::synthetic_unrecoverable_error(
            tool.id,
            tool.name,
            "Tool interrupted because a new user message was queued",
        )
    }

    async fn execute_wave(
        &self,
        store: RuntimeStore,
        run_id: &str,
        wave: Vec<TrackedTool>,
    ) -> Vec<StreamingToolResult> {
        let run_id = Arc::new(run_id.to_string());
        join_all(wave.into_iter().map(|tool| {
            let executor = self.tool_executor.clone();
            let store = store.clone();
            let run_id = run_id.clone();
            let deadline = self.deadline_for_tool(&tool.name);
            async move {
                if !executor.has_tool(&tool.name) {
                    return StreamingToolResult::synthetic_error(
                        tool.id,
                        tool.name.clone(),
                        format!("No such tool available: {}", tool.name),
                    );
                }
                match time::timeout(
                    deadline,
                    executor.execute(store, &run_id, &tool.id, &tool.name, tool.input.clone()),
                )
                .await
                {
                    Ok(execution) => StreamingToolResult {
                        tool_use_id: tool.id,
                        tool_name: tool.name,
                        result: execution.result,
                        synthetic: false,
                    },
                    Err(_) => StreamingToolResult {
                        tool_use_id: tool.id,
                        tool_name: tool.name,
                        result: ToolResult::typed_error(
                            format!(
                                "Tool call exceeded the Runtime wall-clock deadline of {} ms",
                                deadline.as_millis()
                            ),
                            "tool.deadline_exceeded",
                            true,
                            json!({
                                "deadlineMs": u64::try_from(deadline.as_millis()).unwrap_or(u64::MAX),
                                "cancelled": true,
                                "suggestedAction": "Use a bounded query or smaller operation, then retry once with changed input."
                            }),
                        ),
                        synthetic: true,
                    },
                }
            }
        }))
        .await
    }
}

async fn run_is_cancelled(store: &RuntimeStore, run_id: &str) -> bool {
    store
        .get_run(run_id)
        .await
        .is_some_and(|run| run.status == AgentRunStatus::Cancelled)
}

pub fn tool_result_error_text(result: &ToolResult) -> String {
    result
        .content
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("tool execution failed")
        .to_string()
}

pub fn tool_result_to_content(result: ToolResult) -> Value {
    result.content
}

pub fn missing_tool_result(tool_use_id: impl Into<String>, reason: impl Into<String>) -> Value {
    json!({
        "toolUseId": tool_use_id.into(),
        "isError": true,
        "content": { "error": reason.into() }
    })
}
