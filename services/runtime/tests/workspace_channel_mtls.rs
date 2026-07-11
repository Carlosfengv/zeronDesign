use anydesign_runtime::{
    config::{SandboxBackendMode, WorkspaceChannelTlsMode},
    tools::sandbox::{
        WebSocketWorkspaceChannelTransport, WorkspaceChannelClientTls, WorkspaceChannelRequest,
        WorkspaceChannelTransport,
    },
    workspace_auth::{WorkspaceChannelClaims, WorkspaceChannelJwtIssuer},
    RuntimeConfig,
};
use ed25519_dalek::{pkcs8::EncodePublicKey, SigningKey};
use serde_json::json;
use std::{
    fs,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

const RUNTIME_SAN: &str = "spiffe://anydesign.local/ns/anydesign-runtime/sa/anydesign-runtime";
const SANDBOX_SAN: &str = "spiffe://anydesign.local/ns/anydesign-sandboxes/sa/anydesign-sandbox";

struct ServerProcess(Child);

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn mtls_and_pod_bound_jwt_are_both_required_for_workspace_read() {
    let root = std::env::temp_dir().join(format!(
        "anydesign-workspace-mtls-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("workspace/project")).unwrap();
    fs::write(
        root.join("workspace/project/hello.txt"),
        "mTLS channel ready",
    )
    .unwrap();
    generate_certificates(&root);

    let signing_key = SigningKey::from_bytes(&[41_u8; 32]);
    let public_key = signing_key.verifying_key().to_public_key_der().unwrap();
    let public_key_path = root.join("workspace-jwt-public.der");
    fs::write(&public_key_path, public_key.as_bytes()).unwrap();
    let port = free_port();
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../infra/agent-sandbox/base/workspace-channel-server.js");
    let child = Command::new("node")
        .arg(script)
        .env("WORKSPACE_ROOT", root.join("workspace"))
        .env("WORKSPACE_CHANNEL_HOST", "127.0.0.1")
        .env("WORKSPACE_CHANNEL_PORT", port.to_string())
        .env("WORKSPACE_CHANNEL_TLS_MODE", "required")
        .env("WORKSPACE_CHANNEL_CA_FILE", root.join("ca-bundle.crt"))
        .env("WORKSPACE_CHANNEL_CERT_FILE", root.join("server.crt"))
        .env("WORKSPACE_CHANNEL_KEY_FILE", root.join("server.key"))
        .env("WORKSPACE_CHANNEL_RUNTIME_SAN", RUNTIME_SAN)
        .env("WORKSPACE_CHANNEL_AUTH_MODE", "required")
        .env("WORKSPACE_CHANNEL_PUBLIC_KEY_FILE", &public_key_path)
        .env("POD_NAME", "sandbox-mtls")
        .env("POD_UID", "pod-uid-mtls")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _server = ServerProcess(child);
    wait_for_port(port).await;

    let endpoint = format!("wss://127.0.0.1:{port}/workspace");
    let tls = client_tls(&root, root.join("ca.crt"), SANDBOX_SAN).unwrap();
    let request = WorkspaceChannelRequest {
        op: "fs.read",
        path: "/workspace/project/hello.txt".to_string(),
        payload: json!({}),
    };

    let no_client_cert = WebSocketWorkspaceChannelTransport::new(endpoint.clone())
        .with_timeout(Duration::from_secs(2))
        .request(request.clone())
        .await;
    assert!(no_client_cert.is_err());

    let wrong_ca = client_tls(&root, root.join("wrong-ca.crt"), SANDBOX_SAN).unwrap();
    let wrong_ca_result = WebSocketWorkspaceChannelTransport::new(endpoint.clone())
        .with_tls(wrong_ca)
        .with_timeout(Duration::from_secs(2))
        .request(request.clone())
        .await;
    assert!(wrong_ca_result.is_err());

    let no_jwt = WebSocketWorkspaceChannelTransport::new(endpoint.clone())
        .with_tls(tls.clone())
        .with_timeout(Duration::from_secs(2))
        .request(request.clone())
        .await;
    assert!(no_jwt.is_err());

    let issuer = WorkspaceChannelJwtIssuer::from_signing_key(signing_key, 60);
    let wrong_pod = issue_token(&issuer, "wrong-pod-uid");
    let wrong_pod_result = WebSocketWorkspaceChannelTransport::new(endpoint.clone())
        .with_tls(tls.clone())
        .with_authorization(format!("Bearer {wrong_pod}"))
        .with_timeout(Duration::from_secs(2))
        .request(request.clone())
        .await;
    assert!(wrong_pod_result.is_err());

    let wrong_san_tls = client_tls(
        &root,
        root.join("ca.crt"),
        "spiffe://anydesign.local/ns/other/sa/other",
    )
    .unwrap();
    let wrong_san_token = issue_token(&issuer, "pod-uid-mtls");
    let wrong_san = WebSocketWorkspaceChannelTransport::new(endpoint.clone())
        .with_tls(wrong_san_tls)
        .with_authorization(format!("Bearer {wrong_san_token}"))
        .with_timeout(Duration::from_secs(2))
        .request(request.clone())
        .await;
    assert_eq!(
        wrong_san.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );

    let token = issue_token(&issuer, "pod-uid-mtls");
    let response = WebSocketWorkspaceChannelTransport::new(endpoint)
        .with_tls(tls)
        .with_authorization(format!("Bearer {token}"))
        .with_timeout(Duration::from_secs(2))
        .request(request.clone())
        .await
        .unwrap();
    assert_eq!(response["text"], "mTLS channel ready");

    let previous_client_tls = client_tls_with_identity(
        &root,
        root.join("ca.crt"),
        root.join("previous-client.crt"),
        root.join("previous-client.key"),
        SANDBOX_SAN,
    )
    .unwrap();
    let previous_client_token = issue_token(&issuer, "pod-uid-mtls");
    let previous_client_response =
        WebSocketWorkspaceChannelTransport::new(format!("wss://127.0.0.1:{port}/workspace"))
            .with_tls(previous_client_tls)
            .with_authorization(format!("Bearer {previous_client_token}"))
            .with_timeout(Duration::from_secs(2))
            .request(request.clone())
            .await
            .unwrap();
    assert_eq!(previous_client_response["text"], "mTLS channel ready");

    let previous_server_port = free_port();
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../infra/agent-sandbox/base/workspace-channel-server.js");
    let previous_server = Command::new("node")
        .arg(script)
        .env("WORKSPACE_ROOT", root.join("workspace"))
        .env("WORKSPACE_CHANNEL_HOST", "127.0.0.1")
        .env("WORKSPACE_CHANNEL_PORT", previous_server_port.to_string())
        .env("WORKSPACE_CHANNEL_TLS_MODE", "required")
        .env("WORKSPACE_CHANNEL_CA_FILE", root.join("ca-bundle.crt"))
        .env(
            "WORKSPACE_CHANNEL_CERT_FILE",
            root.join("previous-server.crt"),
        )
        .env(
            "WORKSPACE_CHANNEL_KEY_FILE",
            root.join("previous-server.key"),
        )
        .env("WORKSPACE_CHANNEL_RUNTIME_SAN", RUNTIME_SAN)
        .env("WORKSPACE_CHANNEL_AUTH_MODE", "required")
        .env("WORKSPACE_CHANNEL_PUBLIC_KEY_FILE", &public_key_path)
        .env("POD_NAME", "sandbox-mtls")
        .env("POD_UID", "pod-uid-mtls")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _previous_server = ServerProcess(previous_server);
    wait_for_port(previous_server_port).await;
    let trust_bundle = client_tls(&root, root.join("ca-bundle.crt"), SANDBOX_SAN).unwrap();
    let previous_server_token = issue_token(&issuer, "pod-uid-mtls");
    let previous_server_response = WebSocketWorkspaceChannelTransport::new(format!(
        "wss://127.0.0.1:{previous_server_port}/workspace"
    ))
    .with_tls(trust_bundle)
    .with_authorization(format!("Bearer {previous_server_token}"))
    .with_timeout(Duration::from_secs(2))
    .request(request)
    .await
    .unwrap();
    assert_eq!(previous_server_response["text"], "mTLS channel ready");
}

fn client_tls(
    root: &Path,
    ca_file: PathBuf,
    expected_server_san: &str,
) -> std::io::Result<Option<WorkspaceChannelClientTls>> {
    client_tls_with_identity(
        root,
        ca_file,
        root.join("client.crt"),
        root.join("client.key"),
        expected_server_san,
    )
}

fn client_tls_with_identity(
    _root: &Path,
    ca_file: PathBuf,
    cert_file: PathBuf,
    key_file: PathBuf,
    expected_server_san: &str,
) -> std::io::Result<Option<WorkspaceChannelClientTls>> {
    let mut config = RuntimeConfig::from_env();
    config.sandbox_backend_mode = SandboxBackendMode::Kubernetes;
    config.workspace_channel_tls_mode = WorkspaceChannelTlsMode::Required;
    config.workspace_channel_ca_file = Some(ca_file);
    config.workspace_channel_client_cert_file = Some(cert_file);
    config.workspace_channel_client_key_file = Some(key_file);
    config.workspace_channel_server_san = expected_server_san.to_string();
    WorkspaceChannelClientTls::from_runtime_config(&config)
}

fn issue_token(issuer: &WorkspaceChannelJwtIssuer, pod_uid: &str) -> String {
    issuer
        .issue(WorkspaceChannelClaims {
            iss: String::new(),
            aud: String::new(),
            exp: 0,
            iat: 0,
            jti: format!("mtls-jti-{pod_uid}-{:016x}", rand::random::<u64>()),
            sandbox_binding_id: "binding-mtls".to_string(),
            sandbox_name: "sandbox-mtls".to_string(),
            pod_uid: pod_uid.to_string(),
            project_id: "project-mtls".to_string(),
            run_id: "run-mtls".to_string(),
            operations: vec!["fs.read".to_string()],
        })
        .unwrap()
}

fn generate_certificates(root: &Path) {
    run_openssl(
        root,
        &[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "2",
            "-subj",
            "/CN=mtls-test-ca",
            "-keyout",
            "ca.key",
            "-out",
            "ca.crt",
        ],
    );
    run_openssl(
        root,
        &[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "2",
            "-subj",
            "/CN=wrong-ca",
            "-keyout",
            "wrong-ca.key",
            "-out",
            "wrong-ca.crt",
        ],
    );
    fs::write(root.join("client.ext"), format!("basicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=clientAuth\nsubjectAltName=URI:{RUNTIME_SAN}\n")).unwrap();
    fs::write(root.join("server.ext"), format!("basicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1,URI:{SANDBOX_SAN}\n")).unwrap();
    sign_leaf(root, "client", "client.ext");
    sign_leaf(root, "server", "server.ext");
    sign_leaf_with_ca(root, "previous-client", "client.ext", "wrong-ca");
    sign_leaf_with_ca(root, "previous-server", "server.ext", "wrong-ca");
    let mut bundle = fs::read(root.join("ca.crt")).unwrap();
    bundle.extend_from_slice(&fs::read(root.join("wrong-ca.crt")).unwrap());
    fs::write(root.join("ca-bundle.crt"), bundle).unwrap();
}

fn sign_leaf(root: &Path, name: &str, extension: &str) {
    sign_leaf_with_ca(root, name, extension, "ca");
}

fn sign_leaf_with_ca(root: &Path, name: &str, extension: &str, ca: &str) {
    run_openssl(
        root,
        &[
            "req",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-subj",
            &format!("/CN={name}"),
            "-keyout",
            &format!("{name}.key"),
            "-out",
            &format!("{name}.csr"),
        ],
    );
    run_openssl(
        root,
        &[
            "x509",
            "-req",
            "-sha256",
            "-days",
            "2",
            "-in",
            &format!("{name}.csr"),
            "-CA",
            &format!("{ca}.crt"),
            "-CAkey",
            &format!("{ca}.key"),
            "-CAcreateserial",
            "-extfile",
            extension,
            "-out",
            &format!("{name}.crt"),
        ],
    );
}

fn run_openssl(root: &Path, args: &[&str]) {
    let status = Command::new("openssl")
        .args(args)
        .current_dir(root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "openssl failed: {args:?}");
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn wait_for_port(port: u16) {
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("workspace channel TLS server did not start");
}
