use super::{RunLifecycleService, RunSessionLauncher};
use crate::{
    config::RuntimeConfig,
    conversation::RuntimeStore,
    types::{AgentPhase, AgentRunStatus},
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct RecordingSessionLauncher {
    launched: Mutex<Vec<String>>,
}

impl RunSessionLauncher for RecordingSessionLauncher {
    fn launch(&self, run_id: String) {
        self.launched.lock().unwrap().push(run_id);
    }
}

#[tokio::test]
async fn continue_run_uses_injected_session_launcher() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let launcher = Arc::new(RecordingSessionLauncher::default());
    let service = RunLifecycleService::new(RuntimeConfig::from_env(), store, launcher.clone());

    let outcome = service
        .continue_run(&run.id, "Continue".to_string())
        .await
        .unwrap();

    assert_eq!(outcome.status, "running");
    assert_eq!(launcher.launched.lock().unwrap().as_slice(), [run.id]);
}
