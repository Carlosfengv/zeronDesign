use anydesign_runtime::{
    conversation::RuntimeStore,
    sandbox_adapter::{parse_claim_phase_from_json, SandboxClaimManifest, SandboxClaimPhase},
    tools::{
        runtime::ToolContext,
        sandbox::{
            JsonWorkspaceChannelBackend, JsonWorkspaceChannelCommandBackend, SandboxCommandBackend,
            WebSocketWorkspaceChannelTransport, WorkspaceBackend, WorkspaceChannelClientTls,
        },
    },
    types::AgentPhase,
    workspace_auth::{WorkspaceChannelClaims, WorkspaceChannelJwtIssuer},
    RuntimeConfig,
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
        std::env::var("ANYDESIGN_E2E_NAMESPACE").unwrap_or_else(|_| "ws-runtime-rc".into());
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
    let second_claim_name = format!("{claim_name}-parallel");
    if std::env::var("ANYDESIGN_E2E_SKIP_APPLY").ok().as_deref() != Some("1") {
        kubectl_apply_in_namespace(
            &kubectl,
            "infra/agent-sandbox/rbac/runtime-service-account.yaml",
            &namespace,
        )
        .await;
        kubectl_apply_in_namespace(
            &kubectl,
            "infra/agent-sandbox/network/default-deny.yaml",
            &namespace,
        )
        .await;
        kubectl_apply_in_namespace(
            &kubectl,
            "infra/agent-sandbox/astro-website/sandbox-template.yaml",
            &namespace,
        )
        .await;
        kubectl_apply_in_namespace(
            &kubectl,
            "infra/agent-sandbox/astro-website/sandbox-warm-pool.yaml",
            &namespace,
        )
        .await;
    }

    let manifest =
        SandboxClaimManifest::new(claim_name.clone(), namespace.clone(), warm_pool.clone());
    kubectl_stdin(&kubectl, &["apply", "-f", "-"], &manifest.to_yaml()).await;
    let second_manifest =
        SandboxClaimManifest::new(second_claim_name.clone(), namespace.clone(), warm_pool);
    kubectl_stdin(&kubectl, &["apply", "-f", "-"], &second_manifest.to_yaml()).await;

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
    let second_cleanup_kubectl = kubectl.clone();
    let second_cleanup_namespace = namespace.clone();
    let cleanup_second_claim = second_claim_name.clone();
    let second_cleanup = Cleanup::new(move || {
        std::process::Command::new(&second_cleanup_kubectl)
            .args([
                "delete",
                "sandboxclaim",
                &cleanup_second_claim,
                "-n",
                &second_cleanup_namespace,
                "--ignore-not-found=true",
            ])
            .status()
            .ok();
    });

    tokio::join!(
        wait_for_claim_ready(&kubectl, &namespace, &claim_name, Duration::from_secs(120)),
        wait_for_claim_ready(
            &kubectl,
            &namespace,
            &second_claim_name,
            Duration::from_secs(120)
        )
    );
    let sandbox_name = resolve_channel_pod_name(&kubectl, &namespace, &claim_name).await;
    let pod_uid = metadata_uid(&kubectl, &namespace, "pod", &sandbox_name).await;
    let second_sandbox_name =
        resolve_channel_pod_name(&kubectl, &namespace, &second_claim_name).await;
    let second_pod_uid = metadata_uid(&kubectl, &namespace, "pod", &second_sandbox_name).await;
    assert_ne!(sandbox_name, second_sandbox_name);
    assert_ne!(pod_uid, second_pod_uid);

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
    let second_local_port = free_tcp_port();
    let mut second_port_forward = Command::new(&kubectl)
        .args([
            "-n",
            &namespace,
            "port-forward",
            &format!("pod/{second_sandbox_name}"),
            &format!("{second_local_port}:3001"),
        ])
        .spawn()
        .expect("failed to start second kubectl port-forward");
    wait_for_tcp_port(second_local_port).await;

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
    let second_workspace = unique_temp_dir("k8s-sandbox-e2e-parallel");
    fs::create_dir_all(second_workspace.join("project")).unwrap();
    let second_store = RuntimeStore::new();
    let second_run = second_store
        .create_run(
            "project-e2e-parallel".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let second_ctx = ToolContext::new(second_store, second_run, second_workspace.clone());
    let signing_key_file = std::env::var("WORKSPACE_CHANNEL_SIGNING_KEY_FILE")
        .expect("WORKSPACE_CHANNEL_SIGNING_KEY_FILE must be set by run-k8s-e2e.sh");
    let issuer = WorkspaceChannelJwtIssuer::from_pkcs8_der_file(signing_key_file, 60)
        .expect("load workspace channel signing key");
    let tls = WorkspaceChannelClientTls::from_runtime_config(&RuntimeConfig::from_env())
        .expect("load workspace channel mTLS config");
    assert!(tls.is_some(), "k8s gate requires mTLS workspace channels");
    let endpoint = format!("wss://127.0.0.1:{local_port}/workspace");
    let authorization = || {
        issuer
            .issue(WorkspaceChannelClaims {
                iss: String::new(),
                aud: String::new(),
                exp: 0,
                iat: 0,
                jti: format!("{claim_name}-e2e-{}", rand::random::<u64>()),
                sandbox_binding_id: claim_name.clone(),
                sandbox_name: sandbox_name.clone(),
                pod_uid: pod_uid.clone(),
                project_id: ctx.run.project_id.clone(),
                run_id: ctx.run.id.clone(),
                operations: vec![
                    "fs.read".to_string(),
                    "fs.write".to_string(),
                    "process.exec".to_string(),
                    "process.manage".to_string(),
                    "archive.export".to_string(),
                ],
            })
            .map(|token| format!("Bearer {token}"))
            .expect("issue workspace channel token")
    };
    let workspace_backend = || {
        JsonWorkspaceChannelBackend::new(
            WebSocketWorkspaceChannelTransport::new(endpoint.clone())
                .with_tls(tls.clone())
                .with_authorization(authorization())
                .with_timeout(Duration::from_secs(5)),
            &workspace,
        )
    };
    let command_backend = || {
        JsonWorkspaceChannelCommandBackend::new(
            WebSocketWorkspaceChannelTransport::new(endpoint.clone())
                .with_tls(tls.clone())
                .with_authorization(authorization())
                .with_timeout(Duration::from_secs(5)),
            &workspace,
        )
    };
    let second_endpoint = format!("wss://127.0.0.1:{second_local_port}/workspace");
    let second_authorization = || {
        issuer
            .issue(WorkspaceChannelClaims {
                iss: String::new(),
                aud: String::new(),
                exp: 0,
                iat: 0,
                jti: format!("{second_claim_name}-e2e-{}", rand::random::<u64>()),
                sandbox_binding_id: second_claim_name.clone(),
                sandbox_name: second_sandbox_name.clone(),
                pod_uid: second_pod_uid.clone(),
                project_id: second_ctx.run.project_id.clone(),
                run_id: second_ctx.run.id.clone(),
                operations: vec!["fs.read".to_string(), "fs.write".to_string()],
            })
            .map(|token| format!("Bearer {token}"))
            .expect("issue second workspace channel token")
    };
    let second_workspace_backend = || {
        JsonWorkspaceChannelBackend::new(
            WebSocketWorkspaceChannelTransport::new(second_endpoint.clone())
                .with_tls(tls.clone())
                .with_authorization(second_authorization())
                .with_timeout(Duration::from_secs(5)),
            &second_workspace,
        )
    };

    let e2e_path = workspace.join("project/e2e.txt");
    workspace_backend()
        .write_string(&ctx, &e2e_path, "hello k8s sandbox")
        .await
        .expect("fs.write over sandbox channel");
    let text = workspace_backend()
        .read_to_string(&ctx, &e2e_path)
        .await
        .expect("fs.read over sandbox channel");
    assert_eq!(text, "hello k8s sandbox");

    let output = command_backend()
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

    let mkdir_output = command_backend()
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

    workspace_backend()
        .write_string(&ctx, &workspace.join("project/src/index.md"), "copy me")
        .await
        .expect("write source file over sandbox channel");
    workspace_backend()
        .write_string(
            &ctx,
            &workspace.join("project/node_modules/ignored/index.js"),
            "ignored",
        )
        .await
        .expect("write skipped dependency over sandbox channel");
    let snapshot_root = workspace.join("outputs/build/source-snapshots/k8s-copy");
    workspace_backend()
        .copy_dir_all(
            &ctx,
            &workspace.join("project"),
            &snapshot_root,
            &["node_modules".to_string()],
        )
        .await
        .expect("fs.copyDir over sandbox channel");
    let copied = workspace_backend()
        .read_to_string(&ctx, &snapshot_root.join("src/index.md"))
        .await
        .expect("read copied snapshot source over sandbox channel");
    assert_eq!(copied, "copy me");
    let skipped = workspace_backend()
        .read_to_string(&ctx, &snapshot_root.join("node_modules/ignored/index.js"))
        .await;
    assert!(
        skipped.is_err(),
        "copy_dir_all must skip node_modules in sandbox snapshot"
    );

    let candidate_setup = command_backend()
        .run(
            &ctx,
            &[
                "node".to_string(),
                "-e".to_string(),
                "const fs=require('fs'); const p='../outputs/candidates/build-k8s-e2e'; fs.mkdirSync(p,{recursive:true}); fs.writeFileSync(p+'/index.html','<!doctype html><h1>managed preview</h1>'); fs.writeFileSync(p+'/.anydesign-candidate-manifest.json','[]');".to_string(),
            ],
            &workspace.join("project"),
            5_000,
        )
        .await
        .expect("create managed preview candidate");
    assert!(
        candidate_setup.success,
        "candidate stderr: {}",
        candidate_setup.stderr
    );
    let process_lease_id = format!("preview-lease-{}", rand::random::<u64>());
    let started = command_backend()
        .start_process(
            &ctx,
            &process_lease_id,
            &[
                "node".to_string(),
                "/opt/anydesign/bootstrap/static-preview-server.js".to_string(),
            ],
            &workspace,
        )
        .await
        .expect("start managed preview process lease");
    assert_eq!(started.status, "running");
    assert!(started.pid.is_some());
    let status = command_backend()
        .process_status(&ctx, &process_lease_id)
        .await
        .expect("read managed preview process lease");
    assert_eq!(status.status, "running");
    let stopped = command_backend()
        .stop_process(&ctx, &process_lease_id)
        .await
        .expect("stop managed preview process lease");
    assert!(matches!(stopped.status.as_str(), "stopped" | "exited"));

    let binary = [0_u8, 255, 1, 128, 42, 10];
    workspace_backend()
        .write_bytes(&ctx, &workspace.join("project/src/binary.bin"), &binary)
        .await
        .expect("write binary export fixture");
    let export_root = unique_temp_dir("k8s-sandbox-export");
    let receipt = workspace_backend()
        .export_tree(
            &ctx,
            &workspace.join("project"),
            &export_root,
            &["node_modules/ignored/index.js".to_string()],
        )
        .await
        .expect("stream archive.export over sandbox channel");
    assert!(receipt.file_count >= 3);
    assert!(!receipt.manifest_hash.is_empty());
    assert_eq!(
        fs::read(export_root.join("src/binary.bin")).unwrap(),
        binary
    );
    assert!(!export_root.join("node_modules/ignored/index.js").exists());

    let isolation_path = workspace.join("project/isolation.txt");
    let second_isolation_path = second_workspace.join("project/isolation.txt");
    workspace_backend()
        .write_string(&ctx, &isolation_path, "project-one")
        .await
        .expect("write first isolated workspace");
    second_workspace_backend()
        .write_string(&second_ctx, &second_isolation_path, "project-two")
        .await
        .expect("write second isolated workspace");
    assert_eq!(
        workspace_backend()
            .read_to_string(&ctx, &isolation_path)
            .await
            .unwrap(),
        "project-one"
    );
    assert_eq!(
        second_workspace_backend()
            .read_to_string(&second_ctx, &second_isolation_path)
            .await
            .unwrap(),
        "project-two"
    );

    if let Ok(evidence_path) = std::env::var("E2E_EVIDENCE_PATH") {
        let evidence = serde_json::json!({
            "schemaVersion": "anydesign-k3d-channel-evidence@1",
            "gate": "k3d-channel-smoke",
            "repository": {
                "commit": std::env::var("E2E_REPOSITORY_COMMIT").unwrap_or_default(),
                "dirtyFiles": std::env::var("E2E_REPOSITORY_DIRTY_FILES").ok().and_then(|value| value.parse::<u64>().ok()),
            },
            "cluster": {
                "name": std::env::var("E2E_K3D_CLUSTER").unwrap_or_default(),
                "kubeContext": format!("k3d-{}", std::env::var("E2E_K3D_CLUSTER").unwrap_or_default()),
                "workspaceNamespace": namespace,
            },
            "sandbox": {
                "imageRef": std::env::var("E2E_SANDBOX_IMAGE").unwrap_or_default(),
                "imageId": std::env::var("E2E_SANDBOX_IMAGE_ID").unwrap_or_default(),
            },
            "claims": [
                { "claim": claim_name, "pod": sandbox_name, "podUid": pod_uid, "projectId": ctx.run.project_id },
                { "claim": second_claim_name, "pod": second_sandbox_name, "podUid": second_pod_uid, "projectId": second_ctx.run.project_id },
            ],
            "checks": {
                "authenticatedWorkspaceChannel": true,
                "granularScopes": true,
                "processLeaseStartStatusStop": true,
                "binaryArchiveExport": true,
                "parallelWorkspaceIsolation": true,
                "claimCleanupRegistered": true,
            }
        });
        let evidence_path = PathBuf::from(evidence_path);
        if let Some(parent) = evidence_path.parent() {
            fs::create_dir_all(parent).expect("create E2E evidence directory");
        }
        fs::write(
            evidence_path,
            serde_json::to_vec_pretty(&evidence).expect("serialize E2E evidence"),
        )
        .expect("write E2E evidence");
    }

    port_forward.kill().await.ok();
    second_port_forward.kill().await.ok();
    drop(cleanup);
    drop(second_cleanup);
}

async fn kubectl_apply_in_namespace(kubectl: &str, path: &str, namespace: &str) {
    let manifest = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read Kubernetes manifest {path}: {error}"))
        .replace("anydesign-sandboxes", namespace);
    kubectl_stdin(kubectl, &["apply", "-f", "-"], &manifest).await;
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

async fn metadata_uid(kubectl: &str, namespace: &str, resource: &str, name: &str) -> String {
    let output = Command::new(kubectl)
        .args(["get", resource, name, "-n", namespace, "-o", "json"])
        .output()
        .await
        .unwrap_or_else(|error| panic!("kubectl get {resource}/{name}: {error}"));
    assert!(
        output.status.success(),
        "kubectl get {resource}/{name} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    anydesign_runtime::sandbox_adapter::parse_metadata_uid(&String::from_utf8_lossy(&output.stdout))
        .expect("parse metadata UID")
        .expect("metadata UID")
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
