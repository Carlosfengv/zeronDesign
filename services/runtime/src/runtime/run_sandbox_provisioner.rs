use crate::{
    conversation::RuntimeStore, run_lifecycle::BuildSandboxProvisioner,
    tools::control_plane::SandboxBackend, types::SandboxBinding,
};
use async_trait::async_trait;
use std::sync::Arc;

pub struct RuntimeBuildSandboxProvisioner {
    backend: Arc<dyn SandboxBackend>,
}

impl RuntimeBuildSandboxProvisioner {
    pub fn new(backend: Arc<dyn SandboxBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl BuildSandboxProvisioner for RuntimeBuildSandboxProvisioner {
    async fn provision_ready(
        &self,
        store: &RuntimeStore,
        project_id: &str,
        template_key: &str,
    ) -> anyhow::Result<SandboxBinding> {
        let binding = self.backend.claim(store, project_id, template_key).await?;
        match self
            .backend
            .wait_ready(store, &binding.id, Some(120_000))
            .await
        {
            Ok(binding) => Ok(binding),
            Err(error) => {
                let _ = self.backend.release(store, &binding.id).await;
                Err(error)
            }
        }
    }
}
