use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

fn root(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    std::env::temp_dir().join(format!(
        "release-store-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn input() -> ReleasePackagingInput {
    ReleasePackagingInput {
        project_id: "project-1".to_string(),
        version_id: "version-1".to_string(),
        run_id: "run-1".to_string(),
        template_id: "astro-website".to_string(),
        template_version: "astro-website@runtime-p3".to_string(),
        artifact_manifest_hash: "a".repeat(64),
        runtime_manifest_hash: "b".repeat(64),
        source_snapshot_uri: "runtime://source-snapshots/project-1/build-1".to_string(),
        runtime_profile_id: "static-web-v1".to_string(),
        base_image_digest: format!("sha256:{}", "c".repeat(64)),
        packager_version: "packager@1".to_string(),
        registry_repository: "registry.example/works".to_string(),
        scan_policy_version: "scan@1".to_string(),
    }
}

fn digest(character: char) -> String {
    format!("sha256:{}", character.to_string().repeat(64))
}

#[test]
fn prepare_is_idempotent_and_recovers_from_journal() {
    let root = root("recovery");
    let store = ReleaseStore::open(&root).unwrap();
    let first = store.prepare(&input()).unwrap();
    assert_eq!(first, store.prepare(&input()).unwrap());
    drop(store);
    let recovered = ReleaseStore::open(&root).unwrap();
    assert_eq!(recovered.release(&first.0.id), Some(first.0));
    assert_eq!(recovered.packaging(&first.1.id), Some(first.1));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn idempotency_key_cannot_be_rebound_to_different_provenance() {
    let root = root("idempotency-conflict");
    let store = ReleaseStore::open(&root).unwrap();
    store.prepare(&input()).unwrap();
    let mut conflicting = input();
    conflicting.registry_repository = "other-registry.example/works".to_string();
    assert!(matches!(
        store.prepare(&conflicting),
        Err(ReleaseStoreError::IntegrityConflict(_))
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn validation_requires_push_scan_and_signature_evidence() {
    let root = root("evidence");
    let store = ReleaseStore::open(&root).unwrap();
    let (release, packaging) = store.prepare(&input()).unwrap();
    assert!(store
        .record_signature(&packaging.id, "builder", &digest('f'))
        .is_err());
    store.begin_build(&packaging.id).unwrap();
    store.record_built(&packaging.id, &digest('d')).unwrap();
    store.record_pushed(&packaging.id, &digest('d')).unwrap();
    store.begin_scan(&packaging.id).unwrap();
    store
        .record_scan(
            &packaging.id,
            &digest('1'),
            &digest('2'),
            PackagingScanEvidence {
                policy_version: "scan@1".to_string(),
                passed: true,
                critical_vulnerabilities: 0,
                high_vulnerabilities: 0,
                secret_findings: 0,
                report_digest: digest('3'),
            },
        )
        .unwrap();
    let (validated, packaging) = store
        .record_signature(&packaging.id, "cosign://builder", &digest('4'))
        .unwrap();
    assert_eq!(validated.status, WorkReleaseStatus::Validated);
    assert_eq!(packaging.status, ReleasePackagingStatus::Validated);
    assert_eq!(validated.id, release.id);
    assert!(validated
        .runtime_image_ref
        .as_deref()
        .unwrap()
        .contains("@sha256:"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pushed_digest_and_validated_signature_are_immutable() {
    let root = root("immutable");
    let store = ReleaseStore::open(&root).unwrap();
    let (_, packaging) = store.prepare(&input()).unwrap();
    store.begin_build(&packaging.id).unwrap();
    store.record_built(&packaging.id, &digest('a')).unwrap();
    assert!(store.record_pushed(&packaging.id, &digest('b')).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn garbage_collection_is_limited_to_failed_digest_matched_releases() {
    let root = root("garbage-collection");
    let store = ReleaseStore::open(&root).unwrap();
    let (_, packaging) = store.prepare(&input()).unwrap();
    store.begin_build(&packaging.id).unwrap();
    store.record_built(&packaging.id, &digest('a')).unwrap();
    store.record_pushed(&packaging.id, &digest('a')).unwrap();
    assert!(store
        .mark_failed_garbage_collectable(&packaging.id, &digest('a'))
        .is_err());
    store.begin_scan(&packaging.id).unwrap();
    store
        .record_scan(
            &packaging.id,
            &digest('1'),
            &digest('2'),
            PackagingScanEvidence {
                policy_version: "scan@1".to_string(),
                passed: false,
                critical_vulnerabilities: 1,
                high_vulnerabilities: 0,
                secret_findings: 0,
                report_digest: digest('3'),
            },
        )
        .unwrap();
    assert!(store
        .mark_failed_garbage_collectable(&packaging.id, &digest('b'))
        .is_err());
    let (release, _) = store
        .mark_failed_garbage_collectable(&packaging.id, &digest('a'))
        .unwrap();
    assert_eq!(release.status, WorkReleaseStatus::GarbageCollectable);
    let (release, packaging) = store
        .record_garbage_collected(&packaging.id, &digest('a'))
        .unwrap();
    assert_eq!(release.status, WorkReleaseStatus::GarbageCollected);
    assert_eq!(packaging.pushed_image_digest, Some(digest('a')));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn truncated_last_journal_event_is_ignored_during_recovery() {
    let root = root("truncated");
    let store = ReleaseStore::open(&root).unwrap();
    let pair = store.prepare(&input()).unwrap();
    drop(store);
    fs::remove_file(root.join(CHECKPOINT_FILE)).unwrap();
    let mut journal = fs::OpenOptions::new()
        .append(true)
        .open(root.join(JOURNAL_FILE))
        .unwrap();
    journal.write_all(b"{\"sequence\":").unwrap();
    journal.sync_all().unwrap();
    let recovered = ReleaseStore::open(&root).unwrap();
    assert_eq!(recovered.release(&pair.0.id), Some(pair.0));
    fs::remove_dir_all(root).unwrap();
}
