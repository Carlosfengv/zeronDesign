use super::{internal, RunLifecycleError, RunLifecycleService};
use crate::types::{AgentEvent, AgentRun, AgentRunStatus};
use chrono::Utc;

impl RunLifecycleService {
    pub(super) async fn created_run_step<T>(
        &self,
        run: &AgentRun,
        stage: &str,
        result: anyhow::Result<T>,
        map_error: fn(anyhow::Error) -> RunLifecycleError,
    ) -> Result<T, RunLifecycleError> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => Err(self
                .compensate_created_run_error(run, stage, map_error(error))
                .await),
        }
    }

    pub(super) async fn compensate_created_run_error(
        &self,
        run: &AgentRun,
        stage: &str,
        mapped: RunLifecycleError,
    ) -> RunLifecycleError {
        self.store
            .append_audit_record(
                &run.project_id,
                &run.id,
                "run.start",
                format!("stage={stage}"),
                "deny",
                "StartRun compensated after durable run creation",
            )
            .await;
        if let Err(error) = self
            .store
            .update_run_status(&run.id, AgentRunStatus::Cancelled)
            .await
        {
            return internal(error);
        }
        if let Err(error) = self
            .store
            .append_event(AgentEvent::RunCompleted {
                run_id: run.id.clone(),
                status: "cancelled".to_string(),
                summary: format!("StartRun failed during {stage}."),
                timestamp: Utc::now(),
            })
            .await
        {
            return internal(error);
        }
        mapped
    }

    pub(super) async fn register_start_session(
        &self,
        run: &AgentRun,
    ) -> Result<(), RunLifecycleError> {
        let Err(error) = self.launch_session(run.id.clone()) else {
            return Ok(());
        };
        self.store
            .append_audit_record(
                &run.project_id,
                &run.id,
                "run.session.register",
                "state=queued",
                "ask",
                "session registration failed; startup recovery may retry",
            )
            .await;
        self.store
            .append_event(AgentEvent::StateChanged {
                run_id: run.id.clone(),
                state: "queued:session_registration_failed".to_string(),
                timestamp: Utc::now(),
            })
            .await
            .map_err(internal)?;
        Err(internal(error))
    }
}
