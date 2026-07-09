use anydesign_runtime::{
    conversation::RuntimeStore,
    sandbox_adapter::{parse_claim_phase_from_json, SandboxClaimManifest, SandboxClaimPhase},
    tools::{
        runtime::ToolContext,
        sandbox::{
            JsonWorkspaceChannelBackend, JsonWorkspaceChannelCommandBackend, SandboxCommandBackend,
            WebSocketWorkspaceChannelTransport, WorkspaceBackend,
        },
    },
    types::AgentPhase,
};
use std::{
    fs,
    net::TcpListener as StdTcpListener,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{net::TcpStream, process::Command, time};

#[tokio::test]
async fn k8s_sandbox_claim_workspace_channel_smoke() {
    if std::env::var("RUN_AGENT_SANDBOX_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping k8s sandbox E2E; set RUN_AGENT_SANDBOX_E2E=1 to enable");
        return;
    }

    let kubectl = std::env::var("KUBECTL").unwrap_or_else(|_| "kubectl".to_string());
    let namespace =
        std::env::var("ANYDESIGN_E2E_NAMESPACE").unwrap_or_else(|_| "anydesign-sandboxes".into());
    let warm_pool = std::env::var("ANYDESIGN_E2E_WARM_POOL")
        .unwrap_or_else(|_| "anydesign-astro-website-pool".into());
    let claim_name = std::env::var("ANYDESIGN_E2E_CLAIM").unwrap_or_else(|_| {
        format!(
            "anydesign-e2e-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
        )
    });
    if std::env::var("ANYDESIGN_E2E_SKIP_APPLY").ok().as_deref() != Some("1") {
        kubectl_apply(
            &kubectl,
            "infra/agent-sandbox/rbac/runtime-service-account.yaml",
        )
        .await;
        kubectl_apply(&kubectl, "infra/agent-sandbox/network/default-deny.yaml").await;
        kubectl_apply(
            &kubectl,
            "infra/agent-sandbox/astro-website/sandbox-template.yaml",
        )
        .await;
        kubectl_apply(
            &kubectl,
            "infra/agent-sandbox/astro-website/sandbox-warm-pool.yaml",
        )
        .await;
    }

    let manifest = SandboxClaimManifest::new(claim_name.clone(), namespace.clone(), warm_pool);
    kubectl_stdin(&kubectl, &["apply", "-f", "-"], &manifest.to_yaml()).await;

    let cleanup_kubectl = kubectl.clone();
    let cleanup_namespace = namespace.clone();
    let cleanup_claim = claim_name.clone();
    let cleanup = Cleanup::new(move || {
        std::process::Command::new(&cleanup_kubectl)
            .args([
                "delete",
                "sandboxclaim",
                &cleanup_claim,
                "-n",
                &cleanup_namespace,
                "--ignore-not-found=true",
            ])
            .status()
            .ok();
    });

    wait_for_claim_ready(&kubectl, &namespace, &claim_name, Duration::from_secs(120)).await;
    let sandbox_name = resolve_channel_pod_name(&kubectl, &namespace, &claim_name).await;

    let local_port = free_tcp_port();
    let mut port_forward = Command::new(&kubectl)
        .args([
            "-n",
            &namespace,
            "port-forward",
            &format!("pod/{sandbox_name}"),
            &format!("{local_port}:3001"),
        ])
        .spawn()
        .expect("failed to start kubectl port-forward");
    wait_for_tcp_port(local_port).await;

    let workspace = unique_temp_dir("k8s-sandbox-e2e");
    fs::create_dir_all(workspace.join("project")).unwrap();
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-e2e".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let ctx = ToolContext::new(store, run, workspace.clone());
    let endpoint = format!("ws://127.0.0.1:{local_port}/workspace");
    let workspace_backend = JsonWorkspaceChannelBackend::new(
        WebSocketWorkspaceChannelTransport::new(endpoint.clone())
            .with_timeout(Duration::from_secs(5)),
        &workspace,
    );
    let command_backend = JsonWorkspaceChannelCommandBackend::new(
        WebSocketWorkspaceChannelTransport::new(endpoint).with_timeout(Duration::from_secs(5)),
        &workspace,
    );

    let e2e_path = workspace.join("project/e2e.txt");
    workspace_backend
        .write_string(&ctx, &e2e_path, "hello k8s sandbox")
        .await
        .expect("fs.write over sandbox channel");
    let text = workspace_backend
        .read_to_string(&ctx, &e2e_path)
        .await
        .expect("fs.read over sandbox channel");
    assert_eq!(text, "hello k8s sandbox");

    let output = command_backend
        .run(
            &ctx,
            &[
                "node".to_string(),
                "-e".to_string(),
                "process.stdout.write('sandbox command ok')".to_string(),
            ],
            &workspace.join("project"),
            5_000,
        )
        .await
        .expect("process.exec over sandbox channel");
    assert!(output.success, "command stderr: {}", output.stderr);
    assert_eq!(output.stdout, "sandbox command ok");

    let mkdir_output = command_backend
        .run(
            &ctx,
            &[
                "node".to_string(),
                "-e".to_string(),
                "const fs=require('fs'); fs.mkdirSync('src',{recursive:true}); fs.mkdirSync('node_modules/ignored',{recursive:true}); fs.mkdirSync('../outputs/build/source-snapshots',{recursive:true});".to_string(),
            ],
            &workspace.join("project"),
            5_000,
        )
        .await
        .expect("create copyDir fixture directories over sandbox channel");
    assert!(
        mkdir_output.success,
        "mkdir stderr: {}",
        mkdir_output.stderr
    );

    workspace_backend
        .write_string(&ctx, &workspace.join("project/src/index.md"), "copy me")
        .await
        .expect("write source file over sandbox channel");
    workspace_backend
        .write_string(
            &ctx,
            &workspace.join("project/node_modules/ignored/index.js"),
            "ignored",
        )
        .await
        .expect("write skipped dependency over sandbox channel");
    let snapshot_root = workspace.join("outputs/build/source-snapshots/k8s-copy");
    workspace_backend
        .copy_dir_all(
            &ctx,
            &workspace.join("project"),
            &snapshot_root,
            &["node_modules".to_string()],
        )
        .await
        .expect("fs.copyDir over sandbox channel");
    let copied = workspace_backend
        .read_to_string(&ctx, &snapshot_root.join("src/index.md"))
        .await
        .expect("read copied snapshot source over sandbox channel");
    assert_eq!(copied, "copy me");
    let skipped = workspace_backend
        .read_to_string(&ctx, &snapshot_root.join("node_modules/ignored/index.js"))
        .await;
    assert!(
        skipped.is_err(),
        "copy_dir_all must skip node_modules in sandbox snapshot"
    );

    port_forward.kill().await.ok();
    drop(cleanup);
}

async fn kubectl_apply(kubectl: &str, path: &str) {
    kubectl_args(kubectl, &["apply", "-f", path]).await;
}

async fn kubectl_args(kubectl: &str, args: &[&str]) {
    let output = Command::new(kubectl)
        .args(args)
        .output()
        .await
        .unwrap_or_else(|error| panic!("failed to start {kubectl} {args:?}: {error}"));
    assert!(
        output.status.success(),
        "{kubectl} {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn kubectl_stdin(kubectl: &str, args: &[&str], stdin: &str) {
    use tokio::io::AsyncWriteExt;

    let mut child = Command::new(kubectl)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("failed to start {kubectl} {args:?}: {error}"));
    child
        .stdin
        .take()
        .expect("kubectl stdin")
        .write_all(stdin.as_bytes())
        .await
        .expect("write kubectl stdin");
    let output = child.wait_with_output().await.expect("kubectl output");
    assert!(
        output.status.success(),
        "{kubectl} {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn wait_for_claim_ready(kubectl: &str, namespace: &str, claim_name: &str, timeout: Duration) {
    let deadline = time::Instant::now() + timeout;
    loop {
        let output = Command::new(kubectl)
            .args([
                "get",
                "sandboxclaim",
                claim_name,
                "-n",
                namespace,
                "-o",
                "json",
            ])
            .output()
            .await
            .expect("kubectl get sandboxclaim");
        assert!(
            output.status.success(),
            "kubectl get sandboxclaim failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let phase = parse_claim_phase_from_json(&String::from_utf8_lossy(&output.stdout))
            .expect("parse sandboxclaim status");
        if phase == SandboxClaimPhase::Ready {
            return;
        }
        assert!(
            !matches!(
                phase,
                SandboxClaimPhase::Failed | SandboxClaimPhase::Deleted
            ),
            "SandboxClaim entered terminal phase: {phase:?}"
        );
        assert!(
            time::Instant::now() < deadline,
            "SandboxClaim {claim_name} did not become Ready before timeout; last phase={phase:?}"
        );
        time::sleep(Duration::from_secs(2)).await;
    }
}

async fn resolve_channel_pod_name(kubectl: &str, namespace: &str, claim_name: &str) -> String {
    if let Ok(pod_name) = std::env::var("ANYDESIGN_E2E_SANDBOX_POD") {
        return pod_name;
    }

    sandbox_name_for_claim(kubectl, namespace, claim_name)
        .await
        .unwrap_or_else(|| claim_name.to_string())
}

async fn sandbox_name_for_claim(
    kubectl: &str,
    namespace: &str,
    claim_name: &str,
) -> Option<String> {
    let output = Command::new(kubectl)
        .args([
            "get",
            "sandboxclaim",
            claim_name,
            "-n",
            namespace,
            "-o",
            "json",
        ])
        .output()
        .await
        .expect("kubectl get sandboxclaim");
    assert!(
        output.status.success(),
        "kubectl get sandboxclaim failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    anydesign_runtime::sandbox_adapter::parse_claim_sandbox_name_from_json(
        &String::from_utf8_lossy(&output.stdout),
    )
    .expect("parse sandboxclaim sandbox name")
}

fn free_tcp_port() -> u16 {
    let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

async fn wait_for_tcp_port(port: u16) {
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        time::sleep(Duration::from_millis(50)).await;
    }
    panic!("port-forward did not listen on port {port}");
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "{prefix}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

struct Cleanup<F: FnOnce()> {
    cleanup: Option<F>,
}

impl<F: FnOnce()> Cleanup<F> {
    fn new(cleanup: F) -> Self {
        Self {
            cleanup: Some(cleanup),
        }
    }
}

impl<F: FnOnce()> Drop for Cleanup<F> {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
    }
}
