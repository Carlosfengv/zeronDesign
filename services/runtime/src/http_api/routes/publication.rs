use super::super::*;
use crate::{
    publication::{PublicationIntent, PublicationStoreError, PublishOperationKind},
    release::WorkReleaseStatus,
};

const PUBLICATION_BODY_LIMIT_BYTES: usize = 16 * 1024;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project_id}/publish", post(publish_work))
        .route("/projects/{project_id}/unpublish", post(unpublish_work))
        .route("/projects/{project_id}/rollback", post(rollback_work))
        .route(
            "/projects/{project_id}/deployment-state",
            get(deployment_state),
        )
        .route("/projects/{project_id}/releases", get(work_releases))
        .route("/operations/{operation_id}", get(publication_operation))
        .layer(DefaultBodyLimit::max(PUBLICATION_BODY_LIMIT_BYTES))
}

async fn publish_work(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<PublishWorkRequest>,
) -> Result<(StatusCode, Json<PublicationOperationResponse>), (StatusCode, Json<ErrorResponse>)> {
    authorize_publication(&state, &headers, &project_id, PUBLICATION_WRITE_OPERATION).await?;
    validate_release_target(
        &state,
        &project_id,
        &request.release_id,
        &request.runtime_profile_id,
    )
    .await?;
    let kind = if request.expected_current_release_id.is_some() {
        PublishOperationKind::Update
    } else {
        PublishOperationKind::Publish
    };
    commit_publication_intent(
        &state,
        PublicationIntent {
            project_id,
            kind,
            release_id: Some(request.release_id),
            expected_current_release_id: request.expected_current_release_id,
            expected_generation: request.expected_generation,
            runtime_profile_id: request.runtime_profile_id,
            idempotency_key: idempotency_key(&headers)?,
        },
    )
    .await
}

async fn rollback_work(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<PublishWorkRequest>,
) -> Result<(StatusCode, Json<PublicationOperationResponse>), (StatusCode, Json<ErrorResponse>)> {
    authorize_publication(&state, &headers, &project_id, PUBLICATION_WRITE_OPERATION).await?;
    validate_release_target(
        &state,
        &project_id,
        &request.release_id,
        &request.runtime_profile_id,
    )
    .await?;
    commit_publication_intent(
        &state,
        PublicationIntent {
            project_id,
            kind: PublishOperationKind::Rollback,
            release_id: Some(request.release_id),
            expected_current_release_id: request.expected_current_release_id,
            expected_generation: request.expected_generation,
            runtime_profile_id: request.runtime_profile_id,
            idempotency_key: idempotency_key(&headers)?,
        },
    )
    .await
}

async fn unpublish_work(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UnpublishWorkRequest>,
) -> Result<(StatusCode, Json<PublicationOperationResponse>), (StatusCode, Json<ErrorResponse>)> {
    authorize_publication(&state, &headers, &project_id, PUBLICATION_WRITE_OPERATION).await?;
    commit_publication_intent(
        &state,
        PublicationIntent {
            project_id,
            kind: PublishOperationKind::Unpublish,
            release_id: None,
            expected_current_release_id: request.expected_current_release_id,
            expected_generation: request.expected_generation,
            runtime_profile_id: request.runtime_profile_id,
            idempotency_key: idempotency_key(&headers)?,
        },
    )
    .await
}

async fn commit_publication_intent(
    state: &AppState,
    intent: PublicationIntent,
) -> Result<(StatusCode, Json<PublicationOperationResponse>), (StatusCode, Json<ErrorResponse>)> {
    let store = state.store.publication_store();
    let (operation, _) = tokio::task::spawn_blocking(move || store.commit_intent(&intent))
        .await
        .map_err(|error| internal_error(anyhow::anyhow!(error)))?
        .map_err(publication_store_error)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(PublicationOperationResponse { operation }),
    ))
}

async fn deployment_state(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<DeploymentStateResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_publication(&state, &headers, &project_id, PUBLICATION_READ_OPERATION).await?;
    let runtime = state
        .store
        .publication_store()
        .runtime(&project_id)
        .ok_or_else(|| not_found(format!("deployment state not found: {project_id}")))?;
    Ok(Json(DeploymentStateResponse { runtime }))
}

async fn work_releases(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<WorkReleaseListResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_publication(&state, &headers, &project_id, PUBLICATION_READ_OPERATION).await?;
    Ok(Json(WorkReleaseListResponse {
        releases: state
            .store
            .release_store()
            .releases_for_project(&project_id),
        project_id,
    }))
}

async fn publication_operation(
    State(state): State<AppState>,
    Path(operation_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<PublicationOperationResponse>, (StatusCode, Json<ErrorResponse>)> {
    let operation = state
        .store
        .publication_store()
        .operation(&operation_id)
        .ok_or_else(|| not_found(format!("publication operation not found: {operation_id}")))?;
    authorize_publication(
        &state,
        &headers,
        &operation.project_id,
        PUBLICATION_READ_OPERATION,
    )
    .await?;
    Ok(Json(PublicationOperationResponse { operation }))
}

async fn validate_release_target(
    state: &AppState,
    project_id: &str,
    release_id: &str,
    runtime_profile_id: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if runtime_profile_id != crate::release::STATIC_WEB_PROFILE_ID {
        return Err(bad_request("runtime profile is not supported".to_string()));
    }
    let release = state
        .store
        .release_store()
        .release(release_id)
        .ok_or_else(|| not_found(format!("work release not found: {release_id}")))?;
    if release.project_id != project_id
        || release.runtime_profile_id != runtime_profile_id
        || release.status != WorkReleaseStatus::Validated
    {
        return Err(conflict_error(anyhow::anyhow!(
            "work release is not a validated target for this project"
        )));
    }
    let version = state
        .store
        .get_project_version(&release.version_id)
        .await
        .ok_or_else(|| conflict_error(anyhow::anyhow!("release project version is missing")))?;
    if version.project_id != project_id
        || version.status != crate::types::ProjectVersionStatus::Promoted
    {
        return Err(conflict_error(anyhow::anyhow!(
            "release project version is not promoted"
        )));
    }
    Ok(())
}

fn idempotency_key(headers: &HeaderMap) -> Result<String, (StatusCode, Json<ErrorResponse>)> {
    headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| bad_request("Idempotency-Key header is required".to_string()))
}

fn publication_store_error(error: PublicationStoreError) -> (StatusCode, Json<ErrorResponse>) {
    match error {
        PublicationStoreError::InvalidInput(message) => bad_request(message),
        PublicationStoreError::NotFound(message) => not_found(message),
        PublicationStoreError::Conflict(message)
        | PublicationStoreError::InvalidTransition(message) => {
            conflict_error(anyhow::anyhow!(message))
        }
        PublicationStoreError::Storage(message) => internal_error(anyhow::anyhow!(message)),
    }
}
