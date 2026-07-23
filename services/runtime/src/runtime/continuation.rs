use crate::{
    config::ContinuationMode,
    run_lifecycle::RunLifecycleService,
    runtime::{RuntimeSupervisor, SupervisorError},
    types::{AgentEvent, AgentPhase, AgentRun, AgentRunStatus},
    RuntimeConfig, RuntimeStore,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

pub struct RunContinuationController {
    config: RuntimeConfig,
    store: RuntimeStore,
    lifecycle: Arc<RunLifecycleService>,
    interval: Duration,
}

impl RunContinuationController {
    pub fn new(
        config: RuntimeConfig,
        store: RuntimeStore,
        lifecycle: Arc<RunLifecycleService>,
        interval: Duration,
    ) -> Self {
        Self {
            config,
            store,
            lifecycle,
            interval,
        }
    }

    pub fn spawn(self, supervisor: &RuntimeSupervisor) -> Result<(), SupervisorError> {
        if self.config.continuation_mode == ContinuationMode::Off {
            return Ok(());
        }
        supervisor.spawn_with_shutdown(
            "controller/run-continuation",
            true,
            move |mut shutdown| async move {
                loop {
                    self.reconcile_once().await;
                    tokio::select! {
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() { break; }
                        }
                        _ = tokio::time::sleep(self.interval) => {}
                    }
                }
                Ok(())
            },
        )
    }

    pub async fn reconcile_once(&self) -> usize {
        if self.config.continuation_mode == ContinuationMode::Off {
            return 0;
        }
        let Ok(runs) = self.store.all_runs().await else {
            return 0;
        };
        let mut processed = 0;
        for run in runs.into_iter().filter(|run| {
            run.status == AgentRunStatus::Partial
                && run.successor_run_id.is_none()
                && run.continuation_snapshot_id.is_some()
        }) {
            let snapshot_id = run
                .continuation_snapshot_id
                .as_deref()
                .expect("continuation candidate has snapshot identity");
            let decision = match self
                .store
                .evaluate_run_continuation_snapshot(&self.config.runtime_storage_dir, snapshot_id)
                .await
            {
                Ok(decision) => decision,
                Err(error) => {
                    self.record_decision_once(
                        &run.id,
                        snapshot_id,
                        "evaluation_error",
                        json!({ "error": error.to_string() }),
                    )
                    .await;
                    processed += 1;
                    continue;
                }
            };
            if !decision.eligible {
                self.record_decision_once(
                    &run.id,
                    snapshot_id,
                    "rejected",
                    json!({ "reasons": decision.reasons }),
                )
                .await;
                processed += 1;
                continue;
            }
            if self.config.continuation_mode == ContinuationMode::Shadow {
                self.record_decision_once(
                    &run.id,
                    snapshot_id,
                    "eligible_shadow",
                    json!({ "operationId": decision.operation_id }),
                )
                .await;
                processed += 1;
                continue;
            }
            match continuation_allowlisted(&self.config, &run) {
                Ok(true) => {}
                Ok(false) => {
                    self.record_decision_once(
                        &run.id,
                        snapshot_id,
                        "not_allowlisted",
                        json!({
                            "phase": run.phase,
                            "agentProfile": run.agent_profile,
                        }),
                    )
                    .await;
                    processed += 1;
                    continue;
                }
                Err(error) => {
                    self.record_decision_once(
                        &run.id,
                        snapshot_id,
                        "allowlist_invalid",
                        json!({ "error": error }),
                    )
                    .await;
                    processed += 1;
                    continue;
                }
            }
            match self
                .lifecycle
                .dispatch_continuation_successor(snapshot_id)
                .await
            {
                Ok(outcome) => {
                    self.record_decision_once(
                        &run.id,
                        snapshot_id,
                        "dispatched",
                        json!({ "successorRunId": outcome.run_id }),
                    )
                    .await;
                }
                Err(error) => {
                    self.record_decision_once(
                        &run.id,
                        snapshot_id,
                        "dispatch_error",
                        json!({ "error": error.to_string() }),
                    )
                    .await;
                }
            }
            processed += 1;
        }
        processed
    }

    async fn record_decision_once(
        &self,
        run_id: &str,
        snapshot_id: &str,
        outcome: &str,
        details: Value,
    ) {
        let mode = continuation_mode_name(self.config.continuation_mode);
        let already_recorded = self.store.events(run_id).await.iter().any(|event| {
            matches!(event,
                AgentEvent::MetricRecorded { name, metadata: Some(metadata), .. }
                    if name == "run.continuation_decision"
                        && metadata.get("continuationSnapshotId").and_then(Value::as_str)
                            == Some(snapshot_id)
                        && metadata.get("mode").and_then(Value::as_str) == Some(mode)
                        && metadata.get("outcome").and_then(Value::as_str) == Some(outcome)
            )
        });
        if already_recorded {
            return;
        }
        let _ = self
            .store
            .append_event(AgentEvent::MetricRecorded {
                run_id: run_id.to_string(),
                name: "run.continuation_decision".to_string(),
                value: 1,
                metadata: Some(json!({
                    "continuationSnapshotId": snapshot_id,
                    "mode": mode,
                    "outcome": outcome,
                    "details": details,
                })),
                timestamp: Utc::now(),
            })
            .await;
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContinuationAllowlistEntry {
    phase: AgentPhase,
    agent_profile: String,
}

fn continuation_allowlisted(config: &RuntimeConfig, run: &AgentRun) -> Result<bool, String> {
    let raw = config
        .continuation_allowlist_json
        .as_deref()
        .ok_or_else(|| "continuation allowlist is missing".to_string())?;
    let entries = serde_json::from_str::<Vec<ContinuationAllowlistEntry>>(raw)
        .map_err(|error| format!("continuation allowlist is invalid: {error}"))?;
    Ok(entries
        .iter()
        .any(|entry| entry.phase == run.phase && entry.agent_profile == run.agent_profile))
}

fn continuation_mode_name(mode: ContinuationMode) -> &'static str {
    match mode {
        ContinuationMode::Off => "off",
        ContinuationMode::Shadow => "shadow",
        ContinuationMode::Enforced => "enforced",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        agent_loop::progress_ledger_fingerprint,
        artifact_publisher::{source_snapshot_fingerprint, ArtifactFile, FileArtifactPublisher},
        config::SandboxBackendMode,
        design_profile_service::DesignProfileService,
        run_lifecycle::{BuildSandboxProvisioner, EditWorkspaceRestorer, RunSessionLauncher},
        types::{AgentPhase, AgentRun},
    };
    use async_trait::async_trait;
    use std::{path::PathBuf, sync::Mutex};

    struct RecordingLauncher(Arc<Mutex<Vec<String>>>);

    impl RunSessionLauncher for RecordingLauncher {
        fn launch(&self, run_id: String) -> anyhow::Result<()> {
            self.0.lock().unwrap().push(run_id);
            Ok(())
        }
    }

    struct UnusedProvisioner;

    #[async_trait]
    impl BuildSandboxProvisioner for UnusedProvisioner {
        async fn provision_ready(
            &self,
            _store: &RuntimeStore,
            _project_id: &str,
            _template_key: &str,
        ) -> anyhow::Result<crate::types::SandboxBinding> {
            anyhow::bail!("sandbox provisioner must not be used by Brief continuation test")
        }
    }

    struct UnusedRestorer;

    #[async_trait]
    impl EditWorkspaceRestorer for UnusedRestorer {
        async fn prepare_build(
            &self,
            _store: &RuntimeStore,
            _config: &RuntimeConfig,
            _run: &AgentRun,
        ) -> anyhow::Result<()> {
            anyhow::bail!("workspace restorer must not be used by Brief continuation test")
        }

        async fn restore(
            &self,
            _store: &RuntimeStore,
            _config: &RuntimeConfig,
            _run: &AgentRun,
            _source_snapshot_uri: &str,
        ) -> anyhow::Result<()> {
            anyhow::bail!("workspace restorer must not be used by Brief continuation test")
        }
    }

    #[tokio::test]
    async fn enforced_controller_dispatches_exactly_one_successor() {
        let root = std::env::temp_dir().join(format!(
            "anydesign-continuation-controller-test-{}",
            rand::random::<u64>()
        ));
        let runtime_storage_dir = root.join("runtime-storage");
        let checkpoint_dir = root.join("checkpoints");
        let run_log_dir = root.join("run-log");
        let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
        let run = store
            .create_run(
                "project-1".to_string(),
                AgentPhase::Brief,
                "brief".to_string(),
                "fixture".to_string(),
                Vec::new(),
            )
            .await;
        let files = vec![ArtifactFile {
            path: PathBuf::from("brief.md"),
            bytes: b"# Frozen brief".to_vec(),
        }];
        let source_hash = source_snapshot_fingerprint(&files).unwrap();
        let source_snapshot_uri = FileArtifactPublisher::new(&runtime_storage_dir)
            .publish_source_snapshot("project-1", "partial-brief-1", files)
            .await
            .unwrap();
        let checkpoint = store.ensure_initial_checkpoint(&run.id).await.unwrap();
        assert_eq!(checkpoint.run_id, run.id);
        let progress_ledger = json!({
            "schemaVersion": "substantive-progress-ledger@1",
            "state": {}
        });
        store
            .append_event(AgentEvent::RunProgressFingerprint {
                run_id: run.id.clone(),
                turn: 1,
                fingerprint: progress_ledger_fingerprint(&progress_ledger).unwrap(),
                consecutive_no_progress: 0,
                evidence: progress_ledger,
                timestamp: Utc::now(),
            })
            .await
            .unwrap();
        store
            .append_event(AgentEvent::RunWorkflowProgress {
                run_id: run.id.clone(),
                turn: 1,
                stage: "brief_drafting".to_string(),
                completed_steps: vec!["brief.write_draft".to_string()],
                next_action: json!({ "tool": "run.complete" }),
                budgets: json!({ "turnsRemaining": 2 }),
                timestamp: Utc::now(),
            })
            .await
            .unwrap();
        store
            .update_run_status(&run.id, AgentRunStatus::Partial)
            .await
            .unwrap();
        let snapshot = store
            .create_run_continuation_snapshot(
                &runtime_storage_dir,
                &run.id,
                &source_snapshot_uri,
                &source_hash,
                json!({
                    "schemaVersion": "remaining-operation-budget@1",
                    "grossInputTokens": 1000,
                    "uncachedInputTokens": 800,
                    "outputTokens": 200,
                    "turns": 2,
                    "toolCalls": 4
                }),
                "Finish the already drafted brief without repeating diagnostics.",
            )
            .await
            .unwrap();

        let mut config = RuntimeConfig::from_env();
        config.continuation_mode = ContinuationMode::Enforced;
        config.continuation_allowlist_json =
            Some(r#"[{"phase":"brief","agentProfile":"brief"}]"#.to_string());
        assert_eq!(continuation_allowlisted(&config, &run), Ok(true));
        let mut denied_config = config.clone();
        denied_config.continuation_allowlist_json = Some("[]".to_string());
        assert_eq!(continuation_allowlisted(&denied_config, &run), Ok(false));
        denied_config.continuation_allowlist_json = Some("not-json".to_string());
        assert!(continuation_allowlisted(&denied_config, &run).is_err());
        config.runtime_storage_dir = runtime_storage_dir;
        config.workspace_root = root.join("workspace");
        config.sandbox_backend_mode = SandboxBackendMode::PhaseAContract;
        let restart_config = config.clone();
        let launches = Arc::new(Mutex::new(Vec::new()));
        let lifecycle = Arc::new(RunLifecycleService::new(
            config.clone(),
            store.clone(),
            Arc::new(RecordingLauncher(launches.clone())),
            Arc::new(UnusedProvisioner),
            Arc::new(UnusedRestorer),
            DesignProfileService::new(store.clone()),
        ));
        let controller = RunContinuationController::new(
            config,
            store.clone(),
            lifecycle,
            Duration::from_millis(1),
        );

        let (first, concurrent) =
            tokio::join!(controller.reconcile_once(), controller.reconcile_once());
        assert!((1..=2).contains(&(first + concurrent)));
        assert_eq!(controller.reconcile_once().await, 0);
        let predecessor = store.get_run(&run.id).await.unwrap();
        let successor_id = predecessor.successor_run_id.unwrap();
        let successor = store.get_run(&successor_id).await.unwrap();
        assert_eq!(
            successor.continuation_snapshot_id.as_deref(),
            Some(snapshot.snapshot_id.as_str())
        );
        assert_eq!(successor.operation_id, run.operation_id);
        assert_eq!(successor.operation_attempt, 2);
        assert_eq!(launches.lock().unwrap().as_slice(), &[successor_id]);
        assert!(store
            .events(&successor.id)
            .await
            .iter()
            .any(|event| matches!(
                event,
                AgentEvent::RunContinuationCreated {
                    automatic: true,
                    ..
                }
            )));

        let recovered_store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
        let recovered_launches = Arc::new(Mutex::new(Vec::new()));
        let recovered_lifecycle = Arc::new(RunLifecycleService::new(
            restart_config.clone(),
            recovered_store.clone(),
            Arc::new(RecordingLauncher(recovered_launches.clone())),
            Arc::new(UnusedProvisioner),
            Arc::new(UnusedRestorer),
            DesignProfileService::new(recovered_store.clone()),
        ));
        let recovered_controller = RunContinuationController::new(
            restart_config,
            recovered_store.clone(),
            recovered_lifecycle,
            Duration::from_millis(1),
        );
        assert_eq!(recovered_controller.reconcile_once().await, 0);
        let recovered_predecessor = recovered_store.get_run(&run.id).await.unwrap();
        assert_eq!(
            recovered_predecessor.successor_run_id.as_deref(),
            Some(successor.id.as_str())
        );
        assert!(recovered_launches.lock().unwrap().is_empty());
    }
}
