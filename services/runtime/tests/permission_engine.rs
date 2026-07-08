use anydesign_runtime::{
    conversation::RuntimeStore,
    permission::{
        check_command_policy, check_create_path, check_existing_path, PermissionReason,
        PermissionRequestHookDecision, PermissionResult, PermissionRules, PreToolUseHookDecision,
    },
    tools::runtime::{
        ProgressSink, Tool, ToolContext, ToolError, ToolExecutor, ToolResult, ValidationError,
    },
    types::{AgentEvent, AgentPhase, AgentRunStatus},
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{fs, path::PathBuf, sync::Arc};

struct PassthroughTool;

#[async_trait]
impl Tool for PassthroughTool {
    fn name(&self) -> &'static str {
        "test.passthrough"
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn call(
        &self,
        _input: Value,
        _ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::ok(json!({ "ok": true })))
    }
}

struct ValidatingTool;

#[async_trait]
impl Tool for ValidatingTool {
    fn name(&self) -> &'static str {
        "test.validating"
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn validate_input(
        &self,
        _input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        Err(ValidationError::new("validation failed before permission"))
    }

    async fn call(
        &self,
        _input: Value,
        _ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::ok(json!({ "unreachable": true })))
    }
}

struct AllowTool;

#[async_trait]
impl Tool for AllowTool {
    fn name(&self) -> &'static str {
        "test.allow"
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        PermissionResult::Allow {
            updated_input: input.clone(),
            reason: PermissionReason::Other {
                reason: "test allow".to_string(),
            },
        }
    }

    async fn call(
        &self,
        _input: Value,
        _ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::ok(json!({ "ok": true })))
    }
}

struct EchoPathTool;

#[async_trait]
impl Tool for EchoPathTool {
    fn name(&self) -> &'static str {
        "package.install"
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn check_permission(&self, _input: &Value, _ctx: &ToolContext) -> PermissionResult {
        PermissionResult::Ask {
            message: "echo_path requires approval".to_string(),
            reason: PermissionReason::Rule {
                source: anydesign_runtime::permission::RuleSource::Runtime,
                rule_content: "test ask".to_string(),
            },
            suggestions: None,
        }
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::ok(json!({ "path": input["path"] })))
    }
}

struct EchoPathAllowTool;

#[async_trait]
impl Tool for EchoPathAllowTool {
    fn name(&self) -> &'static str {
        "test.echo_path_allow"
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        PermissionResult::Allow {
            updated_input: input.clone(),
            reason: PermissionReason::Other {
                reason: "test path allowed".to_string(),
            },
        }
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::ok(json!({ "path": input["path"] })))
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

async fn create_headless_run(store: &RuntimeStore) -> String {
    let parent = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .create_child_run(
            &parent.id,
            AgentPhase::Repair,
            "repair".to_string(),
            "internal-balanced".to_string(),
            None,
            vec![],
        )
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn validation_error_skips_permission_flow_but_writes_runtime_audit() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = ToolExecutor::new(vec![Arc::new(ValidatingTool)], PermissionRules::default());

    let execution = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-1",
            "test.validating",
            json!({}),
        )
        .await;

    assert!(execution.result.is_error);
    assert!(execution.result.content["error"]
        .as_str()
        .unwrap()
        .contains("validation failed"));
    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].tool, "test.validating");
    assert_eq!(audits[0].decision, "deny");
    assert!(audits[0].reason.contains("input validation failed"));
}

#[tokio::test]
async fn unknown_tool_writes_runtime_audit() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = ToolExecutor::new(vec![Arc::new(AllowTool)], PermissionRules::default());

    let execution = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-1",
            "test.missing",
            json!({ "path": "/workspace/project/a.txt" }),
        )
        .await;

    assert!(execution.result.is_error);
    assert!(execution.result.content["error"]
        .as_str()
        .unwrap()
        .contains("No such tool available"));
    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].tool, "test.missing");
    assert_eq!(audits[0].decision, "deny");
    assert_eq!(audits[0].reason, "tool is not registered");
}

#[tokio::test]
async fn passthrough_defaults_to_permission_request_and_audit() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = ToolExecutor::new(vec![Arc::new(PassthroughTool)], PermissionRules::default());

    let execution = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-1",
            "test.passthrough",
            json!({ "path": "/workspace/project/a.txt" }),
        )
        .await;

    assert!(execution.result.is_error);
    assert_eq!(
        store.get_run(&run_id).await.unwrap().status,
        AgentRunStatus::NeedsUserInput
    );
    let events = store.events(&run_id).await;
    assert!(events
        .iter()
        .any(|event| serde_json::to_value(event).unwrap()["type"] == "permission.requested"));
    assert!(events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::AgentMessage { text, .. }
                if text.contains("Permission required for test.passthrough")
        )
    }));
    let conversation = store.conversation_items("project-1").await;
    assert!(conversation.iter().any(|item| {
        item.kind == "permission_requested"
            && item.text.contains("test.passthrough")
            && item.metadata.as_ref().is_some_and(|metadata| {
                metadata["tool"] == "test.passthrough"
                    && metadata
                        .get("permissionId")
                        .is_some_and(|value| value.is_string())
            })
    }));
    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].decision, "ask");
}

#[tokio::test]
async fn deny_rule_wins_over_always_allow_and_writes_audit() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let rules = PermissionRules::default()
        .deny_tool("test.passthrough")
        .always_allow_tool("test.passthrough");
    let executor = ToolExecutor::new(vec![Arc::new(PassthroughTool)], rules);

    let execution = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-1",
            "test.passthrough",
            json!({}),
        )
        .await;

    assert!(execution.result.is_error);
    let events = store.events(&run_id).await;
    assert!(events
        .iter()
        .any(|event| serde_json::to_value(event).unwrap()["type"] == "permission.denied"));
    let conversation = store.conversation_items("project-1").await;
    assert!(conversation.iter().any(|item| {
        item.kind == "permission_denied"
            && item.text.contains("test.passthrough")
            && item
                .metadata
                .as_ref()
                .is_some_and(|metadata| metadata["tool"] == "test.passthrough")
    }));
    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].decision, "deny");
}

#[tokio::test]
async fn allow_decision_writes_audit() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = ToolExecutor::new(vec![Arc::new(AllowTool)], PermissionRules::default());

    let execution = executor
        .execute(store.clone(), &run_id, "tool-1", "test.allow", json!({}))
        .await;

    assert!(!execution.result.is_error);
    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].decision, "allow");
}

#[tokio::test]
async fn headless_ask_without_permission_request_hook_auto_denies() {
    let store = RuntimeStore::new();
    let run_id = create_headless_run(&store).await;
    let executor = ToolExecutor::new(vec![Arc::new(EchoPathTool)], PermissionRules::default());

    let execution = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-1",
            "package.install",
            json!({ "path": "project/a.txt" }),
        )
        .await;

    assert!(execution.result.is_error);
    assert!(execution.result.content["error"]
        .as_str()
        .unwrap()
        .contains("Permission prompts are not available"));
    let run = store.get_run(&run_id).await.unwrap();
    assert_ne!(run.status, AgentRunStatus::NeedsUserInput);
    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].decision, "deny");
    assert!(audits[0].reason.contains("headless mode"));
}

#[tokio::test]
async fn headless_permission_request_hook_can_allow_and_update_input() {
    let store = RuntimeStore::new();
    let run_id = create_headless_run(&store).await;
    let rules = PermissionRules::default().permission_request_hook(
        "package.install",
        PermissionRequestHookDecision::allow(
            "runtime hook approved scoped path",
            Some(json!({ "path": "project/approved.txt" })),
        ),
    );
    let executor = ToolExecutor::new(vec![Arc::new(EchoPathTool)], rules);

    let execution = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-1",
            "package.install",
            json!({ "path": "project/requested.txt" }),
        )
        .await;

    assert!(!execution.result.is_error);
    assert_eq!(execution.result.content["path"], "project/approved.txt");
    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].decision, "allow");
    assert!(audits[0].reason.contains("PermissionRequest"));
    assert!(audits[0].input_summary.contains("approved.txt"));
}

#[tokio::test]
async fn headless_permission_request_hook_can_deny_without_user_prompt() {
    let store = RuntimeStore::new();
    let run_id = create_headless_run(&store).await;
    let rules = PermissionRules::default().permission_request_hook(
        "package.install",
        PermissionRequestHookDecision::deny("runtime hook rejected public package install"),
    );
    let executor = ToolExecutor::new(vec![Arc::new(EchoPathTool)], rules);

    let execution = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-1",
            "package.install",
            json!({ "path": "project/requested.txt" }),
        )
        .await;

    assert!(execution.result.is_error);
    assert!(execution.result.content["error"]
        .as_str()
        .unwrap()
        .contains("runtime hook rejected"));
    assert_ne!(
        store.get_run(&run_id).await.unwrap().status,
        AgentRunStatus::NeedsUserInput
    );
    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].decision, "deny");
    assert!(audits[0].reason.contains("PermissionRequest"));
}

#[tokio::test]
async fn pre_tool_allow_does_not_bypass_deny_rule() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let rules = PermissionRules::default()
        .deny_tool("test.passthrough")
        .pre_tool_use_hook(
            "test.passthrough",
            PreToolUseHookDecision::allow("pre hook tried allow", None),
        );
    let executor = ToolExecutor::new(vec![Arc::new(PassthroughTool)], rules);

    let execution = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-1",
            "test.passthrough",
            json!({}),
        )
        .await;

    assert!(execution.result.is_error);
    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].decision, "deny");
    assert!(audits[0].reason.contains("deny test.passthrough"));
}

#[tokio::test]
async fn pre_tool_allow_does_not_bypass_ask_rule() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let rules = PermissionRules::default()
        .ask_tool("test.allow")
        .pre_tool_use_hook(
            "test.allow",
            PreToolUseHookDecision::allow("pre hook tried allow", None),
        );
    let executor = ToolExecutor::new(vec![Arc::new(AllowTool)], rules);

    let execution = executor
        .execute(store.clone(), &run_id, "tool-1", "test.allow", json!({}))
        .await;

    assert!(execution.result.is_error);
    assert_eq!(
        store.get_run(&run_id).await.unwrap().status,
        AgentRunStatus::NeedsUserInput
    );
    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].decision, "ask");
    assert!(audits[0].reason.contains("ask test.allow"));
}

#[tokio::test]
async fn pre_tool_updated_input_is_used_for_permission_execution_and_audit() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let rules = PermissionRules::default().pre_tool_use_hook(
        "test.echo_path_allow",
        PreToolUseHookDecision::allow(
            "pre hook scoped path",
            Some(json!({ "path": "project/approved-by-pre-hook.txt" })),
        ),
    );
    let executor = ToolExecutor::new(vec![Arc::new(EchoPathAllowTool)], rules);

    let execution = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-1",
            "test.echo_path_allow",
            json!({ "path": "project/original.txt" }),
        )
        .await;

    assert!(!execution.result.is_error);
    assert_eq!(
        execution.result.content["path"],
        "project/approved-by-pre-hook.txt"
    );
    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].decision, "allow");
    assert!(audits[0].reason.contains("PreToolUse"));
    assert!(audits[0].input_summary.contains("approved-by-pre-hook"));
    assert!(!audits[0].input_summary.contains("original.txt"));
}

#[test]
fn command_policy_denies_shell_wrappers_and_install_bypass() {
    assert!(matches!(
        check_command_policy(&["sh".to_string(), "-c".to_string(), "pnpm build".to_string()]),
        PermissionResult::Deny { .. }
    ));
    assert!(matches!(
        check_command_policy(&["kubectl".to_string(), "get".to_string(), "pods".to_string()]),
        PermissionResult::Deny { .. }
    ));
    match check_command_policy(&["pnpm".to_string(), "install".to_string()]) {
        PermissionResult::Deny { message, .. } => {
            assert!(message.contains("package.install"));
            assert!(message.contains("mode"));
        }
        other => panic!("expected pnpm install deny, got {other:?}"),
    }
    match check_command_policy(&["yarn".to_string(), "add".to_string(), "astro".to_string()]) {
        PermissionResult::Deny { message, .. } => {
            assert!(message.contains("package.install"));
        }
        other => panic!("expected yarn add deny, got {other:?}"),
    }
    match check_command_policy(&["bun".to_string(), "install".to_string()]) {
        PermissionResult::Deny { message, .. } => {
            assert!(message.contains("package.install"));
        }
        other => panic!("expected bun install deny, got {other:?}"),
    }
    assert!(matches!(
        check_command_policy(&[
            "npm".to_string(),
            "create".to_string(),
            "astro@latest".to_string()
        ]),
        PermissionResult::Deny { .. }
    ));
    assert!(matches!(
        check_command_policy(&["npx".to_string(), "serve".to_string(), "dist".to_string()]),
        PermissionResult::Deny { .. }
    ));
    assert!(matches!(
        check_command_policy(&["npm".to_string(), "run".to_string(), "preview".to_string()]),
        PermissionResult::Deny { .. }
    ));
    assert!(matches!(
        check_command_policy(&["pnpm".to_string(), "build".to_string()]),
        PermissionResult::Allow { .. }
    ));
}

#[test]
fn path_policy_blocks_external_and_secret_paths() {
    let workspace = unique_temp_dir("permission-workspace");
    fs::create_dir_all(workspace.join("project")).unwrap();
    fs::write(workspace.join("project").join("index.md"), "ok").unwrap();
    fs::write(workspace.join(".env"), "secret").unwrap();

    let allowed = check_existing_path(&workspace.join("project").join("index.md"), &workspace);
    assert!(allowed.is_ok());
    let secret = check_existing_path(&workspace.join(".env"), &workspace);
    assert!(secret.is_err());
    let external = check_existing_path(PathBuf::from("/etc/passwd").as_path(), &workspace);
    assert!(external.is_err());

    let created = check_create_path(&workspace.join("project").join("new.md"), &workspace);
    assert!(created.is_ok());
    let nested = check_create_path(
        &workspace.join("inputs").join("generated").join("brief.md"),
        &workspace,
    );
    assert!(nested.is_ok());
    let traversal = check_create_path(
        &workspace.join("project").join("..").join("x.md"),
        &workspace,
    );
    assert!(traversal.is_err());
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
