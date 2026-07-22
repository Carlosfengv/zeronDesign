use anydesign_runtime::{types::AuditRecord, RuntimeStore};
use std::{
    collections::HashSet,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_control_plane_appends_remain_complete_jsonl_records() {
    let root = unique_temp_dir("control-plane-concurrency");
    let store = RuntimeStore::with_checkpoint_dir(&root);
    let mut tasks = Vec::new();

    for index in 0..32 {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            store
                .append_audit_record(
                    &format!("project-{index}"),
                    &format!("run-{index}"),
                    "test.concurrent_append",
                    format!("index={index}"),
                    "allow",
                    "concurrency regression fixture",
                )
                .await
        }));
    }

    for task in tasks {
        task.await.expect("audit append task should not panic");
    }

    let records = fs::read_to_string(store.audit_log_path())
        .expect("audit journal should be readable")
        .lines()
        .map(|line| serde_json::from_str::<AuditRecord>(line).expect("journal line should be JSON"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 32);
    assert_eq!(
        records
            .iter()
            .map(|record| record.project_id.as_str())
            .collect::<HashSet<_>>()
            .len(),
        32
    );

    fs::remove_dir_all(root).expect("temporary control-plane store should be removable");
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("anydesign-{prefix}-{}-{nonce}", std::process::id()))
}
