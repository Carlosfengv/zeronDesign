use super::RuntimeSupervisor;
use crate::{http_api, RuntimeConfig};

pub struct RecoveredRuntime {
    pub state: http_api::AppState,
    pub supervisor: RuntimeSupervisor,
}

pub struct RuntimeBootstrap {
    config: RuntimeConfig,
}

impl RuntimeBootstrap {
    pub fn new(config: RuntimeConfig) -> Self {
        Self { config }
    }

    pub async fn recover(self) -> anyhow::Result<RecoveredRuntime> {
        self.config.validate_startup().map_err(anyhow::Error::msg)?;
        let supervisor = RuntimeSupervisor::new();
        let state = http_api::recover_startup_runs(http_api::app_state(self.config)).await?;
        supervisor.mark_recovered();
        Ok(RecoveredRuntime { state, supervisor })
    }
}
