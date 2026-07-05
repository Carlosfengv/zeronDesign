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

    pub async fn submit_run(&self, run_id: &str) -> Result<()> {
        self.loop_runner.run(run_id).await?;
        Ok(())
    }
}
