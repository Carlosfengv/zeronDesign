use anydesign_runtime::conversation::RuntimeStore;

fn test_storage_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "zerondesign-draft-snapshot-store-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[tokio::test]
async fn draft_snapshots_are_idempotent_and_survive_runtime_restart() {
    let storage = test_storage_dir();
    let store = RuntimeStore::with_checkpoint_dir(&storage);
    let source_hash = "a".repeat(64);
    let design_context_hash = "b".repeat(64);

    let created = store
        .create_draft_snapshot(
            "project-1",
            "object://source-snapshots/project-1/source-1.tar.zst".to_string(),
            source_hash.clone(),
            "next-app".to_string(),
            "next-app@1".to_string(),
            "dependency-policy@1".to_string(),
            design_context_hash.clone(),
            "run-build-1",
            None,
            None,
        )
        .await
        .unwrap();

    let replayed = store
        .create_draft_snapshot(
            "project-1",
            "object://source-snapshots/project-1/source-1.tar.zst".to_string(),
            source_hash,
            "next-app".to_string(),
            "next-app@1".to_string(),
            "dependency-policy@1".to_string(),
            design_context_hash,
            "run-build-1",
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(replayed.snapshot_id, created.snapshot_id);

    let restarted = RuntimeStore::with_checkpoint_dir(&storage);
    let recovered = restarted
        .get_draft_snapshot(&created.snapshot_id)
        .await
        .unwrap();
    assert_eq!(recovered, created);
    assert_eq!(
        restarted.list_project_draft_snapshots("project-1").await,
        vec![created]
    );

    std::fs::remove_dir_all(storage).unwrap();
}

#[tokio::test]
async fn draft_snapshot_rejects_a_different_source_for_the_same_successful_run() {
    let storage = test_storage_dir();
    let store = RuntimeStore::with_checkpoint_dir(&storage);

    store
        .create_draft_snapshot(
            "project-1",
            "object://source-snapshots/project-1/source-1.tar.zst".to_string(),
            "a".repeat(64),
            "next-app".to_string(),
            "next-app@1".to_string(),
            "dependency-policy@1".to_string(),
            "b".repeat(64),
            "run-build-1",
            None,
            None,
        )
        .await
        .unwrap();

    let error = store
        .create_draft_snapshot(
            "project-1",
            "object://source-snapshots/project-1/source-2.tar.zst".to_string(),
            "c".repeat(64),
            "next-app".to_string(),
            "next-app@1".to_string(),
            "dependency-policy@1".to_string(),
            "b".repeat(64),
            "run-build-1",
            None,
            None,
        )
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("already persisted a different source"));

    std::fs::remove_dir_all(storage).unwrap();
}
