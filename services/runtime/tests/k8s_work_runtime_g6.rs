use anydesign_runtime::{
    publication::{KubernetesWorkRuntimeBackend, WorkRuntimeController, WorkRuntimeStatus},
    release::{PackagingScanEvidence, ReleasePackagingInput, ReleaseStore},
    types::sha256_hex,
};
use k8s_openapi::api::{
    apps::v1::Deployment,
    core::v1::{Pod, Service},
    networking::v1::{Ingress, NetworkPolicy},
};
use kube::{
    api::{Api, AttachParams, ListParams, Patch, PatchParams},
    Client, ResourceExt,
};
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::io::AsyncReadExt;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dual_work_isolation_restart_and_uid_drift_on_k3d() {
    if std::env::var("RUN_WORK_RUNTIME_G6_K8S_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping G6 k3d E2E; set RUN_WORK_RUNTIME_G6_K8S_E2E=1");
        return;
    }
    let repository = std::env::var("G6_IMAGE_REPOSITORY").expect("G6_IMAGE_REPOSITORY");
    let digest_a = std::env::var("G6_IMAGE_DIGEST_A").expect("G6_IMAGE_DIGEST_A");
    let digest_b = std::env::var("G6_IMAGE_DIGEST_B").expect("G6_IMAGE_DIGEST_B");
    let root = unique_root();
    let releases = Arc::new(ReleaseStore::open(root.join("release")).unwrap());
    let publication = Arc::new(
        anydesign_runtime::publication::PublicationStore::open(root.join("publication")).unwrap(),
    );
    let release_a = seed_release(&releases, "g6-project-a", 'a', &repository, &digest_a);
    let release_b = seed_release(&releases, "g6-project-b", 'b', &repository, &digest_b);
    publish(&publication, "g6-project-a", &release_a.id, "publish-a");
    publish(&publication, "g6-project-b", &release_b.id, "publish-b");

    let backend = Arc::new(KubernetesWorkRuntimeBackend::try_default().await.unwrap());
    let controller = WorkRuntimeController::new(
        Arc::clone(&publication),
        Arc::clone(&releases),
        Arc::clone(&backend),
        Duration::from_secs(1),
    );
    assert_eq!(controller.reconcile_once(true).await.unwrap(), 2);

    let state_a = publication.runtime("g6-project-a").unwrap();
    let state_b = publication.runtime("g6-project-b").unwrap();
    assert_ne!(
        state_a.current_deployment_name,
        state_b.current_deployment_name
    );
    assert_ne!(state_a.service_name, state_b.service_name);
    assert_ne!(state_a.deployment_uid, state_b.deployment_uid);
    assert_eq!(state_a.observed_generation, state_a.desired_generation);
    assert_eq!(state_b.observed_generation, state_b.desired_generation);

    let client = Client::try_default().await.unwrap();
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), "anydesign-works");
    let services: Api<Service> = Api::namespaced(client.clone(), "anydesign-works");
    let policies: Api<NetworkPolicy> = Api::namespaced(client.clone(), "anydesign-works");
    let ingresses: Api<Ingress> = Api::namespaced(client.clone(), "anydesign-works");
    assert!(ingresses
        .list(&Default::default())
        .await
        .unwrap()
        .items
        .is_empty());
    for state in [&state_a, &state_b] {
        let deployment = deployments
            .get(state.current_deployment_name.as_deref().unwrap())
            .await
            .unwrap();
        let pod = deployment.spec.unwrap().template.spec.unwrap();
        assert_eq!(pod.automount_service_account_token, Some(false));
        let volumes = pod.volumes.unwrap_or_default();
        assert_eq!(volumes.len(), 1);
        assert!(volumes[0].empty_dir.is_some());
        assert!(volumes[0].persistent_volume_claim.is_none());
        assert!(volumes[0].secret.is_none());
        assert!(pod.containers[0]
            .image
            .as_deref()
            .unwrap()
            .contains("@sha256:"));
        let service = services.get(&state.service_name).await.unwrap();
        assert_eq!(
            service.spec.as_ref().unwrap().type_.as_deref(),
            Some("ClusterIP")
        );
        let selector = service.spec.unwrap().selector.unwrap();
        assert_eq!(
            selector.get("anydesign.dev/work"),
            Some(&state.service_name)
        );
        assert_eq!(
            selector.get("anydesign.dev/release-id"),
            state.current_release_id.as_ref()
        );
        policies.get(&state.service_name).await.unwrap();
    }
    assert_cross_work_blocked(client.clone(), &state_a.service_name, &state_b.service_name).await;
    assert_cross_work_blocked(client.clone(), &state_b.service_name, &state_a.service_name).await;

    // A new controller instance represents process restart and must safely re-observe the same UIDs.
    let restarted = WorkRuntimeController::new(
        Arc::clone(&publication),
        Arc::clone(&releases),
        backend,
        Duration::from_secs(1),
    );
    assert_eq!(restarted.reconcile_once(true).await.unwrap(), 2);
    assert_eq!(
        publication.runtime("g6-project-a").unwrap().deployment_uid,
        state_a.deployment_uid
    );

    // Same-name replacement is never silently adopted: it becomes ReconcileRequired.
    let deployment_name = state_a.current_deployment_name.as_deref().unwrap();
    deployments
        .delete(deployment_name, &Default::default())
        .await
        .unwrap();
    wait_deleted(&deployments, deployment_name).await;
    let replacement: Deployment = serde_json::from_value(json!({
        "apiVersion": "apps/v1", "kind": "Deployment",
        "metadata": {
            "name": deployment_name, "namespace": "anydesign-works",
            "labels": {"app.kubernetes.io/managed-by": "anydesign-work-runtime-controller"},
            "annotations": {
                "anydesign.dev/signature-digest": format!("sha256:{}", "d".repeat(64)),
                "anydesign.dev/provenance-digest": format!("sha256:{}", "d".repeat(64)),
                "anydesign.dev/scan-report-digest": format!("sha256:{}", "d".repeat(64))
            }
        },
        "spec": {
            "replicas": 0,
            "selector": {"matchLabels": {"replacement": "true"}},
            "template": {"metadata": {"labels": {"replacement": "true"}}, "spec": {
                "automountServiceAccountToken": false,
                "containers": [{"name": "replacement", "image": format!("{repository}/{}@{digest_a}", release_a.id)}]
            }}
        }
    })).unwrap();
    deployments
        .patch(
            deployment_name,
            &PatchParams::apply("g6-drift-fixture").force(),
            &Patch::Apply(&replacement),
        )
        .await
        .unwrap();
    assert_eq!(restarted.reconcile_once(true).await.unwrap(), 1);
    let drifted = publication.runtime("g6-project-a").unwrap();
    assert_eq!(drifted.status, WorkRuntimeStatus::ReconcileRequired);
    assert!(drifted.last_error.unwrap().contains("UID drift"));

    // Recovery is explicit and CAS-bound to the last trusted UID; the next reconcile creates a
    // fresh owned Deployment and never adopts the foreign replacement.
    publication
        .authorize_deployment_recreation("g6-project-a", state_a.deployment_uid.as_deref().unwrap())
        .unwrap();
    deployments
        .delete(deployment_name, &Default::default())
        .await
        .unwrap();
    wait_deleted(&deployments, deployment_name).await;
    assert_eq!(restarted.reconcile_once(true).await.unwrap(), 2);
    let recovered = publication.runtime("g6-project-a").unwrap();
    assert_ne!(recovered.deployment_uid, state_a.deployment_uid);
    assert_eq!(
        deployments
            .get(deployment_name)
            .await
            .unwrap()
            .status
            .unwrap()
            .available_replicas,
        Some(1)
    );

    std::fs::remove_dir_all(root).unwrap();
}

fn seed_release(
    store: &ReleaseStore,
    project_id: &str,
    marker: char,
    repository: &str,
    image_digest: &str,
) -> anydesign_runtime::release::WorkRelease {
    let input = ReleasePackagingInput {
        project_id: project_id.into(),
        version_id: format!("version-{marker}"),
        run_id: format!("run-{marker}"),
        template_id: format!("template-{marker}"),
        template_version: "1".into(),
        artifact_manifest_hash: marker.to_string().repeat(64),
        runtime_manifest_hash: ((marker as u8 + 1) as char).to_string().repeat(64),
        source_snapshot_uri: format!("runtime://source/{project_id}"),
        runtime_profile_id: "static-web-v1".into(),
        base_image_digest: format!("sha256:{}", "c".repeat(64)),
        packager_version: "g6-fixture@1".into(),
        registry_repository: repository.into(),
        scan_policy_version: "g6-scan@1".into(),
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
                policy_version: "g6-scan@1".into(),
                passed: true,
                critical_vulnerabilities: 0,
                high_vulnerabilities: 0,
                secret_findings: 0,
                report_digest: evidence_digest.clone(),
            },
        )
        .unwrap();
    store
        .record_signature(&packaging.id, "g6-fixture-signer", &evidence_digest)
        .unwrap()
        .0
}

fn publish(
    store: &anydesign_runtime::publication::PublicationStore,
    project_id: &str,
    release_id: &str,
    key: &str,
) {
    store
        .commit_intent(&anydesign_runtime::publication::PublicationIntent {
            project_id: project_id.into(),
            kind: anydesign_runtime::publication::PublishOperationKind::Publish,
            release_id: Some(release_id.into()),
            expected_current_release_id: None,
            expected_generation: Some(0),
            runtime_profile_id: "static-web-v1".into(),
            idempotency_key: key.into(),
        })
        .unwrap();
}

async fn wait_deleted(api: &Api<Deployment>, name: &str) {
    for _ in 0..100 {
        if api.get_opt(name).await.unwrap().is_none() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("Deployment was not deleted: {name}");
}

async fn assert_cross_work_blocked(client: Client, source_work: &str, target_service: &str) {
    let pods: Api<Pod> = Api::namespaced(client, "anydesign-works");
    let pod = pods
        .list(&ListParams::default().labels(&format!("anydesign.dev/work={source_work}")))
        .await
        .unwrap()
        .items
        .into_iter()
        .next()
        .expect("source work pod");
    let command = format!(
        "if wget -q -T 2 -O /dev/null http://{target_service}; then echo CONNECTED; else echo BLOCKED; fi"
    );
    let mut process = pods
        .exec(
            &pod.name_any(),
            ["sh", "-c", &command],
            &AttachParams::default(),
        )
        .await
        .unwrap();
    let mut stdout = String::new();
    process
        .stdout()
        .expect("exec stdout")
        .read_to_string(&mut stdout)
        .await
        .unwrap();
    process.join().await.unwrap();
    assert_eq!(
        stdout.trim(),
        "BLOCKED",
        "cross-work network access was not denied"
    );
}

fn unique_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "work-runtime-g6-{}-{}-{}",
        std::process::id(),
        sha256_hex(b"g6"),
        rand::random::<u64>()
    ))
}
