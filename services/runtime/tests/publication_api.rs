use anydesign_runtime::{
    config::{PublicPrincipalAuthMode, RuntimePolicyProfile, SandboxBackendMode},
    http_api,
    public_principal::{
        PublicPrincipalClaims, PublicPrincipalJwtIssuer, PUBLICATION_WRITE_OPERATION,
    },
    publication::{PublishOperationStatus, WorkRuntimeStatus},
    release::{PackagingScanEvidence, ReleasePackagingInput, RuntimeProfile, WorkRelease},
    types::{sha256_hex, AgentPhase},
    RuntimeConfig,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use ed25519_dalek::{pkcs8::EncodePublicKey, SigningKey};
use serde_json::{json, Value};
use std::path::PathBuf;
use tower::ServiceExt;

fn root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "publication-api-{name}-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ))
}

fn config(root: &std::path::Path) -> RuntimeConfig {
    let mut config = RuntimeConfig::from_env();
    config.sandbox_backend_mode = SandboxBackendMode::PhaseAContract;
    config.policy_profile = RuntimePolicyProfile::LocalE2e;
    config.public_principal_auth_mode = PublicPrincipalAuthMode::Disabled;
    config.runtime_storage_dir = root.join("runtime");
    config.workspace_root = root.join("workspaces");
    config
}

fn digest(character: char) -> String {
    format!("sha256:{}", character.to_string().repeat(64))
}

async fn seed_validated_release(state: &http_api::AppState) -> WorkRelease {
    let run = state
        .store
        .create_run(
            "publication-project".into(),
            AgentPhase::Build,
            "build".into(),
            "fixture".into(),
            vec![],
        )
        .await;
    let version = state
        .store
        .create_project_version_candidate(
            &run.project_id,
            &run.id,
            "http://preview.invalid".into(),
            None,
            Some("fixture://source".into()),
        )
        .await;
    state
        .store
        .promote_project_version(&run.project_id, &run.id, &version.id)
        .await
        .unwrap();
    let profile = RuntimeProfile::static_web_v1(digest('b'), "packager@1", "scan@1").unwrap();
    let release_store = state.store.release_store();
    let (_, packaging) = release_store
        .prepare(&ReleasePackagingInput {
            project_id: run.project_id,
            version_id: version.id,
            run_id: run.id,
            template_id: "generic".into(),
            template_version: "1".into(),
            artifact_manifest_hash: sha256_hex(b"artifact"),
            runtime_manifest_hash: profile.manifest.sha256().unwrap(),
            source_snapshot_uri: "fixture://source".into(),
            runtime_profile_id: profile.id,
            base_image_digest: profile.base_image_digest,
            packager_version: profile.packager_version,
            registry_repository: "registry.example/works".into(),
            scan_policy_version: profile.scan_policy_version,
        })
        .unwrap();
    release_store.begin_build(&packaging.id).unwrap();
    release_store
        .record_built(&packaging.id, &digest('d'))
        .unwrap();
    release_store
        .record_pushed(&packaging.id, &digest('d'))
        .unwrap();
    release_store.begin_scan(&packaging.id).unwrap();
    release_store
        .record_scan(
            &packaging.id,
            &digest('1'),
            &digest('2'),
            PackagingScanEvidence {
                policy_version: "scan@1".into(),
                passed: true,
                critical_vulnerabilities: 0,
                high_vulnerabilities: 0,
                secret_findings: 0,
                report_digest: digest('3'),
            },
        )
        .unwrap();
    release_store
        .record_signature(&packaging.id, "cosign://fixture", &digest('4'))
        .unwrap()
        .0
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

#[tokio::test]
async fn publication_routes_are_idempotent_persistent_and_queryable() {
    let root = root("lifecycle");
    let mut config = config(&root);
    config.works_base_domain = Some("works.example.test".into());
    let state = http_api::app_state(config.clone());
    state
        .store
        .upsert_project_access(
            "publication-project",
            "owner-1".into(),
            "ws-publication-test".into(),
        )
        .await
        .unwrap();
    let release = seed_validated_release(&state).await;
    let app = http_api::router_with_state(state.clone());
    let body = json!({
        "releaseId": release.id,
        "expectedGeneration": 0,
        "runtimeProfileId": "static-web-v1"
    });
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/projects/publication-project/publish")
            .header("content-type", "application/json")
            .header("idempotency-key", "publish-request-1")
            .header("if-none-match", "*")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    };
    let first = app.clone().oneshot(request()).await.unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first = response_json(first).await;
    let operation_id = first["operation"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        first["operation"]["status"],
        serde_json::to_value(PublishOperationStatus::DesiredStateCommitted).unwrap()
    );
    let repeated = response_json(app.clone().oneshot(request()).await.unwrap()).await;
    assert_eq!(repeated["operation"]["id"], operation_id);

    let conflicting = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/publication-project/publish")
                .header("content-type", "application/json")
                .header("idempotency-key", "publish-request-1")
                .header("if-none-match", "*")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "releaseId": release.id,
                        "expectedGeneration": 1,
                        "runtimeProfileId": "static-web-v1"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(conflicting.status(), StatusCode::CONFLICT);

    let deployment = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects/publication-project/deployment-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deployment.status(), StatusCode::OK);
    let deployment = response_json(deployment).await;
    assert_eq!(deployment["runtime"]["desiredGeneration"], 1);
    assert!(deployment["publicUrl"]
        .as_str()
        .unwrap()
        .ends_with(".works.example.test"));
    assert_eq!(
        deployment["runtime"]["status"],
        serde_json::to_value(WorkRuntimeStatus::Publishing).unwrap()
    );

    let operation = app
        .oneshot(
            Request::builder()
                .uri(format!("/operations/{operation_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(operation.status(), StatusCode::OK);

    drop(state);
    let recovered = http_api::app_state(config);
    assert_eq!(
        recovered
            .store
            .publication_store()
            .operation(&operation_id)
            .unwrap()
            .desired_generation,
        1
    );
    assert_eq!(
        recovered.store.publication_store().pending_outbox().len(),
        1
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn publication_mutation_requires_idempotency_and_validated_release() {
    let root = root("validation");
    let state = http_api::app_state(config(&root));
    state
        .store
        .upsert_project_access(
            "publication-project",
            "owner-1".into(),
            "ws-publication-test".into(),
        )
        .await
        .unwrap();
    let release = seed_validated_release(&state).await;
    let app = http_api::router_with_state(state);
    let missing_precondition = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/publication-project/publish")
                .header("content-type", "application/json")
                .header("idempotency-key", "missing-precondition")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "releaseId": release.id,
                        "expectedGeneration": 0
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        missing_precondition.status(),
        StatusCode::PRECONDITION_REQUIRED
    );
    let stale_precondition = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/publication-project/publish")
                .header("content-type", "application/json")
                .header("idempotency-key", "stale-precondition")
                .header("if-none-match", "\"already-published\"")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "releaseId": release.id,
                        "expectedGeneration": 0
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale_precondition.status(), StatusCode::PRECONDITION_FAILED);
    let missing_key = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/publication-project/publish")
                .header("content-type", "application/json")
                .header("if-none-match", "*")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "releaseId": release.id })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    assert!(app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/publication-project/unpublish")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
        .is_client_error());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn publication_write_requires_scoped_owner_principal_when_auth_is_enabled() {
    let root = root("authorization");
    let signing_key = SigningKey::from_bytes(&[5_u8; 32]);
    let public_key_path = root.join("publication-public.der");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        &public_key_path,
        signing_key
            .verifying_key()
            .to_public_key_der()
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let mut config = config(&root);
    config.public_principal_auth_mode = PublicPrincipalAuthMode::Required;
    config.public_principal_public_key_files = vec![public_key_path];
    config.public_principal_issuer = "anydesign-bff".into();
    config.public_principal_audience = "anydesign-runtime-public".into();
    let state = http_api::app_state(config);
    state
        .store
        .upsert_project_access("publication-project", "owner-1".into(), "ws-test".into())
        .await
        .unwrap();
    let release = seed_validated_release(&state).await;
    let issuer = PublicPrincipalJwtIssuer::from_signing_key(
        signing_key,
        "anydesign-bff",
        "anydesign-runtime-public",
        60,
    );
    let token = issuer
        .issue(PublicPrincipalClaims {
            iss: String::new(),
            aud: String::new(),
            sub: "owner-1".into(),
            jti: "publication-owner-jti-0001".into(),
            exp: 0,
            iat: 0,
            project_id: "publication-project".into(),
            operations: vec![PUBLICATION_WRITE_OPERATION.into()],
        })
        .unwrap();
    let body = serde_json::to_vec(&json!({
        "releaseId": release.id,
        "expectedGeneration": 0
    }))
    .unwrap();
    let app = http_api::router_with_state(state);
    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/publication-project/publish")
                .header("content-type", "application/json")
                .header("idempotency-key", "authorized-publish")
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let authorized = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/publication-project/publish")
                .header("content-type", "application/json")
                .header("idempotency-key", "authorized-publish")
                .header("authorization", format!("Bearer {token}"))
                .header("if-none-match", "*")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authorized.status(), StatusCode::ACCEPTED);
    std::fs::remove_dir_all(root).unwrap();
}
