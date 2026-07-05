use crate::{
    conversation::RuntimeStore,
    model_gateway::{ModelClient, ModelRequest, ModelResponse, ToolCall},
    tools::{
        self,
        runtime::ToolExecutor,
        streaming::{tool_result_error_text, StreamingToolExecutor, StreamingToolResult},
    },
    types::{
        AgentCheckpoint, AgentEvent, AgentPhase, AgentRun, AgentRunStatus, Brief,
        CheckpointConversationRange,
    },
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{json, Value};
use std::{fs, sync::Arc};

const EMPTY_TURN_LIMIT: u32 = 3;
const MAX_TURNS: u32 = 12;
const COMPACT_MESSAGE_THRESHOLD: usize = 8;
const COMPACT_KEEP_RECENT: usize = 4;

#[derive(Debug, Clone)]
pub struct ToolResultMessage {
    pub tool_use_id: String,
    pub tool_name: String,
    pub is_error: bool,
    pub content: Value,
}

#[derive(Clone)]
pub struct AgentLoop {
    store: RuntimeStore,
    model: Arc<dyn ModelClient>,
    tool_executor: ToolExecutor,
}

impl AgentLoop {
    pub fn new(store: RuntimeStore, model: Arc<dyn ModelClient>) -> Self {
        Self {
            store,
            model,
            tool_executor: tools::control_plane::control_plane_executor(),
        }
    }

    pub fn with_tool_executor(
        store: RuntimeStore,
        model: Arc<dyn ModelClient>,
        tool_executor: ToolExecutor,
    ) -> Self {
        Self {
            store,
            model,
            tool_executor,
        }
    }

    pub async fn run(&self, run_id: &str) -> Result<Vec<ToolResultMessage>> {
        let run = self
            .store
            .update_run_status(run_id, AgentRunStatus::Running)
            .await?;
        let project_id = run.project_id.clone();
        self.store
            .append_event(AgentEvent::RunStarted {
                run_id: run_id.to_string(),
                label: format!("{} Agent", run.agent_profile),
                timestamp: Utc::now(),
            })
            .await;
        let start_message = format!("{} agent is preparing the run.", run.agent_profile);
        self.store
            .append_event(AgentEvent::AgentMessage {
                run_id: run_id.to_string(),
                text: start_message.clone(),
                timestamp: Utc::now(),
            })
            .await;
        self.store
            .append_conversation_item(
                &project_id,
                Some(run_id),
                "progress",
                Some("assistant"),
                start_message,
                None,
            )
            .await;

        if let Err(error) = self.bootstrap_sandbox_workspace(&run).await {
            self.finalize(
                run_id,
                AgentRunStatus::Failed,
                &format!("Workspace bootstrap failed: {error}"),
                &[],
            )
            .await?;
            return Ok(Vec::new());
        }

        let mut empty_turns = 0;
        let mut results = Vec::new();
        let mut message_window = self.recovered_message_window(run_id).await;

        for turn in 1..=MAX_TURNS {
            self.save_checkpoint(
                run_id,
                &message_window,
                format!("turn {turn} starting; empty_turns={empty_turns}"),
            )
            .await?;
            let current_run = self
                .store
                .get_run(run_id)
                .await
                .ok_or_else(|| anyhow!("run not found before model turn: {run_id}"))?;
            let (tools, deferred_tools) = self
                .tool_executor
                .model_tool_snapshot(self.store.clone(), run_id)
                .await;
            match self
                .model
                .next_response(ModelRequest {
                    run_id: run_id.to_string(),
                    turn,
                    model: current_run.model.clone(),
                    phase: current_run.phase,
                    agent_profile: current_run.agent_profile.clone(),
                    system_prompt: system_prompt_for_run(&current_run),
                    messages: message_window.clone(),
                    tools,
                    deferred_tools,
                })
                .await
            {
                Ok(ModelResponse::ToolCalls(calls)) => {
                    if calls.is_empty() {
                        message_window.push(json!({
                            "role": "assistant",
                            "turn": turn,
                            "toolCalls": [],
                        }));
                        empty_turns += 1;
                        if empty_turns >= EMPTY_TURN_LIMIT {
                            self.finalize(
                                run_id,
                                AgentRunStatus::Partial,
                                "No tool calls for 3 consecutive turns",
                                &message_window,
                            )
                            .await?;
                            break;
                        }
                        message_window.push(json!({
                            "role": "system",
                            "turn": turn,
                            "text": "Continue working or call run.complete if the task is done.",
                        }));
                        self.save_turn_checkpoint(run_id, turn, &message_window)
                            .await?;
                        self.compact_if_needed(run_id, &mut message_window).await?;
                        continue;
                    }
                    empty_turns = 0;

                    message_window.push(json!({
                        "role": "assistant",
                        "turn": turn,
                        "toolCalls": calls
                            .iter()
                            .map(|call| json!({
                                "id": call.id,
                                "name": call.name,
                                "input": call.input,
                            }))
                            .collect::<Vec<_>>(),
                    }));
                    let tool_results = self.execute_tools(run_id, calls).await;
                    for result in &tool_results {
                        message_window.push(json!({
                            "role": "tool",
                            "turn": turn,
                            "toolUseId": result.tool_use_id,
                            "toolName": result.tool_name,
                            "isError": result.is_error,
                            "content": result.content,
                        }));
                    }
                    self.save_turn_checkpoint(run_id, turn, &message_window)
                        .await?;
                    let completion = tool_results
                        .iter()
                        .find(|result| result.tool_name == "run.complete" && !result.is_error)
                        .map(|result| {
                            (
                                status_from_value(&result.content),
                                result
                                    .content
                                    .get("summary")
                                    .and_then(Value::as_str)
                                    .unwrap_or("Run completed.")
                                    .to_string(),
                            )
                        });
                    results.extend(tool_results);
                    if let Some((status, summary)) = completion {
                        self.finalize(run_id, status, &summary, &message_window)
                            .await?;
                        return Ok(results);
                    }
                    let should_stop = matches!(
                        self.store.get_run(run_id).await.map(|run| run.status),
                        Some(status) if status.is_terminal() || status == AgentRunStatus::NeedsUserInput
                    );
                    if should_stop {
                        return Ok(results);
                    }
                    self.compact_if_needed(run_id, &mut message_window).await?;
                }
                Ok(ModelResponse::ToolCallsThenError { calls, error }) => {
                    message_window.push(json!({
                        "role": "assistant",
                        "turn": turn,
                        "toolCalls": calls
                            .iter()
                            .map(|call| json!({
                                "id": call.id,
                                "name": call.name,
                                "input": call.input,
                            }))
                            .collect::<Vec<_>>(),
                    }));
                    self.record_tool_starts(run_id, &calls).await;
                    let missing_results =
                        self.emit_missing_tool_results(run_id, &calls, &error).await;
                    for result in &missing_results {
                        message_window.push(json!({
                            "role": "tool",
                            "turn": turn,
                            "toolUseId": result.tool_use_id,
                            "toolName": result.tool_name,
                            "isError": result.is_error,
                            "content": result.content,
                        }));
                    }
                    self.save_turn_checkpoint(run_id, turn, &message_window)
                        .await?;
                    results.extend(missing_results);
                    message_window.push(json!({
                        "role": "model",
                        "turn": turn,
                        "error": error,
                    }));
                    self.finalize(run_id, AgentRunStatus::Failed, &error, &message_window)
                        .await?;
                    break;
                }
                Ok(ModelResponse::ToolCallsThenFallback { calls, reason }) => {
                    message_window.push(json!({
                        "role": "assistant",
                        "turn": turn,
                        "toolCalls": calls
                            .iter()
                            .map(|call| json!({
                                "id": call.id,
                                "name": call.name,
                                "input": call.input,
                            }))
                            .collect::<Vec<_>>(),
                    }));
                    self.record_tool_starts(run_id, &calls).await;
                    let missing_results = self
                        .emit_missing_tool_results(run_id, &calls, &reason)
                        .await;
                    for result in &missing_results {
                        message_window.push(json!({
                            "role": "tool",
                            "turn": turn,
                            "toolUseId": result.tool_use_id,
                            "toolName": result.tool_name,
                            "isError": result.is_error,
                            "content": result.content,
                        }));
                    }
                    results.extend(missing_results);
                    let fallback_message = format!(
                        "Model fallback triggered: {reason}. Retrying with fallback model."
                    );
                    self.store
                        .append_event(AgentEvent::AgentMessage {
                            run_id: run_id.to_string(),
                            text: fallback_message.clone(),
                            timestamp: Utc::now(),
                        })
                        .await;
                    message_window.push(json!({
                        "role": "system",
                        "turn": turn,
                        "text": fallback_message,
                    }));
                    self.save_turn_checkpoint(run_id, turn, &message_window)
                        .await?;
                    self.compact_if_needed(run_id, &mut message_window).await?;
                    continue;
                }
                Ok(ModelResponse::TextOnly(text)) => {
                    self.store
                        .append_event(AgentEvent::AgentMessage {
                            run_id: run_id.to_string(),
                            text: text.clone(),
                            timestamp: Utc::now(),
                        })
                        .await;
                    self.store
                        .append_conversation_item(
                            &project_id,
                            Some(run_id),
                            "assistant_message",
                            Some("assistant"),
                            text.clone(),
                            None,
                        )
                        .await;
                    message_window.push(json!({
                        "role": "assistant",
                        "turn": turn,
                        "text": text,
                    }));
                    empty_turns += 1;
                    self.save_turn_checkpoint(run_id, turn, &message_window)
                        .await?;
                    if empty_turns >= EMPTY_TURN_LIMIT {
                        self.finalize(
                            run_id,
                            AgentRunStatus::Partial,
                            "No tool calls for 3 consecutive turns",
                            &message_window,
                        )
                        .await?;
                        break;
                    }
                    self.compact_if_needed(run_id, &mut message_window).await?;
                }
                Ok(ModelResponse::Error(error)) => {
                    message_window.push(json!({
                        "role": "model",
                        "turn": turn,
                        "error": error,
                    }));
                    self.save_turn_checkpoint(run_id, turn, &message_window)
                        .await?;
                    self.finalize(run_id, AgentRunStatus::Failed, &error, &message_window)
                        .await?;
                    break;
                }
                Err(error) => {
                    let error = error.to_string();
                    message_window.push(json!({
                        "role": "runtime",
                        "turn": turn,
                        "error": error,
                    }));
                    self.save_turn_checkpoint(run_id, turn, &message_window)
                        .await?;
                    self.finalize(run_id, AgentRunStatus::Failed, &error, &message_window)
                        .await?;
                    break;
                }
            }
        }

        let run = self
            .store
            .get_run(run_id)
            .await
            .ok_or_else(|| anyhow!("run not found after loop"))?;
        if !run.status.is_terminal() && run.status != AgentRunStatus::NeedsUserInput {
            self.finalize(
                run_id,
                AgentRunStatus::Partial,
                "Reached max turns",
                &message_window,
            )
            .await?;
        }

        Ok(results)
    }

    async fn bootstrap_sandbox_workspace(&self, run: &AgentRun) -> Result<()> {
        if !matches!(run.phase, AgentPhase::Build | AgentPhase::Edit) {
            return Ok(());
        }
        let Some(brief_id) = run.brief_version.as_deref() else {
            return Ok(());
        };
        let brief = self
            .store
            .get_brief(brief_id)
            .await
            .ok_or_else(|| anyhow!("brief not found: {brief_id}"))?;
        let content_sources = self.store.content_sources(&run.id).await;
        let readable_sources = content_sources
            .iter()
            .filter(|source| source.readable)
            .map(|source| {
                json!({
                    "id": source.id,
                    "kind": source.kind,
                    "text": source.text,
                })
            })
            .collect::<Vec<_>>();

        self.write_workspace_file(
            run,
            "inputs/brief.md",
            render_brief_markdown(brief_id, &brief),
        )
        .await?;
        self.write_workspace_file(
            run,
            "inputs/content-sources.json",
            serde_json::to_string_pretty(&readable_sources)?,
        )
        .await?;

        let design_context = content_sources
            .iter()
            .filter(|source| source.readable && source.kind == "design_md")
            .map(|source| source.text.as_str())
            .collect::<Vec<_>>();
        if !design_context.is_empty() {
            self.write_workspace_file(run, "inputs/design.md", design_context.join("\n\n---\n\n"))
                .await?;
        }
        self.write_workspace_file(run, "state/tasks.json", "[]".to_string())
            .await?;
        self.write_workspace_file(run, "state/preview.json", "{}".to_string())
            .await?;
        self.store
            .append_conversation_item(
                &run.project_id,
                Some(&run.id),
                "progress",
                Some("assistant"),
                "Workspace inputs prepared for sandbox execution.",
                Some(json!({
                    "briefId": brief_id,
                    "contentSourceCount": readable_sources.len(),
                })),
            )
            .await;
        Ok(())
    }

    async fn write_workspace_file(&self, run: &AgentRun, path: &str, text: String) -> Result<()> {
        let tool_use_id = format!("bootstrap:{path}");
        let tool_call = ToolCall::new(
            tool_use_id.clone(),
            "fs.write",
            json!({ "path": path, "text": text }),
        );
        self.record_tool_starts(&run.id, std::slice::from_ref(&tool_call))
            .await;
        let execution = self
            .tool_executor
            .execute(
                self.store.clone(),
                &run.id,
                &tool_use_id,
                &tool_call.name,
                tool_call.input.clone(),
            )
            .await;
        let result = self
            .record_tool_result(
                &run.id,
                StreamingToolResult {
                    tool_use_id,
                    tool_name: tool_call.name,
                    result: execution.result,
                    synthetic: false,
                },
            )
            .await;
        if result.is_error {
            return Err(anyhow!(result
                .content
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("fs.write failed during workspace bootstrap")
                .to_string()));
        }
        Ok(())
    }

    async fn execute_tools(&self, run_id: &str, calls: Vec<ToolCall>) -> Vec<ToolResultMessage> {
        self.record_tool_starts(run_id, &calls).await;

        let streaming = StreamingToolExecutor::new(self.tool_executor.clone());
        let results = streaming
            .execute_calls(self.store.clone(), run_id, calls)
            .await;

        let mut messages = Vec::new();
        for result in results {
            messages.push(self.record_tool_result(run_id, result).await);
        }
        messages
    }

    async fn record_tool_starts(&self, run_id: &str, calls: &[ToolCall]) {
        for call in calls {
            self.store
                .append_event(AgentEvent::ToolStarted {
                    run_id: run_id.to_string(),
                    tool: call.name.clone(),
                    summary: format!("Running {}", call.name),
                    tool_use_id: call.id.clone(),
                    timestamp: Utc::now(),
                })
                .await;
        }
    }

    async fn emit_missing_tool_results(
        &self,
        run_id: &str,
        calls: &[ToolCall],
        reason: &str,
    ) -> Vec<ToolResultMessage> {
        let mut messages = Vec::new();
        for call in calls {
            let error = format!("Tool call did not complete: {reason}");
            self.store
                .append_event(AgentEvent::ToolFailed {
                    run_id: run_id.to_string(),
                    tool: call.name.clone(),
                    error: error.clone(),
                    tool_use_id: call.id.clone(),
                    recoverable: false,
                    timestamp: Utc::now(),
                })
                .await;
            self.append_tool_conversation_item(
                run_id,
                "tool_failed",
                format!(
                    "{} failed: {}",
                    call.name,
                    truncate_conversation_text(&error)
                ),
                json!({
                    "tool": call.name.clone(),
                    "toolUseId": call.id.clone(),
                    "recoverable": false,
                    "synthetic": true,
                }),
            )
            .await;
            messages.push(ToolResultMessage {
                tool_use_id: call.id.clone(),
                tool_name: call.name.clone(),
                is_error: true,
                content: json!({ "error": error }),
            });
        }
        messages
    }

    async fn record_tool_result(
        &self,
        run_id: &str,
        result: StreamingToolResult,
    ) -> ToolResultMessage {
        if result.result.is_error {
            let error = tool_result_error_text(&result.result);
            let recoverable = result
                .result
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("recoverable"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            self.store
                .append_event(AgentEvent::ToolFailed {
                    run_id: run_id.to_string(),
                    tool: result.tool_name.clone(),
                    error: error.clone(),
                    tool_use_id: result.tool_use_id.clone(),
                    recoverable,
                    timestamp: Utc::now(),
                })
                .await;
            self.append_tool_conversation_item(
                run_id,
                "tool_failed",
                format!(
                    "{} failed: {}",
                    result.tool_name,
                    truncate_conversation_text(&error)
                ),
                json!({
                    "tool": result.tool_name.clone(),
                    "toolUseId": result.tool_use_id.clone(),
                    "recoverable": recoverable,
                }),
            )
            .await;
            return ToolResultMessage {
                tool_use_id: result.tool_use_id,
                tool_name: result.tool_name,
                is_error: true,
                content: json!({ "error": error }),
            };
        }

        let summary = tool_summary(&result.tool_name, false);
        self.store
            .append_event(AgentEvent::ToolCompleted {
                run_id: run_id.to_string(),
                tool: result.tool_name.clone(),
                summary: summary.clone(),
                tool_use_id: result.tool_use_id.clone(),
                metadata: None,
                timestamp: Utc::now(),
            })
            .await;
        self.append_tool_conversation_item(
            run_id,
            "tool_completed",
            summary,
            json!({
                "tool": result.tool_name.clone(),
                "toolUseId": result.tool_use_id.clone(),
            }),
        )
        .await;
        ToolResultMessage {
            tool_use_id: result.tool_use_id,
            tool_name: result.tool_name,
            is_error: false,
            content: result.result.content,
        }
    }

    async fn finalize(
        &self,
        run_id: &str,
        status: AgentRunStatus,
        summary: &str,
        message_window: &[Value],
    ) -> Result<()> {
        if status == AgentRunStatus::Partial {
            self.save_checkpoint(run_id, message_window, summary.to_string())
                .await?;
        }
        self.store.update_run_status(run_id, status).await?;
        self.store
            .append_event(AgentEvent::RunCompleted {
                run_id: run_id.to_string(),
                status: status_string(status).to_string(),
                summary: summary.to_string(),
                timestamp: Utc::now(),
            })
            .await;
        if let Some(run) = self.store.get_run(run_id).await {
            self.append_run_completed_conversation_item(
                run_id,
                run.project_id,
                status_string(status),
                summary,
            )
            .await;
        }
        Ok(())
    }

    async fn append_tool_conversation_item(
        &self,
        run_id: &str,
        kind: &str,
        text: impl Into<String>,
        metadata: Value,
    ) {
        if let Some(run) = self.store.get_run(run_id).await {
            self.store
                .append_conversation_item(
                    &run.project_id,
                    Some(run_id),
                    kind,
                    Some("assistant"),
                    text,
                    Some(metadata),
                )
                .await;
        }
    }

    async fn append_run_completed_conversation_item(
        &self,
        run_id: &str,
        project_id: String,
        status: &str,
        summary: &str,
    ) {
        self.store
            .append_conversation_item(
                &project_id,
                Some(run_id),
                "run_completed",
                Some("assistant"),
                summary,
                Some(json!({ "status": status })),
            )
            .await;
    }

    async fn save_checkpoint(
        &self,
        run_id: &str,
        message_window: &[Value],
        context_summary: String,
    ) -> Result<()> {
        let run = self
            .store
            .get_run(run_id)
            .await
            .ok_or_else(|| anyhow!("run not found for checkpoint: {run_id}"))?;
        let (message_window, conversation_range) = recent_messages_with_range(message_window);
        let checkpoint = AgentCheckpoint {
            id: self.store.next_id("checkpoint"),
            run_id: run.id.clone(),
            project_id: run.project_id.clone(),
            phase: run.phase,
            message_window,
            conversation_range,
            task_list: Vec::new(),
            workspace_snapshot_uri: None,
            build_result: None,
            brief_version: run.brief_version,
            design_version: run.design_version,
            last_known_preview_url: None,
            context_summary,
            created_at: Utc::now(),
        };
        self.store.save_checkpoint(checkpoint).await
    }

    async fn recovered_message_window(&self, run_id: &str) -> Vec<Value> {
        self.store
            .latest_checkpoint_for_run(run_id)
            .await
            .map(|checkpoint| checkpoint.message_window)
            .unwrap_or_default()
    }

    async fn save_turn_checkpoint(
        &self,
        run_id: &str,
        turn: u32,
        message_window: &[Value],
    ) -> Result<()> {
        self.save_checkpoint(
            run_id,
            message_window,
            format!("turn {turn} transcript captured"),
        )
        .await
    }

    async fn compact_if_needed(&self, run_id: &str, message_window: &mut Vec<Value>) -> Result<()> {
        if message_window.len() <= COMPACT_MESSAGE_THRESHOLD {
            return Ok(());
        }

        let compacted_count = message_window.len().saturating_sub(COMPACT_KEEP_RECENT);
        let recent = message_window
            .iter()
            .skip(compacted_count)
            .cloned()
            .collect::<Vec<_>>();
        let compacted = message_window
            .iter()
            .take(compacted_count)
            .cloned()
            .collect::<Vec<_>>();
        let context_path = self.tool_executor.workspace_root().join("state/context.md");
        if let Some(parent) = context_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let previous_context = fs::read_to_string(&context_path).ok();
        fs::write(
            &context_path,
            render_compacted_context(
                run_id,
                compacted_count,
                previous_context.as_deref(),
                &compacted,
            ),
        )?;

        let summary = json!({
            "role": "system",
            "kind": "compact_summary",
            "text": format!(
                "Older conversation compacted to state/context.md; retained the last {} messages.",
                recent.len()
            ),
            "contextPath": "state/context.md",
            "compactedMessages": compacted_count,
        });
        message_window.clear();
        message_window.push(summary);
        message_window.extend(recent);
        self.save_checkpoint(
            run_id,
            message_window,
            "Compacted conversation history to state/context.md".to_string(),
        )
        .await?;
        Ok(())
    }
}

fn recent_messages_with_range(
    messages: &[Value],
) -> (Vec<Value>, Option<CheckpointConversationRange>) {
    const MAX_MESSAGE_WINDOW: usize = 20;
    let start_index = messages.len().saturating_sub(MAX_MESSAGE_WINDOW);
    let retained = messages
        .iter()
        .skip(start_index)
        .cloned()
        .collect::<Vec<_>>();
    let range = (!retained.is_empty()).then_some(CheckpointConversationRange {
        start_index: start_index as u64,
        end_index_exclusive: messages.len() as u64,
        retained_count: retained.len() as u64,
    });
    (retained, range)
}

fn render_compacted_context(
    run_id: &str,
    compacted_count: usize,
    previous_context: Option<&str>,
    compacted: &[Value],
) -> String {
    let mut output = format!(
        "# Runtime Context Compact\n\nRun: {run_id}\nCompacted messages: {compacted_count}\n\n## Messages\n\n"
    );
    if let Some(previous_context) = previous_context {
        output.push_str("## Previous Compact\n\n");
        output.push_str(previous_context);
        if !previous_context.ends_with('\n') {
            output.push('\n');
        }
        output.push('\n');
        output.push_str("## Newly Compacted Messages\n\n");
    }
    for (index, message) in compacted.iter().enumerate() {
        output.push_str(&format!(
            "### Message {}\n\n```json\n{}\n```\n\n",
            index + 1,
            serde_json::to_string_pretty(message).unwrap_or_else(|_| message.to_string())
        ));
    }
    output
}

fn tool_summary(name: &str, is_error: bool) -> String {
    if is_error {
        return format!("{name} failed");
    }
    match name {
        "content.list_sources" => "Listed content sources".to_string(),
        "content.read_source" => "Read content source".to_string(),
        "brief.write_draft" | "brief.update" => "Wrote brief draft".to_string(),
        "brief.request_confirmation" => "Requested brief confirmation".to_string(),
        "run.report_progress" => "Reported progress".to_string(),
        "run.complete" => "Completed run".to_string(),
        "user.ask" => "Asked user".to_string(),
        other => format!("Ran {other}"),
    }
}

fn truncate_conversation_text(text: &str) -> String {
    const MAX_CHARS: usize = 500;
    let mut chars = text.chars();
    let truncated = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

pub fn status_string(status: AgentRunStatus) -> &'static str {
    match status {
        AgentRunStatus::Queued => "queued",
        AgentRunStatus::Running => "running",
        AgentRunStatus::NeedsUserInput => "needs_user_input",
        AgentRunStatus::Completed => "completed",
        AgentRunStatus::Partial => "partial",
        AgentRunStatus::Blocked => "blocked",
        AgentRunStatus::Failed => "failed",
        AgentRunStatus::Cancelled => "cancelled",
    }
}

fn status_from_value(content: &Value) -> AgentRunStatus {
    match content.get("status").and_then(Value::as_str) {
        Some("partial") => AgentRunStatus::Partial,
        Some("blocked") => AgentRunStatus::Blocked,
        Some("failed") => AgentRunStatus::Failed,
        Some("cancelled") => AgentRunStatus::Cancelled,
        Some("completed" | "success") | None => AgentRunStatus::Completed,
        Some(_) => AgentRunStatus::Completed,
    }
}

fn system_prompt_for_run(run: &AgentRun) -> String {
    let phase_instruction = match run.phase {
        AgentPhase::Brief => {
            "Create a structured Brief draft from the provided content, then request user confirmation before completing."
        }
        AgentPhase::Build => {
            "Generate or update the sandbox workspace, build the Astro website, verify preview readiness, and only complete after preview promotion."
        }
        AgentPhase::Edit => {
            "Apply focused changes to the existing project version, rebuild, verify, and emit a promoted preview before completing."
        }
        AgentPhase::Review => {
            "Review the candidate preview using read-only tools and report actionable findings."
        }
        AgentPhase::Repair => {
            "Repair the targeted review finding within the scoped workspace and stop if the same failure repeats."
        }
        AgentPhase::Export => "Prepare export artifacts from the current promoted project version.",
    };
    format!(
        "You are the AnyDesign runtime {profile} agent.\nProject: {project_id}\nRun: {run_id}\nPhase: {phase:?}\n{phase_instruction}\nUse only the provided tools. Preserve the tool_use/tool_result invariant. Respect the sandbox workspace boundary.",
        profile = run.agent_profile,
        project_id = run.project_id,
        run_id = run.id,
        phase = run.phase,
    )
}

fn render_brief_markdown(brief_id: &str, brief: &Brief) -> String {
    let hierarchy = brief
        .content_hierarchy
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "# Brief {brief_id}\n\nProject type: {}\nAudience: {}\nTemplate: {}\nVisual direction: {}\n\n## Content hierarchy\n{}\n\n## Page structure\n{}\n\n## Assumptions\n{}\n\n## Missing information\n{}\n",
        brief.project_type,
        brief.audience,
        brief.recommended_template,
        brief.visual_direction,
        hierarchy,
        serde_json::to_string_pretty(&brief.page_structure).unwrap_or_else(|_| "{}".to_string()),
        render_markdown_list(&brief.assumptions),
        render_markdown_list(&brief.missing_information),
    )
}

fn render_markdown_list(items: &[String]) -> String {
    if items.is_empty() {
        return "- None".to_string();
    }
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}
