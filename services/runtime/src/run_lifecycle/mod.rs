mod cancel;
mod continue_run;
mod permission;
mod profile_token_sync;
mod start;
mod start_failure;
mod start_validation;

#[cfg(test)]
mod tests;

pub use permission::PermissionDecision;
pub use start::{StartRunCommand, StartRunContext};

use crate::{
    config::RuntimeConfig, conversation::RuntimeStore, design_profile_service::DesignProfileService,
};
use async_trait::async_trait;
use std::{error::Error, fmt, sync::Arc};

pub trait RunSessionLauncher: Send + Sync {
    fn launch(&self, run_id: String) -> anyhow::Result<()>;
}

#[async_trait]
pub trait BuildSandboxProvisioner: Send + Sync {
    async fn provision_ready(
        &self,
        store: &RuntimeStore,
        project_id: &str,
        template_key: &str,
    ) -> anyhow::Result<crate::types::SandboxBinding>;
}

#[async_trait]
pub trait EditWorkspaceRestorer: Send + Sync {
    async fn restore(
        &self,
        store: &RuntimeStore,
        config: &RuntimeConfig,
        run: &crate::types::AgentRun,
        source_snapshot_uri: &str,
    ) -> anyhow::Result<()>;
}

#[derive(Clone)]
pub struct RunLifecycleService {
    config: RuntimeConfig,
    store: RuntimeStore,
    session_launcher: Arc<dyn RunSessionLauncher>,
    sandbox_provisioner: Arc<dyn BuildSandboxProvisioner>,
    edit_workspace_restorer: Arc<dyn EditWorkspaceRestorer>,
    design_profiles: DesignProfileService,
}

impl RunLifecycleService {
    pub fn new(
        config: RuntimeConfig,
        store: RuntimeStore,
        session_launcher: Arc<dyn RunSessionLauncher>,
        sandbox_provisioner: Arc<dyn BuildSandboxProvisioner>,
        edit_workspace_restorer: Arc<dyn EditWorkspaceRestorer>,
        design_profiles: DesignProfileService,
    ) -> Self {
        Self {
            config,
            store,
            session_launcher,
            sandbox_provisioner,
            edit_workspace_restorer,
            design_profiles,
        }
    }

    fn launch_session(&self, run_id: String) -> anyhow::Result<()> {
        self.session_launcher.launch(run_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLifecycleOutcome {
    pub run_id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunLifecycleError {
    InvalidRequest(String),
    NotFound(String),
    Conflict(String),
    Internal(String),
}

impl fmt::Display for RunLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message)
            | Self::NotFound(message)
            | Self::Conflict(message)
            | Self::Internal(message) => formatter.write_str(message),
        }
    }
}

impl Error for RunLifecycleError {}

fn update_error(error: anyhow::Error) -> RunLifecycleError {
    let message = error.to_string();
    if message.contains("run not found") {
        RunLifecycleError::NotFound(message)
    } else {
        RunLifecycleError::Conflict(message)
    }
}

fn conflict(error: anyhow::Error) -> RunLifecycleError {
    RunLifecycleError::Conflict(error.to_string())
}

fn internal(error: impl fmt::Display) -> RunLifecycleError {
    RunLifecycleError::Internal(error.to_string())
}

fn invalid_request(message: String) -> RunLifecycleError {
    RunLifecycleError::InvalidRequest(message)
}

fn not_found(message: String) -> RunLifecycleError {
    RunLifecycleError::NotFound(message)
}

fn sandbox_binding_error(error: anyhow::Error) -> RunLifecycleError {
    let message = error.to_string();
    if message.contains("sandbox binding not found") {
        not_found(message)
    } else {
        RunLifecycleError::Conflict(message)
    }
}

fn design_profile_error(error: anyhow::Error) -> RunLifecycleError {
    let message = error.to_string();
    if message.contains("design profile not found") {
        not_found(message)
    } else if message.contains("invalid design profile") {
        invalid_request(message)
    } else {
        RunLifecycleError::Conflict(message)
    }
}

fn repair_run_error(error: anyhow::Error) -> RunLifecycleError {
    let message = error.to_string();
    if message.contains("parent run not found") || message.contains("review finding not found") {
        not_found(message)
    } else {
        RunLifecycleError::Conflict(message)
    }
}

fn profile_service_error(
    error: crate::design_profile_service::DesignProfileServiceError,
) -> RunLifecycleError {
    use crate::design_profile_service::DesignProfileServiceError;
    match error {
        DesignProfileServiceError::InvalidRequest(message) => {
            RunLifecycleError::InvalidRequest(message)
        }
        DesignProfileServiceError::NotFound(message) => RunLifecycleError::NotFound(message),
        DesignProfileServiceError::Conflict(message)
        | DesignProfileServiceError::ActivationConflict { message, .. } => {
            RunLifecycleError::Conflict(message)
        }
        DesignProfileServiceError::Internal(message) => RunLifecycleError::Internal(message),
    }
}
