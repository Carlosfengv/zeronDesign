use super::super::*;
use crate::content_plan_approval::{
    ContentPlanApproval, ContentPlanApprovalError, ContentPlanApprovalProducerStatus,
    ContentPlanApprovalVerification, ContentPlanChangeResult, RecordContentPlanApproval,
    RecordContentPlanChange,
};

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project_id}/content-plan-approvals",
            post(record_content_plan_approval),
        )
        .route(
            "/projects/{project_id}/content-plan-approvals/verify",
            get(verify_content_plan_approval),
        )
        .route(
            "/projects/{project_id}/content-plan-changes",
            post(record_content_plan_change),
        )
        .route(
            "/projects/{project_id}/content-plan-approval-producer",
            get(content_plan_approval_producer),
        )
}

async fn record_content_plan_approval(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<RecordContentPlanApprovalRequest>,
) -> Result<Json<ContentPlanApproval>, (StatusCode, Json<ErrorResponse>)> {
    authorize_content_plan_producer(
        &state,
        &headers,
        &project_id,
        "content_plan.approval.record",
    )
    .await?;
    state
        .store
        .content_plan_approval_store()
        .approve(RecordContentPlanApproval {
            project_id,
            plan_id: request.plan_id,
            revision: request.revision,
            content_hash: request.content_hash,
            confirmation_event_id: request.confirmation_event_id,
        })
        .map(Json)
        .map_err(content_plan_approval_error)
}

async fn verify_content_plan_approval(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<VerifyContentPlanApprovalQuery>,
) -> Result<Json<ContentPlanApprovalVerification>, (StatusCode, Json<ErrorResponse>)> {
    authorize_content_plan_reader(&state, &policy, &headers, &project_id).await?;
    state
        .store
        .content_plan_approval_store()
        .verify_exact(
            &project_id,
            &query.plan_id,
            query.revision,
            &query.content_hash,
        )
        .map(Json)
        .map_err(content_plan_approval_error)
}

async fn record_content_plan_change(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<RecordContentPlanChangeRequest>,
) -> Result<Json<ContentPlanChangeResult>, (StatusCode, Json<ErrorResponse>)> {
    authorize_content_plan_producer(&state, &headers, &project_id, "content_plan.change.record")
        .await?;
    state
        .store
        .content_plan_approval_store()
        .record_plan_change(RecordContentPlanChange {
            project_id,
            plan_id: request.plan_id,
            revision: request.revision,
            content_hash: request.content_hash,
            change_event_id: request.change_event_id,
        })
        .map(Json)
        .map_err(content_plan_approval_error)
}

async fn content_plan_approval_producer(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ContentPlanApprovalProducerStatus>, (StatusCode, Json<ErrorResponse>)> {
    authorize_content_plan_reader(&state, &policy, &headers, &project_id).await?;
    Ok(Json(
        state.store.content_plan_approval_store().producer_status(),
    ))
}

async fn authorize_content_plan_producer(
    state: &AppState,
    headers: &HeaderMap,
    project_id: &str,
    operation: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if state.config.public_principal_auth_mode == PublicPrincipalAuthMode::Disabled {
        return Ok(());
    }
    if !internal_admin_authorized(&state.config, headers) {
        state
            .store
            .append_audit_record(
                project_id,
                "",
                operation,
                "Content Plan producer transaction".to_string(),
                "deny",
                "missing or invalid internal producer authorization",
            )
            .await;
        return Err(unauthorized(
            "Content Plan mutations require internal producer authorization".to_string(),
        ));
    }
    state
        .store
        .append_audit_record(
            project_id,
            "",
            operation,
            "Content Plan producer transaction".to_string(),
            "allow",
            "internal producer authorized",
        )
        .await;
    Ok(())
}

async fn authorize_content_plan_reader(
    state: &AppState,
    policy: &ApplicationAuthorizationPolicy,
    headers: &HeaderMap,
    project_id: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if internal_admin_authorized(&state.config, headers) {
        return Ok(());
    }
    authorize_project_operation(state, policy, headers, project_id, PROJECT_READ_OPERATION)
        .await
        .map(|_| ())
}

fn content_plan_approval_error(
    error: ContentPlanApprovalError,
) -> (StatusCode, Json<ErrorResponse>) {
    match error {
        ContentPlanApprovalError::InvalidInput(message) => bad_request(message),
        ContentPlanApprovalError::NotFound(message) => not_found(message),
        ContentPlanApprovalError::Conflict { kind, message } => (
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: message,
                error_code: Some(kind.to_string()),
            }),
        ),
        ContentPlanApprovalError::Storage(message) => internal_error(anyhow::anyhow!(message)),
    }
}
