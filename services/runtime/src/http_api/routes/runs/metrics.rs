use super::super::super::*;
use crate::run_metrics::{
    calculate_run_efficiency_metrics, calculate_run_model_usage, calculate_run_prompt_efficiency,
    RunEfficiencyMetrics, RunModelUsage, RunPromptEfficiency,
};
use crate::types::RunBudgetProfile;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/runs/{run_id}/efficiency-metrics",
            get(get_run_efficiency_metrics),
        )
        .route("/runs/{run_id}/model-usage", get(get_run_model_usage))
        .route("/runs/{run_id}/budget-profile", get(get_run_budget_profile))
        .route(
            "/runs/{run_id}/prompt-efficiency",
            get(get_run_prompt_efficiency),
        )
}

async fn get_run_budget_profile(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<RunBudgetProfile>, (StatusCode, Json<ErrorResponse>)> {
    let run = state
        .store
        .get_run(&run_id)
        .await
        .ok_or_else(|| not_found(format!("run not found: {run_id}")))?;
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &run.project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    let profile = run.budget_profile.ok_or_else(|| {
        conflict_error(anyhow::anyhow!(
            "RunBudgetProfile is unavailable for restored legacy Run: {run_id}"
        ))
    })?;
    profile
        .validate(run.phase)
        .map_err(|message| internal_error(anyhow::anyhow!(message)))?;
    Ok(Json(*profile))
}

async fn get_run_prompt_efficiency(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<RunPromptEfficiency>, (StatusCode, Json<ErrorResponse>)> {
    let run = state
        .store
        .get_run(&run_id)
        .await
        .ok_or_else(|| not_found(format!("run not found: {run_id}")))?;
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &run.project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    let events = state.store.events(&run_id).await;
    Ok(Json(calculate_run_prompt_efficiency(&run, &events)))
}

async fn get_run_model_usage(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<RunModelUsage>, (StatusCode, Json<ErrorResponse>)> {
    let run = state
        .store
        .get_run(&run_id)
        .await
        .ok_or_else(|| not_found(format!("run not found: {run_id}")))?;
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &run.project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    let events = state.store.events(&run_id).await;
    Ok(Json(calculate_run_model_usage(&run, &events)))
}

async fn get_run_efficiency_metrics(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<RunEfficiencyMetrics>, (StatusCode, Json<ErrorResponse>)> {
    let run = state
        .store
        .get_run(&run_id)
        .await
        .ok_or_else(|| not_found(format!("run not found: {run_id}")))?;
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &run.project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    let events = state.store.events(&run_id).await;
    Ok(Json(calculate_run_efficiency_metrics(&run, &events)))
}
