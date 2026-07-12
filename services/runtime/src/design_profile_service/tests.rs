use super::{DesignProfileService, DesignProfileServiceError, RunProfileContextQuery};
use crate::{conversation::RuntimeStore, types::AgentPhase};

#[tokio::test]
async fn application_service_validates_identifiers_without_http_types() {
    let service = DesignProfileService::new(RuntimeStore::new());

    let error = service.get(" ").await.unwrap_err();

    assert_eq!(
        error,
        DesignProfileServiceError::InvalidRequest("designProfileId must not be empty".to_string())
    );
}

#[tokio::test]
async fn run_context_resolution_fails_closed_for_missing_explicit_profile() {
    let service = DesignProfileService::new(RuntimeStore::new());

    let error = service
        .prepare_run_context(RunProfileContextQuery {
            project_id: "project-1",
            workspace_id: None,
            organization_id: None,
            explicit_profile_id: Some("profile-missing"),
            phase: AgentPhase::Edit,
            brief_id: None,
        })
        .await
        .unwrap_err();

    assert!(matches!(error, DesignProfileServiceError::NotFound(message)
        if message == "design profile not found: profile-missing"));
}
