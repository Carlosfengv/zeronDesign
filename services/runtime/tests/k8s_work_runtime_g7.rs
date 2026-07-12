use anydesign_runtime::{
    publication::{
        KubernetesWorkRuntimeBackend, PublicationIntent, PublicationStore, PublishOperationKind,
        PublishOperationStatus, WorkRuntimeController, WorkRuntimeStatus,
    },
    release::{PackagingScanEvidence, ReleasePackagingInput, ReleaseStore},
};
use k8s_openapi::api::{apps::v1::Deployment, core::v1::Service, networking::v1::Ingress};
use kube::{
    api::{Api, Patch, PatchParams},
    Client,
};
use serde_json::json;
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_unpublish_and_republish_keep_stable_https_host_on_k3d() {
    if std::env::var("RUN_WORK_RUNTIME_G7_K8S_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping G7 k3d E2E; set RUN_WORK_RUNTIME_G7_K8S_E2E=1");
        return;
    }
    let repository = required_env("G7_IMAGE_REPOSITORY");
    let image_digest = required_env("G7_IMAGE_DIGEST");
    let root = unique_root();
    let releases = Arc::new(ReleaseStore::open(root.join("release")).unwrap());
    let publication = Arc::new(PublicationStore::open(root.join("publication")).unwrap());
    let release = seed_release(&releases, &repository, &image_digest);
    let (publish_operation, initial) = publication
        .commit_intent(&publish_intent(&release.id, 0, "g7-publish"))
        .unwrap();
    let stable_host_slug = initial.host_slug.clone();

    let client = Client::try_default().await.unwrap();
    let ingresses: Api<Ingress> = Api::namespaced(client.clone(), "anydesign-works");
    assert!(ingresses
        .list(&Default::default())
        .await
        .unwrap()
        .items
        .is_empty());
    let reserved_host = external_host(&initial.host_slug);
    let conflicting: Ingress = serde_json::from_value(json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "Ingress",
        "metadata": {"name": "foreign-host-owner", "namespace": "anydesign-works"},
        "spec": {
            "ingressClassName": "traefik",
            "rules": [{"host": reserved_host, "http": {"paths": [{
                "path": "/", "pathType": "Prefix",
                "backend": {"service": {"name": "foreign-service", "port": {"number": 80}}}
            }]}}]
        }
    }))
    .unwrap();
    ingresses
        .patch(
            "foreign-host-owner",
            &PatchParams::apply("g7-host-conflict-fixture").force(),
            &Patch::Apply(&conflicting),
        )
        .await
        .unwrap();
    let backend = Arc::new(KubernetesWorkRuntimeBackend::try_default().await.unwrap());
    let controller = WorkRuntimeController::new(
        Arc::clone(&publication),
        Arc::clone(&releases),
        backend,
        Duration::from_secs(1),
    );
    assert_eq!(controller.reconcile_once(false).await.unwrap(), 0);
    assert!(publication
        .runtime("g7-project")
        .unwrap()
        .last_error
        .unwrap()
        .contains("already owned"));
    ingresses
        .delete("foreign-host-owner", &Default::default())
        .await
        .unwrap();
    for _ in 0..100 {
        if ingresses
            .get_opt("foreign-host-owner")
            .await
            .unwrap()
            .is_none()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(controller.reconcile_once(true).await.unwrap(), 1);
    let published = publication.runtime("g7-project").unwrap();
    assert_eq!(published.status, WorkRuntimeStatus::Published);
    assert_eq!(published.host_slug, stable_host_slug);
    assert!(published.ingress_uid.is_some());
    let live_ingress = ingresses.get(&published.ingress_name).await.unwrap();
    let backend_service = live_ingress.spec.unwrap().rules.unwrap()[0]
        .http
        .as_ref()
        .unwrap()
        .paths[0]
        .backend
        .service
        .as_ref()
        .unwrap()
        .name
        .clone();
    assert_eq!(backend_service, published.service_name);
    assert!(!backend_service.contains("sandbox"));
    assert_eq!(
        publication.operation(&publish_operation.id).unwrap().status,
        PublishOperationStatus::Completed
    );
    let first_deployment_uid = published.deployment_uid.clone();
    let first_service_uid = published.service_uid.clone();
    let first_ingress_uid = published.ingress_uid.clone();
    let host = external_host(&published.host_slug);
    assert_external_release(&host, &release.id).await;

    let (unpublish_operation, _) = publication
        .commit_intent(&PublicationIntent {
            project_id: "g7-project".into(),
            kind: PublishOperationKind::Unpublish,
            release_id: None,
            expected_current_release_id: Some(release.id.clone()),
            expected_generation: Some(1),
            runtime_profile_id: "static-web-v1".into(),
            idempotency_key: "g7-unpublish".into(),
        })
        .unwrap();
    assert_eq!(controller.reconcile_once(false).await.unwrap(), 1);
    let unpublished = publication.runtime("g7-project").unwrap();
    assert_eq!(unpublished.status, WorkRuntimeStatus::Unpublished);
    assert_eq!(unpublished.host_slug, stable_host_slug);
    assert_eq!(
        unpublished.last_successful_release_id,
        Some(release.id.clone())
    );
    assert_eq!(
        publication
            .operation(&unpublish_operation.id)
            .unwrap()
            .status,
        PublishOperationStatus::Completed
    );
    assert_resources_absent(client.clone(), &unpublished.service_name).await;
    assert_external_closed(&host).await;

    let (republish_operation, _) = publication
        .commit_intent(&publish_intent(&release.id, 2, "g7-republish"))
        .unwrap();
    assert_eq!(controller.reconcile_once(false).await.unwrap(), 1);
    let republished = publication.runtime("g7-project").unwrap();
    assert_eq!(republished.status, WorkRuntimeStatus::Published);
    assert_eq!(republished.host_slug, stable_host_slug);
    assert_ne!(republished.deployment_uid, first_deployment_uid);
    assert_ne!(republished.service_uid, first_service_uid);
    assert_ne!(republished.ingress_uid, first_ingress_uid);
    assert_eq!(
        publication
            .operation(&republish_operation.id)
            .unwrap()
            .status,
        PublishOperationStatus::Completed
    );
    assert_external_release(&host, &release.id).await;
    std::fs::remove_dir_all(root).unwrap();
}

fn publish_intent(release_id: &str, generation: u64, key: &str) -> PublicationIntent {
    PublicationIntent {
        project_id: "g7-project".into(),
        kind: PublishOperationKind::Publish,
        release_id: Some(release_id.into()),
        expected_current_release_id: None,
        expected_generation: Some(generation),
        runtime_profile_id: "static-web-v1".into(),
        idempotency_key: key.into(),
    }
}

fn seed_release(
    store: &ReleaseStore,
    repository: &str,
    image_digest: &str,
) -> anydesign_runtime::release::WorkRelease {
    let input = ReleasePackagingInput {
        project_id: "g7-project".into(),
        version_id: "g7-version".into(),
        run_id: "g7-run".into(),
        template_id: "future-static-template".into(),
        template_version: "1".into(),
        artifact_manifest_hash: "e".repeat(64),
        runtime_manifest_hash: "f".repeat(64),
        source_snapshot_uri: "runtime://source/g7-project".into(),
        runtime_profile_id: "static-web-v1".into(),
        base_image_digest: format!("sha256:{}", "c".repeat(64)),
        packager_version: "g7-fixture@1".into(),
        registry_repository: repository.into(),
        scan_policy_version: "g7-scan@1".into(),
    };
    let (_, packaging) = store.prepare(&input).unwrap();
    store.begin_build(&packaging.id).unwrap();
    store.record_built(&packaging.id, image_digest).unwrap();
    store.record_pushed(&packaging.id, image_digest).unwrap();
    store.begin_scan(&packaging.id).unwrap();
    let evidence_digest = format!("sha256:{}", "d".repeat(64));
    store
        .record_scan(
            &packaging.id,
            &evidence_digest,
            &evidence_digest,
            PackagingScanEvidence {
                policy_version: "g7-scan@1".into(),
                passed: true,
                critical_vulnerabilities: 0,
                high_vulnerabilities: 0,
                secret_findings: 0,
                report_digest: evidence_digest.clone(),
            },
        )
        .unwrap();
    store
        .record_signature(&packaging.id, "g7-fixture-signer", &evidence_digest)
        .unwrap()
        .0
}

async fn assert_resources_absent(client: Client, work_name: &str) {
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), "anydesign-works");
    let services: Api<Service> = Api::namespaced(client.clone(), "anydesign-works");
    let ingresses: Api<Ingress> = Api::namespaced(client, "anydesign-works");
    assert!(services.get_opt(work_name).await.unwrap().is_none());
    assert!(ingresses.get_opt(work_name).await.unwrap().is_none());
    assert!(deployments
        .list(&kube::api::ListParams::default().labels(&format!("anydesign.dev/work={work_name}")))
        .await
        .unwrap()
        .items
        .is_empty());
}

async fn assert_external_release(host: &str, release_id: &str) {
    let response = probe_client(host)
        .get(format!("https://{host}/.well-known/anydesign/release"))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert_eq!(
        response
            .headers()
            .get("x-anydesign-release-id")
            .unwrap()
            .to_str()
            .unwrap(),
        release_id
    );
    assert_eq!(
        response.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert_eq!(
        response.headers().get("referrer-policy").unwrap(),
        "no-referrer"
    );
    assert!(response.headers().contains_key("strict-transport-security"));
    assert!(response.text().await.unwrap().contains(release_id));
}

async fn assert_external_closed(host: &str) {
    if let Ok(response) = probe_client(host)
        .get(format!("https://{host}/"))
        .send()
        .await
    {
        assert!(matches!(response.status().as_u16(), 404 | 410));
    }
}

fn probe_client(host: &str) -> reqwest::Client {
    let address: SocketAddr = required_env("WORKS_PROBE_RESOLVE").parse().unwrap();
    let ca = reqwest::Certificate::from_pem(
        &std::fs::read(required_env("WORKS_PROBE_CA_FILE")).unwrap(),
    )
    .unwrap();
    reqwest::Client::builder()
        .no_proxy()
        .resolve(host, address)
        .add_root_certificate(ca)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

fn external_host(host_slug: &str) -> String {
    format!("{host_slug}.{}", required_env("WORKS_BASE_DOMAIN"))
}

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"))
}

fn unique_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "work-runtime-g7-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ))
}
