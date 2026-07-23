use crate::{agent_loop::AgentLoop, conversation::RuntimeStore, model_gateway::ModelClient};
use anyhow::Result;
use std::sync::Arc;

#[derive(Clone)]
pub struct QuerySession {
    loop_runner: AgentLoop,
}

impl QuerySession {
    pub fn new(store: RuntimeStore, model: Arc<dyn ModelClient>) -> Self {
        Self {
            loop_runner: AgentLoop::new(store, model),
        }
    }

    pub fn with_tool_executor(
        store: RuntimeStore,
        model: Arc<dyn ModelClient>,
        tool_executor: crate::tools::runtime::ToolExecutor,
    ) -> Self {
        Self {
            loop_runner: AgentLoop::with_tool_executor(store, model, tool_executor),
        }
    }

    pub fn with_tool_executor_and_generation_context(
        store: RuntimeStore,
        model: Arc<dyn ModelClient>,
        tool_executor: crate::tools::runtime::ToolExecutor,
        generation_context_enabled: bool,
        observation_receipts_enabled: bool,
        run_budget_profile: Option<crate::types::RunBudgetProfile>,
    ) -> Result<Self> {
        Ok(Self {
            loop_runner: AgentLoop::with_tool_executor(store, model, tool_executor)
                .with_generation_context_enabled(generation_context_enabled)
                .with_observation_receipts_enabled(observation_receipts_enabled)
                .with_run_budget_profile(run_budget_profile)?,
        })
    }

    pub async fn submit_run(&self, run_id: &str) -> Result<()> {
        self.loop_runner.run(run_id).await?;
        Ok(())
    }
}
