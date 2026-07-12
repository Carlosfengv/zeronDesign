mod cancel;
mod continue_run;
mod permission;

#[cfg(test)]
mod tests;

pub use permission::PermissionDecision;

use crate::{config::RuntimeConfig, conversation::RuntimeStore};
use std::{error::Error, fmt, sync::Arc};

pub trait RunSessionLauncher: Send + Sync {
    fn launch(&self, run_id: String);
}

#[derive(Clone)]
pub struct RunLifecycleService {
    config: RuntimeConfig,
    store: RuntimeStore,
    session_launcher: Arc<dyn RunSessionLauncher>,
}

impl RunLifecycleService {
    pub fn new(
        config: RuntimeConfig,
        store: RuntimeStore,
        session_launcher: Arc<dyn RunSessionLauncher>,
    ) -> Self {
        Self {
            config,
            store,
            session_launcher,
        }
    }

    fn launch_session(&self, run_id: String) {
        self.session_launcher.launch(run_id);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLifecycleOutcome {
    pub run_id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunLifecycleError {
    NotFound(String),
    Conflict(String),
    Internal(String),
}

impl fmt::Display for RunLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(message) | Self::Conflict(message) | Self::Internal(message) => {
                formatter.write_str(message)
            }
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
