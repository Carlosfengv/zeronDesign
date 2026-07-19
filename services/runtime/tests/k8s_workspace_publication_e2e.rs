use anydesign_runtime::{
    publication::{
        KubernetesWorkRuntimeBackend, PublicationIntent, PublicationStore, PublishOperationKind,
        WorkRuntimeController, WorkRuntimeStatus,
    },
    release::{PackagingScanEvidence, ReleasePackagingInput, ReleaseStore, WorkRelease},
    types::sha256_hex,
};
use k8s_openapi::api::{apps::v1::Deployment, core::v1::Service, networking::v1::Ingress};
use kube::{Api, Client};
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn generated_artifacts_publish_into_their_workspace_and_survive_controller_restart() {
    if std::env::var("RUN_WORKSPACE_PUBLICATION_E2E")
        .ok()
        .as_deref()
        != Some("1")
    {
        eprintln!("skipping Workspace publication E2E; set RUN_WORKSPACE_PUBLICATION_E2E=1");
        return;
    }

    let project_a = required_env("WORKSPACE_PUBLICATION_PROJECT_A");
    let project_b = required_env("WORKSPACE_PUBLICATION_PROJECT_B");
    let namespace_a = required_env("WORKSPACE_PUBLICATION_NAMESPACE_A");
    let namespace_b = required_env("WORKSPACE_PUBLICATION_NAMESPACE_B");
    let repository = required_env("WORKSPACE_PUBLICATION_IMAGE_REPOSITORY");
    let digest_a = required_env("WORKSPACE_PUBLICATION_IMAGE_DIGEST_A");
    let digest_b = required_env("WORKSPACE_PUBLICATION_IMAGE_DIGEST_B");
    let artifact_hash_a = required_env("WORKSPACE_PUBLICATION_ARTIFACT_HASH_A");
    let artifact_hash_b = required_env("WORKSPACE_PUBLICATION_ARTIFACT_HASH_B");
    let runtime_manifest_hash = required_env("WORKSPACE_PUBLICATION_RUNTIME_MANIFEST_HASH");
    let version_a = required_env("WORKSPACE_PUBLICATION_VERSION_A");
    let version_b = required_env("WORKSPACE_PUBLICATION_VERSION_B");
    let evidence_path = required_env("WORKSPACE_PUBLICATION_EVIDENCE_PATH");
    let ingress_exposure =
        std::env::var("WORK_RUNTIME_EXPOSURE").ok().as_deref() == Some("ingress");

    let root = unique_root();
    let releases = Arc::new(ReleaseStore::open(root.join("release")).unwrap());
    let publications = Arc::new(PublicationStore::open(root.join("publication")).unwrap());
    let release_a = seed_release(
        &releases,
        &project_a,
        &version_a,
        &artifact_hash_a,
        &runtime_manifest_hash,
        &repository,
        &digest_a,
    );
    let release_b = seed_release(
        &releases,
        &project_b,
        &version_b,
        &artifact_hash_b,
        &runtime_manifest_hash,
        &repository,
        &digest_b,
    );
    publish(
        &publications,
        &project_a,
        &namespace_a,
        &release_a.id,
        "workspace-publish-a",
    );
    publish(
        &publications,
        &project_b,
        &namespace_b,
        &release_b.id,
        "workspace-publish-b",
    );

    let backend = Arc::new(KubernetesWorkRuntimeBackend::try_default().await.unwrap());
    let controller = WorkRuntimeController::new(
        Arc::clone(&publications),
        Arc::clone(&releases),
        Arc::clone(&backend),
        Duration::from_millis(250),
    );
    assert_eq!(controller.reconcile_once(true).await.unwrap(), 2);

    let state_a = publications.runtime(&project_a).unwrap();
    let state_b = publications.runtime(&project_b).unwrap();
    let expected_status = if ingress_exposure {
        WorkRuntimeStatus::Published
    } else {
        WorkRuntimeStatus::Publishing
    };
    assert_eq!(state_a.status, expected_status);
    assert_eq!(state_b.status, expected_status);
    assert_eq!(
        state_a.current_release_id.as_deref(),
        Some(release_a.id.as_str())
    );
    assert_eq!(
        state_b.current_release_id.as_deref(),
        Some(release_b.id.as_str())
    );
    assert_eq!(state_a.observed_generation, state_a.desired_generation);
    assert_eq!(state_b.observed_generation, state_b.desired_generation);
    assert_eq!(state_a.workspace_namespace, namespace_a);
    assert_eq!(state_b.workspace_namespace, namespace_b);
    assert_ne!(state_a.service_name, state_b.service_name);

    let client = Client::try_default().await.unwrap();
    verify_resources(
        client.clone(),
        &namespace_a,
        state_a.current_deployment_name.as_deref().unwrap(),
        &state_a.service_name,
        ingress_exposure,
    )
    .await;
    verify_resources(
        client,
        &namespace_b,
        state_b.current_deployment_name.as_deref().unwrap(),
        &state_b.service_name,
        ingress_exposure,
    )
    .await;

    let restarted = WorkRuntimeController::new(
        Arc::clone(&publications),
        releases,
        backend,
        Duration::from_millis(250),
    );
    assert_eq!(restarted.reconcile_once(true).await.unwrap(), 2);
    assert_eq!(
        publications.runtime(&project_a).unwrap().deployment_uid,
        state_a.deployment_uid
    );
    assert_eq!(
        publications.runtime(&project_b).unwrap().deployment_uid,
        state_b.deployment_uid
    );

    let external_url = |host_slug: &str| {
        if !ingress_exposure {
            return None;
        }
        let base_domain = required_env("WORKS_BASE_DOMAIN");
        let port = required_env("WORKSPACE_PUBLICATION_HTTPS_PORT");
        Some(format!("https://{host_slug}.{base_domain}:{port}/"))
    };
    let evidence_status = if ingress_exposure {
        "published"
    } else {
        "workload_ready"
    };

    let evidence = json!({
        "schemaVersion": "workspace-published-works-evidence@1",
        "projects": [
            {
                "projectId": project_a,
                "workspaceNamespace": namespace_a,
                "releaseId": release_a.id,
                "imageDigest": digest_a,
                "artifactManifestHash": artifact_hash_a,
                "runtimeManifestHash": runtime_manifest_hash,
                "versionId": version_a,
                "deploymentName": state_a.current_deployment_name,
                "deploymentUid": state_a.deployment_uid,
                "serviceName": state_a.service_name,
                "ingressName": state_a.ingress_name,
                "hostSlug": state_a.host_slug,
                "url": external_url(&state_a.host_slug),
                "status": evidence_status,
                "externalReleaseIdentityVerified": ingress_exposure,
                "controllerRestartRecovered": true
            },
            {
                "projectId": project_b,
                "workspaceNamespace": namespace_b,
                "releaseId": release_b.id,
                "imageDigest": digest_b,
                "artifactManifestHash": artifact_hash_b,
                "runtimeManifestHash": runtime_manifest_hash,
                "versionId": version_b,
                "deploymentName": state_b.current_deployment_name,
                "deploymentUid": state_b.deployment_uid,
                "serviceName": state_b.service_name,
                "ingressName": state_b.ingress_name,
                "hostSlug": state_b.host_slug,
                "url": external_url(&state_b.host_slug),
                "status": evidence_status,
                "externalReleaseIdentityVerified": ingress_exposure,
                "controllerRestartRecovered": true
            }
        ]
    });
    let evidence_path = PathBuf::from(evidence_path);
    std::fs::create_dir_all(evidence_path.parent().unwrap()).unwrap();
    std::fs::write(&evidence_path, format!("{evidence:#}\n")).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

async fn verify_resources(
    client: Client,
    namespace: &str,
    deployment_name: &str,
    service_name: &str,
    ingress_exposure: bool,
) {
    let deployment = Api::<Deployment>::namespaced(client.clone(), namespace)
        .get(deployment_name)
        .await
        .unwrap();
    assert_eq!(deployment.status.unwrap().available_replicas, Some(1));
    let service = Api::<Service>::namespaced(client.clone(), namespace)
        .get(service_name)
        .await
        .unwrap();
    assert_eq!(service.spec.unwrap().type_.as_deref(), Some("ClusterIP"));
    let ingress = Api::<Ingress>::namespaced(client, namespace)
        .get_opt(service_name)
        .await
        .unwrap();
    assert_eq!(ingress.is_some(), ingress_exposure);
}

fn seed_release(
    store: &ReleaseStore,
    project_id: &str,
    version_id: &str,
    artifact_manifest_hash: &str,
    runtime_manifest_hash: &str,
    repository: &str,
    image_digest: &str,
) -> WorkRelease {
    let input = ReleasePackagingInput {
        project_id: project_id.into(),
        version_id: version_id.into(),
        run_id: "workspace-publication-e2e".into(),
        template_id: format!("workspace-generated-{project_id}"),
        template_version: "1".into(),
        artifact_manifest_hash: artifact_manifest_hash.into(),
        runtime_manifest_hash: runtime_manifest_hash.into(),
        source_snapshot_uri: format!("runtime://source/{project_id}"),
        runtime_profile_id: "static-web-v1".into(),
        base_image_digest: format!("sha256:{}", "c".repeat(64)),
        packager_version: "workspace-generated-e2e@1".into(),
        registry_repository: repository.into(),
        scan_policy_version: "workspace-generated-e2e-scan@1".into(),
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
                policy_version: "workspace-generated-e2e-scan@1".into(),
                passed: true,
                critical_vulnerabilities: 0,
                high_vulnerabilities: 0,
                secret_findings: 0,
                report_digest: evidence_digest.clone(),
            },
        )
        .unwrap();
    store
        .record_signature(
            &packaging.id,
            "workspace-generated-e2e-signer",
            &evidence_digest,
        )
        .unwrap()
        .0
}

fn publish(
    store: &PublicationStore,
    project_id: &str,
    workspace_namespace: &str,
    release_id: &str,
    idempotency_key: &str,
) {
    store
        .commit_intent(&PublicationIntent {
            project_id: project_id.into(),
            workspace_namespace: workspace_namespace.into(),
            kind: PublishOperationKind::Publish,
            release_id: Some(release_id.into()),
            expected_current_release_id: None,
            expected_generation: Some(0),
            runtime_profile_id: "static-web-v1".into(),
            idempotency_key: idempotency_key.into(),
        })
        .unwrap();
}

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"))
}

fn unique_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "workspace-publication-{}-{}-{}",
        std::process::id(),
        sha256_hex(b"workspace-publication"),
        rand::random::<u64>()
    ))
}
