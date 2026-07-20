use super::*;
use crate::{
    artifact_access::ArtifactAccessService,
    authorization::ApplicationAuthorizationPolicy,
    design_profile_service::DesignProfileService,
    preview_access::PreviewAccessService,
    release_evidence::ReleaseEvidenceService,
    runtime_storage::{FileArtifactStore, FileRuntimeEvidenceStore, RuntimeEvidenceStore},
};

pub(super) fn router_with_services(state: AppState) -> Router {
    let design_profiles = DesignProfileService::new(state.store.clone());
    let run_lifecycle = run_lifecycle_service(&state, design_profiles.clone());
    let artifact_access = ArtifactAccessService::new(
        state.store.clone(),
        Arc::new(FileArtifactStore::new(&state.config.runtime_storage_dir)),
    );
    let runtime_evidence: Arc<dyn RuntimeEvidenceStore> = Arc::new(FileRuntimeEvidenceStore::new(
        &state.config.runtime_storage_dir,
    ));
    let release_evidence = ReleaseEvidenceService::new(state.store.clone(), runtime_evidence);
    let authorization = ApplicationAuthorizationPolicy::new(state.store.clone());
    let preview_access = PreviewAccessService::new(state.store.clone(), authorization.clone());
    let visual_review = VisualReviewService::new(
        state.store.clone(),
        state.model.clone(),
        state.config.clone(),
    );
    let publish_workflow = Arc::new(PublishWorkflowService::new(
        state.store.clone(),
        state.config.clone(),
    ));
    Router::new()
        .merge(routes::system::router())
        .merge(routes::briefs::router())
        .merge(routes::runs::router())
        .merge(routes::run_events::router())
        .merge(routes::design_sources::router())
        .merge(routes::draft_preview_events::router())
        .merge(routes::design_profiles::router())
        .merge(routes::projects::router())
        .merge(routes::previews::router())
        .merge(routes::publication::router())
        .merge(routes::artifacts::router())
        .merge(routes::internal::router())
        .layer(Extension(preview_access))
        .layer(Extension(visual_review))
        .layer(Extension(publish_workflow))
        .layer(Extension(authorization))
        .layer(Extension(release_evidence))
        .layer(Extension(artifact_access))
        .layer(Extension(run_lifecycle))
        .layer(Extension(design_profiles))
        .with_state(state)
}

pub(super) fn capture_router_with_services(state: AppState) -> Router {
    let authorization = ApplicationAuthorizationPolicy::new(state.store.clone());
    let preview_access = PreviewAccessService::new(state.store.clone(), authorization);
    routes::capture::router()
        .layer(Extension(preview_access))
        .with_state(state)
}
