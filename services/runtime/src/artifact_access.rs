use crate::{
    conversation::RuntimeStore,
    runtime_storage::{ArtifactContent, ArtifactReadError, ArtifactReadRequest, ArtifactStore},
};
use std::sync::Arc;

#[derive(Clone)]
pub struct ArtifactAccessService {
    store: RuntimeStore,
    artifacts: Arc<dyn ArtifactStore>,
}

impl ArtifactAccessService {
    pub fn new(store: RuntimeStore, artifacts: Arc<dyn ArtifactStore>) -> Self {
        Self { store, artifacts }
    }

    pub async fn read_current(
        &self,
        project_id: &str,
        artifact_path: &str,
    ) -> Result<ArtifactContent, ArtifactReadError> {
        let current = self
            .store
            .current_project_version(project_id)
            .await
            .ok_or_else(|| {
                ArtifactReadError::NotFound(format!(
                    "current artifact not found for project: {project_id}"
                ))
            })?;
        let publish = self
            .store
            .artifact_publish_for_version(project_id, &current.created_by_run_id, &current.id)
            .await;
        self.artifacts.read(ArtifactReadRequest {
            project_id,
            version_id: &current.id,
            artifact_path,
            expected_manifest_hash: publish
                .as_ref()
                .and_then(|publish| publish.artifact_manifest_hash.as_deref()),
        })
    }
}
