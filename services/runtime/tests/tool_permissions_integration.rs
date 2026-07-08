use anydesign_runtime::{
    conversation::RuntimeStore,
    tools::{control_plane::control_plane_executor, runtime::ToolExecutor, sandbox::sandbox_tools},
    types::{AgentPhase, AgentRunStatus},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tokio::{io::AsyncWriteExt, net::TcpListener, task::JoinHandle};

#[cfg(unix)]
use std::os::unix::fs::symlink;

async fn create_run(store: &RuntimeStore) -> String {
    store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await
        .id
}

fn sandbox_executor(workspace: &Path) -> ToolExecutor {
    ToolExecutor::new_with_workspace_root(sandbox_tools(), Default::default(), workspace)
}

#[tokio::test]
async fn control_plane_tool_calls_write_one_audit_record_each() {
    let store = RuntimeStore::new();
    let run_id = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await
        .id;
    let executor = control_plane_executor();

    let content = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-content",
            "content.list_sources",
            json!({}),
        )
        .await;
    let completion = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-complete",
            "run.complete",
            json!({ "status": "partial", "summary": "Stopping with checkpointed progress" }),
        )
        .await;

    assert!(!content.result.is_error);
    assert!(!completion.result.is_error);
    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 2);
    assert_eq!(audits[0].tool, "content.list_sources");
    assert_eq!(audits[0].decision, "allow");
    assert_eq!(audits[1].tool, "run.complete");
    assert_eq!(audits[1].decision, "allow");
}

#[tokio::test]
async fn sandbox_tool_validation_runs_before_permission_but_writes_runtime_audit() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let execution = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-1",
            "shell.run",
            json!({ "argv": [] }),
        )
        .await;

    assert!(execution.result.is_error);
    assert!(execution.result.content["error"]
        .as_str()
        .unwrap()
        .contains("argv"));
    assert!(store
        .events(&run_id)
        .await
        .iter()
        .all(|event| { serde_json::to_value(event).unwrap()["type"] != "permission.requested" }));
    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].tool, "shell.run");
    assert_eq!(audits[0].decision, "deny");
    assert!(audits[0].reason.contains("input validation failed"));
}

#[tokio::test]
async fn denied_secret_read_does_not_leak_contents_to_result_events_or_conversation() {
    let workspace = setup_workspace();
    let secret = "SUPER_SECRET_RUNTIME_VALUE";
    fs::write(workspace.join(".env"), secret).unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let execution = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-1",
            "fs.read",
            json!({ "path": ".env" }),
        )
        .await;

    assert!(execution.result.is_error);
    assert!(!execution.result.content.to_string().contains(secret));
    assert!(!serde_json::to_string(&store.events(&run_id).await)
        .unwrap()
        .contains(secret));
    assert!(
        !serde_json::to_string(&store.conversation_items("project-1").await)
            .unwrap()
            .contains(secret)
    );
    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].decision, "deny");
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_escape_is_denied_for_read_patch_and_delete_after_realpath() {
    let workspace = setup_workspace();
    let outside = unique_temp_dir("sandbox-outside").join("outside.txt");
    fs::write(&outside, "outside secret").unwrap();
    symlink(&outside, workspace.join("project").join("escape.txt")).unwrap();

    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let read = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-read",
            "fs.read",
            json!({ "path": "project/escape.txt" }),
        )
        .await;
    let patch = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-patch",
            "fs.patch",
            json!({
                "path": "project/escape.txt",
                "oldStr": "outside",
                "newStr": "changed"
            }),
        )
        .await;
    let delete = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-delete",
            "fs.delete",
            json!({ "path": "project/escape.txt" }),
        )
        .await;

    assert!(read.result.is_error);
    assert!(patch.result.is_error);
    assert!(delete.result.is_error);
    assert_eq!(fs::read_to_string(&outside).unwrap(), "outside secret");
    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 3);
    assert!(audits.iter().all(|audit| audit.decision == "deny"));
}

#[tokio::test]
async fn shell_and_package_policies_are_enforced_on_real_sandbox_tools() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let sh = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-sh",
            "shell.run",
            json!({ "argv": ["sh", "-c", "pnpm build"], "cwd": "project" }),
        )
        .await;
    let pnpm_install = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-install",
            "shell.run",
            json!({ "argv": ["pnpm", "install"], "cwd": "project" }),
        )
        .await;
    let node = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-node",
            "shell.run",
            json!({ "argv": ["node", "-e", "process.stdout.write('ok')"], "cwd": "project" }),
        )
        .await;
    let public_package = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-package",
            "package.install",
            json!({ "packages": ["left-pad"], "registry": "https://registry.npmjs.org" }),
        )
        .await;

    assert!(sh.result.is_error);
    assert!(sh.result.content["error"]
        .as_str()
        .unwrap()
        .contains("not allowed"));
    assert!(pnpm_install.result.is_error);
    assert!(pnpm_install.result.content["error"]
        .as_str()
        .unwrap()
        .contains("package.install"));
    assert!(!node.result.is_error);
    assert_eq!(node.result.content["stdout"], "ok");
    assert!(public_package.result.is_error);
    assert_eq!(
        store.get_run(&run_id).await.unwrap().status,
        AgentRunStatus::Queued
    );

    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 4);
    assert_eq!(audits[0].decision, "deny");
    assert_eq!(audits[1].decision, "deny");
    assert_eq!(audits[2].decision, "allow");
    assert_eq!(audits[3].decision, "deny");
    assert!(audits[0].input_summary.contains("argv=[sh -c pnpm build]"));
    assert!(audits[3].input_summary.contains("registry.npmjs.org"));
}

#[tokio::test]
async fn package_install_local_file_dependency_cannot_escape_workspace() {
    let workspace = setup_workspace();
    let outside = unique_temp_dir("package-outside");
    fs::write(
        outside.join("package.json"),
        serde_json::to_string_pretty(&json!({
            "name": "@internal/escape",
            "version": "1.0.0"
        }))
        .unwrap(),
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let result = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-package-escape",
            "package.install",
            json!({
                "packages": [format!("file:{}", outside.display())],
                "registry": "https://registry.internal.local"
            }),
        )
        .await;

    assert!(result.result.is_error);
    assert!(!workspace.join("project/node_modules").exists());
    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].decision, "deny");
}

#[tokio::test]
async fn browser_open_denies_public_internet_and_allows_internal_preview_urls() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let public = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-public-browser",
            "browser.open",
            json!({ "url": "https://example.com" }),
        )
        .await;
    let internal = executor
        .execute(
            store.clone(),
            &run_id,
            "tool-internal-browser",
            "browser.open",
            json!({ "url": "http://preview.local/preview/project-1/current" }),
        )
        .await;

    assert!(public.result.is_error);
    assert!(public.result.content["error"]
        .as_str()
        .unwrap()
        .contains("public internet"));
    assert!(!internal.result.is_error);
    assert_eq!(
        internal.result.content["url"],
        "http://preview.local/preview/project-1/current"
    );

    let browser_state = fs::read_to_string(workspace.join("state/browser.json")).unwrap();
    assert!(browser_state.contains("preview.local"));
    assert!(!browser_state.contains("example.com"));

    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 2);
    assert_eq!(audits[0].decision, "deny");
    assert_eq!(audits[1].decision, "allow");
    assert!(audits[0].reason.contains("public internet egress denied"));
}

#[tokio::test]
async fn each_sandbox_tool_decision_writes_one_audit_record() {
    let workspace = setup_workspace();
    let (preview_url, _preview_server) = start_preview_server().await;
    let local_package = workspace.join("local-package");
    fs::create_dir_all(&local_package).unwrap();
    fs::write(
        local_package.join("package.json"),
        serde_json::to_string_pretty(&json!({
            "name": "@internal/audit",
            "version": "1.0.0",
            "main": "index.js"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        local_package.join("index.js"),
        "export const audit = true;\n",
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let calls = vec![
        ("fs.list", json!({ "path": "project" })),
        ("fs.search", json!({ "path": "project", "query": "hello" })),
        ("fs.write", json!({ "path": "project/a.txt", "text": "a" })),
        ("fs.read", json!({ "path": "project/a.txt" })),
        (
            "fs.patch",
            json!({ "path": "project/a.txt", "oldStr": "a", "newStr": "b" }),
        ),
        ("fs.delete", json!({ "path": "project/a.txt" })),
        ("preview.rebuilding", json!({})),
        ("preview.start", json!({ "url": preview_url })),
        ("preview.status", json!({})),
        ("preview.stop", json!({})),
        (
            "package.install",
            json!({ "packages": ["file:../local-package"], "registry": "https://registry.internal.local" }),
        ),
        ("diagnostics.build_log", json!({})),
        ("diagnostics.typescript", json!({})),
        ("browser.open", json!({ "url": "http://127.0.0.1:4321" })),
        ("browser.screenshot", json!({ "screenshotId": "shot-a" })),
        ("browser.inspect", json!({})),
    ];

    for (index, (tool, input)) in calls.iter().enumerate() {
        let execution = executor
            .execute(
                store.clone(),
                &run_id,
                &format!("tool-{index}"),
                tool,
                input.clone(),
            )
            .await;
        assert!(
            !execution.result.is_error,
            "{tool} failed: {}",
            error_text(&execution.result.content)
        );
    }

    let audits = store.audit_records().await;
    assert_eq!(audits.len(), calls.len());
    for (audit, (tool, _)) in audits.iter().zip(calls.iter()) {
        assert_eq!(&audit.tool, tool);
        assert_eq!(audit.project_id, "project-1");
        assert_eq!(audit.run_id, run_id);
        assert!(!audit.reason.is_empty());
    }
}

fn setup_workspace() -> PathBuf {
    let workspace = unique_temp_dir("tool-permissions");
    fs::create_dir_all(workspace.join("project")).unwrap();
    fs::create_dir_all(workspace.join("inputs")).unwrap();
    fs::create_dir_all(workspace.join("state")).unwrap();
    fs::create_dir_all(workspace.join("outputs/build")).unwrap();
    fs::create_dir_all(workspace.join("outputs/reports")).unwrap();
    fs::create_dir_all(workspace.join("outputs/screenshots")).unwrap();
    fs::write(workspace.join("project").join("index.md"), "hello").unwrap();
    fs::write(workspace.join("outputs/build/build.log"), "Build ok").unwrap();
    fs::write(
        workspace.join("outputs/build/latest.json"),
        json!({
            "status": "success",
            "success": true,
            "cwd": "/workspace/project",
            "argv": ["npm", "run", "build"],
            "logPath": "/workspace/outputs/build/build.log"
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        workspace.join("outputs/reports/typescript.json"),
        json!({ "ok": true, "diagnostics": [] }).to_string(),
    )
    .unwrap();
    workspace
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

fn error_text(value: &Value) -> String {
    value
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

async fn start_preview_server() -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await;
        }
    });
    (format!("http://{}", addr), handle)
}
