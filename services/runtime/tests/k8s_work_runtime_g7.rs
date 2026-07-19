use anydesign_runtime::{
    publication::{
        KubernetesIngressExposure, KubernetesReleaseProtectionSource, KubernetesWorkRuntimeBackend,
        PublicationIntent, PublicationStore, PublishOperationKind, PublishOperationStatus,
        WorkRuntimeController, WorkRuntimeStatus,
    },
    release::{
        PackagingScanEvidence, ReleasePackagingInput, ReleaseProtectionSource, ReleaseStore,
    },
};
use k8s_openapi::api::{
    apps::v1::Deployment,
    core::v1::{Pod, Service},
    discovery::v1::EndpointSlice,
    networking::v1::Ingress,
};
use kube::{
    api::{Api, Patch, PatchParams},
    Client, ResourceExt,
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
    let ingresses: Api<Ingress> = Api::namespaced(client.clone(), "ws-public-runtime-e2e");
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
        "metadata": {"name": "foreign-host-owner", "namespace": "ws-public-runtime-e2e"},
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
            workspace_namespace: "ws-public-runtime-e2e".into(),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_rollback_restart_and_failed_switch_restore_blue_on_k3d() {
    if std::env::var("RUN_WORK_RUNTIME_G8_K8S_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping G8 k3d E2E; set RUN_WORK_RUNTIME_G8_K8S_E2E=1");
        return;
    }
    let repository = required_env("G8_IMAGE_REPOSITORY");
    let root = unique_root();
    let releases = Arc::new(ReleaseStore::open(root.join("release")).unwrap());
    let publication = Arc::new(PublicationStore::open(root.join("publication")).unwrap());
    let release_a = seed_g8_release(
        &releases,
        &repository,
        &required_env("G8_IMAGE_DIGEST_A"),
        'a',
    );
    let release_b = seed_g8_release(
        &releases,
        &repository,
        &required_env("G8_IMAGE_DIGEST_B"),
        'b',
    );
    let (publish_operation, _) = publication
        .commit_intent(&g8_intent(
            PublishOperationKind::Publish,
            &release_a.id,
            None,
            0,
            "g8-publish-a",
        ))
        .unwrap();
    let client = Client::try_default().await.unwrap();
    let controller = g8_controller(
        Arc::clone(&publication),
        Arc::clone(&releases),
        client.clone(),
    );
    assert_eq!(controller.reconcile_once(false).await.unwrap(), 1);
    assert_eq!(
        publication.operation(&publish_operation.id).unwrap().status,
        PublishOperationStatus::Completed
    );
    let published_a = publication.runtime("g8-project").unwrap();
    let host = external_host(&published_a.host_slug);
    assert_external_release(&host, &release_a.id).await;
    assert_html_no_store(&host).await;

    let (update_operation, _) = publication
        .commit_intent(&g8_intent(
            PublishOperationKind::Update,
            &release_b.id,
            Some(&release_a.id),
            1,
            "g8-update-b",
        ))
        .unwrap();
    assert_eq!(controller.reconcile_once(false).await.unwrap(), 1);
    assert_eq!(
        publication.operation(&update_operation.id).unwrap().status,
        PublishOperationStatus::TrafficSwitched
    );
    assert_eq!(controller.reconcile_once(false).await.unwrap(), 1);
    let updated_b = publication.runtime("g8-project").unwrap();
    assert_eq!(
        updated_b.current_release_id.as_deref(),
        Some(release_b.id.as_str())
    );
    assert_eq!(
        updated_b.previous_release_id.as_deref(),
        Some(release_a.id.as_str())
    );
    assert_eq!(
        service_release(&client, &updated_b.service_name).await,
        release_b.id
    );
    assert_endpoint_release(&client, &updated_b.service_name, &release_b.id).await;
    assert_external_release(&host, &release_b.id).await;
    assert_two_release_deployments(&client, &updated_b.service_name).await;
    let protection = KubernetesReleaseProtectionSource::new(
        Arc::clone(&publication),
        Arc::clone(&releases),
        client.clone(),
    )
    .snapshot()
    .await
    .unwrap();
    assert!(protection.release_ids.contains(&release_a.id));
    assert!(protection.release_ids.contains(&release_b.id));
    assert!(protection
        .image_digests
        .contains(&required_env("G8_IMAGE_DIGEST_A")));
    assert!(protection
        .image_digests
        .contains(&required_env("G8_IMAGE_DIGEST_B")));

    let (rollback_operation, _) = publication
        .commit_intent(&g8_intent(
            PublishOperationKind::Rollback,
            &release_a.id,
            Some(&release_b.id),
            2,
            "g8-rollback-a",
        ))
        .unwrap();
    assert_eq!(controller.reconcile_once(false).await.unwrap(), 1);
    assert_eq!(
        publication
            .operation(&rollback_operation.id)
            .unwrap()
            .status,
        PublishOperationStatus::TrafficSwitched
    );
    assert_eq!(controller.reconcile_once(false).await.unwrap(), 1);
    let rolled_back_a = publication.runtime("g8-project").unwrap();
    assert_eq!(
        rolled_back_a.current_release_id.as_deref(),
        Some(release_a.id.as_str())
    );
    assert_eq!(
        rolled_back_a.previous_release_id.as_deref(),
        Some(release_b.id.as_str())
    );
    assert_eq!(
        service_release(&client, &rolled_back_a.service_name).await,
        release_a.id
    );
    assert_endpoint_release(&client, &rolled_back_a.service_name, &release_a.id).await;
    assert_external_release(&host, &release_a.id).await;

    let (crash_recovery_operation, _) = publication
        .commit_intent(&g8_intent(
            PublishOperationKind::Update,
            &release_b.id,
            Some(&release_a.id),
            3,
            "g8-crash-after-switch",
        ))
        .unwrap();
    force_service_release(&client, &rolled_back_a.service_name, &release_b.id).await;
    let restarted = g8_controller(
        Arc::clone(&publication),
        Arc::clone(&releases),
        client.clone(),
    );
    assert_eq!(restarted.reconcile_once(false).await.unwrap(), 1);
    assert_eq!(
        publication
            .operation(&crash_recovery_operation.id)
            .unwrap()
            .status,
        PublishOperationStatus::TrafficSwitched
    );
    assert_eq!(restarted.reconcile_once(false).await.unwrap(), 1);
    let recovered_b = publication.runtime("g8-project").unwrap();
    assert_eq!(
        recovered_b.current_release_id.as_deref(),
        Some(release_b.id.as_str())
    );
    assert_endpoint_release(&client, &recovered_b.service_name, &release_b.id).await;

    let pods: Api<Pod> = Api::namespaced(client.clone(), "ws-public-runtime-e2e");
    let blue_pod = pods
        .list(&kube::api::ListParams::default().labels(&format!(
            "anydesign.dev/work={},anydesign.dev/release-id={}",
            recovered_b.service_name, release_b.id
        )))
        .await
        .unwrap()
        .items
        .into_iter()
        .next()
        .unwrap();
    create_blocking_endpoint_slice(&client, &recovered_b.service_name, &blue_pod.name_any()).await;
    let (failed_rollback, _) = publication
        .commit_intent(&g8_intent(
            PublishOperationKind::Rollback,
            &release_a.id,
            Some(&release_b.id),
            4,
            "g8-timeout-rollback",
        ))
        .unwrap();
    assert_eq!(restarted.reconcile_once(false).await.unwrap(), 0);
    assert_eq!(
        publication.operation(&failed_rollback.id).unwrap().status,
        PublishOperationStatus::ReconcileRequired
    );
    let restored_b = publication.runtime("g8-project").unwrap();
    assert_eq!(
        restored_b.current_release_id.as_deref(),
        Some(release_b.id.as_str())
    );
    assert_eq!(
        service_release(&client, &restored_b.service_name).await,
        release_b.id
    );
    assert_external_release(&host, &release_b.id).await;
    let slices: Api<EndpointSlice> = Api::namespaced(client.clone(), "ws-public-runtime-e2e");
    slices
        .delete("g8-blocking-old-endpoint", &Default::default())
        .await
        .unwrap();
    for _ in 0..100 {
        if slices
            .get_opt("g8-blocking-old-endpoint")
            .await
            .unwrap()
            .is_none()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(restarted.reconcile_once(true).await.unwrap(), 1);
    assert_eq!(
        publication.operation(&failed_rollback.id).unwrap().status,
        PublishOperationStatus::TrafficSwitched
    );
    assert_eq!(restarted.reconcile_once(true).await.unwrap(), 1);
    let final_a = publication.runtime("g8-project").unwrap();
    assert_eq!(
        final_a.current_release_id.as_deref(),
        Some(release_a.id.as_str())
    );
    assert_eq!(
        service_release(&client, &final_a.service_name).await,
        release_a.id
    );
    assert_endpoint_release(&client, &final_a.service_name, &release_a.id).await;
    assert_external_release(&host, &release_a.id).await;
    std::fs::remove_dir_all(root).unwrap();
}

fn publish_intent(release_id: &str, generation: u64, key: &str) -> PublicationIntent {
    PublicationIntent {
        project_id: "g7-project".into(),
        workspace_namespace: "ws-public-runtime-e2e".into(),
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

fn seed_g8_release(
    store: &ReleaseStore,
    repository: &str,
    image_digest: &str,
    marker: char,
) -> anydesign_runtime::release::WorkRelease {
    let next = char::from_u32(marker as u32 + 1).unwrap();
    let input = ReleasePackagingInput {
        project_id: "g8-project".into(),
        version_id: format!("g8-version-{marker}"),
        run_id: format!("g8-run-{marker}"),
        template_id: "future-static-template".into(),
        template_version: "1".into(),
        artifact_manifest_hash: marker.to_string().repeat(64),
        runtime_manifest_hash: next.to_string().repeat(64),
        source_snapshot_uri: format!("runtime://source/g8-project/{marker}"),
        runtime_profile_id: "static-web-v1".into(),
        base_image_digest: format!("sha256:{}", "c".repeat(64)),
        packager_version: "g8-fixture@1".into(),
        registry_repository: repository.into(),
        scan_policy_version: "g8-scan@1".into(),
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
                policy_version: "g8-scan@1".into(),
                passed: true,
                critical_vulnerabilities: 0,
                high_vulnerabilities: 0,
                secret_findings: 0,
                report_digest: evidence_digest.clone(),
            },
        )
        .unwrap();
    store
        .record_signature(&packaging.id, "g8-fixture-signer", &evidence_digest)
        .unwrap()
        .0
}

fn g8_intent(
    kind: PublishOperationKind,
    release_id: &str,
    expected_current_release_id: Option<&str>,
    expected_generation: u64,
    idempotency_key: &str,
) -> PublicationIntent {
    PublicationIntent {
        project_id: "g8-project".into(),
        workspace_namespace: "ws-public-runtime-e2e".into(),
        kind,
        release_id: Some(release_id.into()),
        expected_current_release_id: expected_current_release_id.map(str::to_string),
        expected_generation: Some(expected_generation),
        runtime_profile_id: "static-web-v1".into(),
        idempotency_key: idempotency_key.into(),
    }
}

fn g8_controller(
    publication: Arc<PublicationStore>,
    releases: Arc<ReleaseStore>,
    client: Client,
) -> WorkRuntimeController<KubernetesWorkRuntimeBackend> {
    let exposure = KubernetesIngressExposure {
        base_domain: required_env("WORKS_BASE_DOMAIN"),
        ingress_class: required_env("WORKS_INGRESS_CLASS"),
        tls_secret_name: required_env("WORKS_TLS_SECRET_NAME"),
        certificate_issuer_name: std::env::var("WORKS_CERTIFICATE_ISSUER_NAME")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        probe_scheme: required_env("WORKS_PROBE_SCHEME"),
        probe_resolve: Some(required_env("WORKS_PROBE_RESOLVE").parse().unwrap()),
        probe_ca_file: Some(PathBuf::from(required_env("WORKS_PROBE_CA_FILE"))),
    };
    let backend = KubernetesWorkRuntimeBackend::new_with_ingress(
        client,
        Duration::from_secs(15),
        required_env("WORK_RUNTIME_PROBER_IMAGE"),
        exposure,
    )
    .unwrap();
    WorkRuntimeController::new(
        publication,
        releases,
        Arc::new(backend),
        Duration::from_secs(1),
    )
}

async fn service_release(client: &Client, service_name: &str) -> String {
    let services: Api<Service> = Api::namespaced(client.clone(), "ws-public-runtime-e2e");
    services
        .get(service_name)
        .await
        .unwrap()
        .spec
        .unwrap()
        .selector
        .unwrap()
        .remove("anydesign.dev/release-id")
        .unwrap()
}

async fn force_service_release(client: &Client, service_name: &str, release_id: &str) {
    let services: Api<Service> = Api::namespaced(client.clone(), "ws-public-runtime-e2e");
    services
        .patch(
            service_name,
            &PatchParams::default(),
            &Patch::Merge(json!({"spec": {"selector": {"anydesign.dev/release-id": release_id}}})),
        )
        .await
        .unwrap();
}

async fn assert_endpoint_release(client: &Client, service_name: &str, release_id: &str) {
    let slices: Api<EndpointSlice> = Api::namespaced(client.clone(), "ws-public-runtime-e2e");
    let pods: Api<Pod> = Api::namespaced(client.clone(), "ws-public-runtime-e2e");
    let mut endpoints = 0usize;
    for slice in slices
        .list(
            &kube::api::ListParams::default()
                .labels(&format!("kubernetes.io/service-name={service_name}")),
        )
        .await
        .unwrap()
    {
        for endpoint in slice.endpoints.into_iter().flatten() {
            let target = endpoint.target_ref.unwrap();
            let pod = pods.get(target.name.as_deref().unwrap()).await.unwrap();
            assert_eq!(
                pod.metadata
                    .labels
                    .unwrap()
                    .get("anydesign.dev/release-id")
                    .unwrap(),
                release_id
            );
            endpoints += 1;
        }
    }
    assert!(endpoints > 0);
}

async fn assert_two_release_deployments(client: &Client, work_name: &str) {
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), "ws-public-runtime-e2e");
    assert_eq!(
        deployments
            .list(
                &kube::api::ListParams::default()
                    .labels(&format!("anydesign.dev/work={work_name}"))
            )
            .await
            .unwrap()
            .items
            .len(),
        2
    );
}

async fn create_blocking_endpoint_slice(client: &Client, service_name: &str, pod_name: &str) {
    let slices: Api<EndpointSlice> = Api::namespaced(client.clone(), "ws-public-runtime-e2e");
    let slice: EndpointSlice = serde_json::from_value(json!({
        "apiVersion": "discovery.k8s.io/v1",
        "kind": "EndpointSlice",
        "metadata": {
            "name": "g8-blocking-old-endpoint",
            "namespace": "ws-public-runtime-e2e",
            "labels": {"kubernetes.io/service-name": service_name}
        },
        "addressType": "IPv4",
        "ports": [{"name": "http", "port": 8080, "protocol": "TCP"}],
        "endpoints": [{
            "addresses": ["10.255.255.1"],
            "conditions": {"ready": true},
            "targetRef": {"kind": "Pod", "namespace": "ws-public-runtime-e2e", "name": pod_name}
        }]
    }))
    .unwrap();
    slices
        .patch(
            "g8-blocking-old-endpoint",
            &PatchParams::apply("g8-switch-timeout-fixture").force(),
            &Patch::Apply(&slice),
        )
        .await
        .unwrap();
}

async fn assert_resources_absent(client: Client, work_name: &str) {
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), "ws-public-runtime-e2e");
    let services: Api<Service> = Api::namespaced(client.clone(), "ws-public-runtime-e2e");
    let ingresses: Api<Ingress> = Api::namespaced(client, "ws-public-runtime-e2e");
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

async fn assert_html_no_store(host: &str) {
    let response = probe_client(host)
        .get(format!("https://{host}/"))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
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
