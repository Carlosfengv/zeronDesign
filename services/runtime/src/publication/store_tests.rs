use super::*;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

fn root(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    std::env::temp_dir().join(format!(
        "publication-store-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn publish(key: &str) -> PublicationIntent {
    PublicationIntent {
        project_id: "project-1".into(),
        workspace_namespace: "ws-project-one".into(),
        kind: PublishOperationKind::Publish,
        release_id: Some("release-1".into()),
        expected_current_release_id: None,
        expected_generation: Some(0),
        runtime_profile_id: "static-web-v1".into(),
        idempotency_key: key.into(),
    }
}

#[test]
fn idempotency_reuses_same_operation_and_rejects_different_body() {
    let root = root("idempotency");
    let store = PublicationStore::open(&root).unwrap();
    let first = store.commit_intent(&publish("key-1")).unwrap();
    assert_eq!(first, store.commit_intent(&publish("key-1")).unwrap());
    assert_eq!(first.1.desired_generation, 1);
    assert_eq!(store.pending_outbox().len(), 1);
    let mut conflicting = publish("key-1");
    conflicting.release_id = Some("release-2".into());
    assert!(matches!(
        store.commit_intent(&conflicting),
        Err(PublicationStoreError::Conflict(_))
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_runtime_and_outbox_recover_atomically_from_journal() {
    let root = root("recovery");
    let store = PublicationStore::open(&root).unwrap();
    let (operation, runtime) = store.commit_intent(&publish("key-recovery")).unwrap();
    drop(store);
    fs::remove_file(root.join(CHECKPOINT_FILE)).unwrap();
    let recovered = PublicationStore::open(&root).unwrap();
    assert_eq!(recovered.operation(&operation.id), Some(operation));
    assert_eq!(recovered.runtime(&runtime.project_id), Some(runtime));
    assert_eq!(recovered.pending_outbox().len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn truncated_uncommitted_tail_never_creates_partial_publication_state() {
    let root = root("truncated-tail");
    let store = PublicationStore::open(&root).unwrap();
    let (operation, runtime) = store.commit_intent(&publish("key-truncated")).unwrap();
    drop(store);
    fs::remove_file(root.join(CHECKPOINT_FILE)).unwrap();
    let mut journal = fs::OpenOptions::new()
        .append(true)
        .open(root.join(JOURNAL_FILE))
        .unwrap();
    journal
        .write_all(b"{\"sequence\":2,\"operation\":")
        .unwrap();
    journal.sync_all().unwrap();
    drop(journal);
    let recovered = PublicationStore::open(&root).unwrap();
    assert_eq!(recovered.operation(&operation.id), Some(operation));
    assert_eq!(recovered.runtime(&runtime.project_id), Some(runtime));
    assert_eq!(recovered.pending_outbox().len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn desired_generation_compare_and_set_serializes_concurrent_requests() {
    let root = root("cas");
    let store = Arc::new(PublicationStore::open(&root).unwrap());
    store.commit_intent(&publish("publish-key")).unwrap();
    let intent = PublicationIntent {
        project_id: "project-1".into(),
        workspace_namespace: "ws-project-one".into(),
        kind: PublishOperationKind::Unpublish,
        release_id: None,
        expected_current_release_id: None,
        expected_generation: Some(1),
        runtime_profile_id: "static-web-v1".into(),
        idempotency_key: "unpublish-key".into(),
    };
    let left = Arc::clone(&store);
    let left_intent = intent.clone();
    let right = Arc::clone(&store);
    let right_intent = PublicationIntent {
        idempotency_key: "other-unpublish-key".into(),
        ..intent
    };
    let first = std::thread::spawn(move || left.commit_intent(&left_intent));
    let second = std::thread::spawn(move || right.commit_intent(&right_intent));
    let results = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(PublicationStoreError::Conflict(_))))
            .count(),
        1
    );
    assert_eq!(store.runtime("project-1").unwrap().desired_generation, 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn delivered_outbox_and_reconciling_checkpoint_persist_together() {
    let root = root("delivery");
    let store = PublicationStore::open(&root).unwrap();
    store.commit_intent(&publish("key-delivery")).unwrap();
    let event = store.pending_outbox().pop().unwrap();
    store.record_delivery_attempt(&event.id, None).unwrap();
    let (operation, _) = store.record_delivered(&event.id).unwrap();
    assert_eq!(operation.status, PublishOperationStatus::Reconciling);
    assert_eq!(operation.checkpoint, PublishCheckpoint::Reconciling);
    assert!(store.pending_outbox().is_empty());
    drop(store);
    let recovered = PublicationStore::open(&root).unwrap();
    assert_eq!(
        recovered.operation(&operation.id).unwrap().status,
        PublishOperationStatus::Reconciling
    );
    assert!(recovered.pending_outbox().is_empty());
    assert_eq!(recovered.replay_nonterminal_outbox().unwrap(), 1);
    assert_eq!(recovered.pending_outbox().len(), 1);
    assert_eq!(
        recovered.operation(&operation.id).unwrap().status,
        PublishOperationStatus::ReconcileRequired
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stale_outbox_cannot_update_a_newer_generation_or_poison_recovery() {
    let root = root("stale-outbox");
    let store = PublicationStore::open(&root).unwrap();
    store.commit_intent(&publish("publish-key")).unwrap();
    let stale_outbox = store.pending_outbox().pop().unwrap();
    let (unpublish, _) = store
        .commit_intent(&PublicationIntent {
            project_id: "project-1".into(),
            workspace_namespace: "ws-project-one".into(),
            kind: PublishOperationKind::Unpublish,
            release_id: None,
            expected_current_release_id: None,
            expected_generation: Some(1),
            runtime_profile_id: "static-web-v1".into(),
            idempotency_key: "unpublish-key".into(),
        })
        .unwrap();

    let pending = store.pending_outbox();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].operation_id, unpublish.id);
    assert!(matches!(
        store.record_reconcile_failure(&stale_outbox.id, "stale failure"),
        Err(PublicationStoreError::Conflict(_))
    ));

    drop(store);
    let recovered = PublicationStore::open(&root).unwrap();
    assert_eq!(
        recovered.runtime("project-1").unwrap().desired_generation,
        2
    );
    assert_eq!(recovered.pending_outbox().len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_commit_is_rejected_before_it_reaches_the_journal() {
    let root = root("invalid-before-append");
    let store = PublicationStore::open(&root).unwrap();
    store.commit_intent(&publish("publish-key")).unwrap();
    let journal_before = fs::read(root.join(JOURNAL_FILE)).unwrap();

    let mut snapshot = store.state.lock().unwrap();
    let operation = snapshot.operations.values().next().unwrap().clone();
    let outbox = snapshot.outbox.values().next().unwrap().clone();
    let mut runtime = snapshot.runtimes.values().next().unwrap().clone();
    runtime.desired_generation += 1;
    let scoped_key = framed_hash(&[
        operation.project_id.as_str(),
        operation.idempotency_key_hash.as_str(),
    ]);
    assert!(matches!(
        persist_commit(
            &store.root,
            None,
            &mut snapshot,
            operation,
            runtime,
            outbox,
            scoped_key,
        ),
        Err(PublicationStoreError::Storage(_))
    ));
    drop(snapshot);

    assert_eq!(fs::read(root.join(JOURNAL_FILE)).unwrap(), journal_before);
    drop(store);
    PublicationStore::open(&root).unwrap();
    fs::remove_dir_all(root).unwrap();
}
