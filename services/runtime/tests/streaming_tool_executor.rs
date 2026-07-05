use anydesign_runtime::{
    conversation::RuntimeStore,
    model_gateway::ToolCall,
    permission::{PermissionReason, PermissionResult, PermissionRules},
    tools::{
        runtime::{
            InterruptBehavior, ProgressSink, Tool, ToolContext, ToolError, ToolExecutor, ToolResult,
        },
        streaming::StreamingToolExecutor,
    },
    types::{AgentEvent, AgentPhase, AgentRunStatus},
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{fs, path::PathBuf, sync::Arc};
use tokio::sync::Notify;

struct SafeReadTool;

#[async_trait]
impl Tool for SafeReadTool {
    fn name(&self) -> &'static str {
        "safe.read"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["safe.alias"]
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow(input)
    }

    async fn call(
        &self,
        _input: Value,
        _ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::ok(json!({ "read": true })))
    }
}

struct FailingTool;

#[async_trait]
impl Tool for FailingTool {
    fn name(&self) -> &'static str {
        "safe.fail"
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow(input)
    }

    async fn call(
        &self,
        _input: Value,
        _ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::Recoverable("ordinary tool failed".to_string()))
    }
}

struct ShellRunTool;

#[async_trait]
impl Tool for ShellRunTool {
    fn name(&self) -> &'static str {
        "shell.run"
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow(input)
    }

    async fn call(
        &self,
        _input: Value,
        _ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::Recoverable("shell failed".to_string()))
    }
}

struct CancellingTool;

#[async_trait]
impl Tool for CancellingTool {
    fn name(&self) -> &'static str {
        "run.cancel_self"
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow(input)
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        ctx.store
            .update_run_status(&ctx.run.id, AgentRunStatus::Cancelled)
            .await
            .unwrap();
        Ok(ToolResult::ok(json!({ "cancelled": true })))
    }
}

struct InterruptCancelTool;

#[async_trait]
impl Tool for InterruptCancelTool {
    fn name(&self) -> &'static str {
        "interrupt.cancel"
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn interrupt_behavior(&self) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow(input)
    }

    async fn call(
        &self,
        _input: Value,
        _ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::ok(json!({ "cancelToolRan": true })))
    }
}

struct HugeResultTool;

#[async_trait]
impl Tool for HugeResultTool {
    fn name(&self) -> &'static str {
        "huge.result"
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow(input)
    }

    async fn call(
        &self,
        _input: Value,
        _ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::ok(json!({
            "text": "x".repeat(3000),
            "keep": "metadata"
        })))
    }

    fn max_result_size_chars(&self) -> usize {
        80
    }
}

struct ProgressThenWaitTool {
    release: Arc<Notify>,
}

#[async_trait]
impl Tool for ProgressThenWaitTool {
    fn name(&self) -> &'static str {
        "progress.wait"
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow(input)
    }

    async fn call(
        &self,
        _input: Value,
        _ctx: ToolContext,
        progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        progress.emit("Still working").await;
        self.release.notified().await;
        Ok(ToolResult::ok(json!({ "done": true })))
    }
}

async fn create_run(store: &RuntimeStore) -> String {
    store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await
        .id
}

fn executor_with_test_tools() -> StreamingToolExecutor {
    StreamingToolExecutor::new(ToolExecutor::new(
        vec![
            Arc::new(SafeReadTool),
            Arc::new(FailingTool),
            Arc::new(ShellRunTool),
            Arc::new(CancellingTool),
            Arc::new(InterruptCancelTool),
            Arc::new(HugeResultTool),
        ],
        PermissionRules::default(),
    ))
}

fn allow(input: &Value) -> PermissionResult {
    PermissionResult::Allow {
        updated_input: input.clone(),
        reason: PermissionReason::Other {
            reason: "test allow".to_string(),
        },
    }
}

fn assert_unrecoverable(result: &ToolResult) {
    assert_eq!(
        result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("recoverable"))
            .and_then(Value::as_bool),
        Some(false)
    );
}

#[test]
fn add_tool_caches_concurrency_safety_and_aliases() {
    let executor = executor_with_test_tools();
    let tracked = executor.track_calls(vec![
        ToolCall::new("tool-1", "safe.read", json!({})),
        ToolCall::new("tool-2", "safe.alias", json!({})),
        ToolCall::new("tool-3", "safe.fail", json!({})),
        ToolCall::new("tool-4", "missing.tool", json!({})),
    ]);

    assert!(tracked[0].is_concurrency_safe);
    assert!(tracked[1].is_concurrency_safe);
    assert!(!tracked[2].is_concurrency_safe);
    assert!(!tracked[3].is_concurrency_safe);
}

#[tokio::test]
async fn unknown_tool_returns_synthetic_error_result() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = executor_with_test_tools();

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new("tool-1", "missing.tool", json!({}))],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].synthetic);
    assert!(results[0].result.is_error);
    assert!(results[0].result.content["error"]
        .as_str()
        .unwrap()
        .contains("No such tool"));
}

#[tokio::test]
async fn shell_error_cancels_later_sibling_tools() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = executor_with_test_tools();

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new("tool-1", "shell.run", json!({})),
                ToolCall::new("tool-2", "safe.read", json!({})),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(results[0].result.is_error);
    assert!(results[1].synthetic);
    assert!(results[1].result.content["error"]
        .as_str()
        .unwrap()
        .contains("shell.run failed"));
}

#[tokio::test]
async fn non_shell_error_does_not_cancel_later_sibling_tools() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = executor_with_test_tools();

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new("tool-1", "safe.fail", json!({})),
                ToolCall::new("tool-2", "safe.read", json!({})),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(results[0].result.is_error);
    assert!(!results[1].result.is_error);
    assert!(!results[1].synthetic);
}

#[tokio::test]
async fn continue_interrupt_blocks_block_tools_and_synthetically_cancels_cancel_tools() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    store.request_continue_interrupt(&run_id).await;
    let executor = executor_with_test_tools();

    let results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![
                ToolCall::new("tool-block", "safe.read", json!({})),
                ToolCall::new("tool-cancel", "interrupt.cancel", json!({})),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].tool_use_id, "tool-block");
    assert!(!results[0].synthetic);
    assert_eq!(results[0].result.content["read"], true);
    assert_eq!(results[1].tool_use_id, "tool-cancel");
    assert!(results[1].synthetic);
    assert!(results[1].result.content["error"]
        .as_str()
        .unwrap()
        .contains("new user message"));
    assert_unrecoverable(&results[1].result);
    assert!(!store.continue_interrupt_requested(&run_id).await);
}

#[tokio::test]
async fn cancelled_run_returns_synthetic_results_for_all_queued_tools() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    store
        .update_run_status(&run_id, AgentRunStatus::Cancelled)
        .await
        .unwrap();
    let executor = executor_with_test_tools();

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new("tool-1", "safe.read", json!({})),
                ToolCall::new("tool-2", "safe.read", json!({})),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|result| result.synthetic));
    assert!(results.iter().all(|result| result.result.is_error));
    assert!(results.iter().all(|result| result.result.content["error"]
        .as_str()
        .unwrap()
        .contains("run was cancelled")));
    assert!(results.iter().all(|result| result
        .result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("recoverable"))
        .and_then(Value::as_bool)
        == Some(false)));
}

#[tokio::test]
async fn cancellation_after_wave_preserves_completed_result_and_interrupts_queued_tools() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = executor_with_test_tools();

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new("tool-1", "run.cancel_self", json!({})),
                ToolCall::new("tool-2", "safe.read", json!({})),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(!results[0].synthetic);
    assert!(!results[0].result.is_error);
    assert_eq!(results[0].result.content["cancelled"], true);
    assert!(results[1].synthetic);
    assert!(results[1].result.content["error"]
        .as_str()
        .unwrap()
        .contains("run was cancelled"));
    assert_unrecoverable(&results[1].result);
}

#[tokio::test]
async fn oversized_tool_result_is_truncated_and_written_to_workspace_artifact() {
    let workspace = unique_temp_dir("streaming-tool-result");
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        vec![Arc::new(HugeResultTool)],
        PermissionRules::default(),
        &workspace,
    ));

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new("tool-big", "huge.result", json!({}))],
        )
        .await;

    assert_eq!(results.len(), 1);
    let content = &results[0].result.content;
    assert_eq!(content["truncated"], true);
    assert_eq!(
        content["path"],
        "/workspace/outputs/tool-results/tool-big.json"
    );
    assert!(content["preview"].as_str().unwrap().len() < 3000);
    let artifact = fs::read_to_string(workspace.join("outputs/tool-results/tool-big.json"))
        .expect("full result artifact should be written");
    assert!(artifact.contains("\"keep\": \"metadata\""));
    assert!(artifact.contains(&"x".repeat(3000)));
}

#[tokio::test]
async fn progress_events_are_visible_before_tool_completion() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let release = Arc::new(Notify::new());
    let executor = StreamingToolExecutor::new(ToolExecutor::new(
        vec![Arc::new(ProgressThenWaitTool {
            release: release.clone(),
        })],
        PermissionRules::default(),
    ));

    let task_store = store.clone();
    let task_run_id = run_id.clone();
    let handle = tokio::spawn(async move {
        executor
            .execute_calls(
                task_store,
                &task_run_id,
                vec![ToolCall::new("tool-progress", "progress.wait", json!({}))],
            )
            .await
    });

    let mut saw_progress = false;
    for _ in 0..20 {
        if store.events(&run_id).await.iter().any(|event| {
            matches!(
                event,
                AgentEvent::ToolStarted {
                    tool,
                    tool_use_id,
                    summary,
                    ..
                } if tool == "progress"
                    && tool_use_id == "tool-progress"
                    && summary == "Still working"
            )
        }) {
            saw_progress = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert!(
        saw_progress,
        "progress event should be visible before completion"
    );
    assert!(
        !handle.is_finished(),
        "tool should still be waiting after progress is emitted"
    );

    release.notify_one();
    let results = handle.await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].result.content["done"], true);
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "{prefix}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}
