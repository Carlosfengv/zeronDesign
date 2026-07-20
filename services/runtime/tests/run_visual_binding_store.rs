use anydesign_runtime::{
    types::AgentPhase,
    visual_contracts::{RunVisualBinding, RunVisualBindingRole, RunVisualTarget, VisualViewport},
    RuntimeStore,
};

#[tokio::test]
async fn run_visual_bindings_are_idempotent_ordered_and_restart_safe() {
    let root = std::env::temp_dir().join(format!(
        "runtime-run-visual-bindings-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    let store = RuntimeStore::with_checkpoint_dir(&root);
    let run = store
        .create_run(
            "project-visual".to_string(),
            AgentPhase::Review,
            "visual-review".to_string(),
            "resource:vision-model".to_string(),
            vec![],
        )
        .await;
    let binding = RunVisualBinding {
        run_id: run.id.clone(),
        artifact_id: "visual-reference".to_string(),
        role: RunVisualBindingRole::Reference,
        route: "/".to_string(),
        viewport: VisualViewport {
            width: 1440,
            height: 900,
            device_scale_factor: 1.0,
        },
        target: RunVisualTarget::StaticSnapshot {
            snapshot_id: "draft-snapshot-1".to_string(),
            source_hash: "a".repeat(64),
        },
        order: 0,
    };
    assert_eq!(
        store
            .upsert_run_visual_binding(binding.clone())
            .await
            .unwrap(),
        binding
    );
    assert_eq!(
        store
            .upsert_run_visual_binding(binding.clone())
            .await
            .unwrap(),
        binding
    );

    let restarted = RuntimeStore::with_checkpoint_dir(&root);
    assert_eq!(
        restarted.run_visual_bindings(&run.id).await.unwrap(),
        vec![binding]
    );
    std::fs::remove_dir_all(root).unwrap();
}
