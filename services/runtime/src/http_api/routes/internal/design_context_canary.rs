use super::super::super::*;
use crate::{
    profile_token_sync::ProfileTokenSyncOperationStatus,
    types::{AgentEvent, AgentRunStatus, DesignContextEnforcementBinding},
};
use chrono::{DateTime, Duration};
use std::collections::{BTreeMap, HashMap};

const MAX_CANARY_WINDOW_DAYS: i64 = 31;
const CANARY_METRIC_NAMES: &[&str] = &[
    "design_context_package_compiled_total",
    "design_context_required_read_block_total",
    "design_context_capability_gap_total",
    "design_context_fidelity_pass_rate",
    "design_context_recipe_rule_fail_total",
    "design_context_source_sections_read",
    "design_context_a11y_required_fail_total",
    "design_context_responsive_required_fail_total",
    "design_context_profile_sync_total",
    "design_context_verifier_unavailable_total",
];

pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/internal/projects/{project_id}/design-context-canary-metrics",
        get(internal_design_context_canary_metrics),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesignContextCanaryMetricsQuery {
    design_profile_id: String,
    design_profile_version: u32,
    observe_policy_revision: u64,
    policy_revision: u64,
    baseline_started_at: String,
    baseline_ended_at: String,
    observation_started_at: String,
    observation_ended_at: String,
    conclusion_recorded_by: String,
}

#[derive(Clone, Copy)]
enum CanaryMode {
    Baseline,
    Enforced,
}

impl CanaryMode {
    fn sample_mode(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Enforced => "enforced",
        }
    }

    fn effective_mode(self) -> &'static str {
        match self {
            Self::Baseline => "observe",
            Self::Enforced => "enforced",
        }
    }
}

struct CanaryWindow {
    baseline_started_at: DateTime<Utc>,
    baseline_ended_at: DateTime<Utc>,
    observation_started_at: DateTime<Utc>,
    observation_ended_at: DateTime<Utc>,
}

async fn internal_design_context_canary_metrics(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<DesignContextCanaryMetricsQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    if !internal_admin_authorized(&state.config, &headers) {
        state
            .store
            .append_audit_record(
                &project_id,
                "",
                "internal.design_context_canary.metrics_export",
                "authorization=missing_or_invalid".to_string(),
                "deny",
                "missing or invalid internal service authorization",
            )
            .await;
        return Err(unauthorized(
            "design context canary metrics export requires service authorization".to_string(),
        ));
    }
    let window = validate_query(&query).map_err(conflict_error)?;
    let profile = state
        .store
        .design_profile_versions(&query.design_profile_id)
        .await
        .map_err(internal_error)?
        .into_iter()
        .rev()
        .find(|profile| profile.version == query.design_profile_version)
        .ok_or_else(|| not_found("design profile revision not found".to_string()))?;
    if profile.project_id() != Some(project_id.as_str())
        || profile.version != query.design_profile_version
    {
        return Err(conflict_error(anyhow::anyhow!(
            "design profile does not match the requested canary project/revision"
        )));
    }

    let runs = state
        .store
        .project_runs(&project_id)
        .await
        .map_err(internal_error)?;
    let mut events_by_run = HashMap::new();
    for run in &runs {
        events_by_run.insert(run.id.clone(), state.store.events(&run.id).await);
    }

    let mut samples = Vec::new();
    let mut metric_totals = BTreeMap::<String, u64>::new();
    let mut metric_event_count = 0_u64;
    let mut relevant_run_windows = HashMap::<String, (DateTime<Utc>, DateTime<Utc>)>::new();
    let mut required_failure_at = HashMap::<String, DateTime<Utc>>::new();
    let mut repair_pass_at = HashMap::<String, DateTime<Utc>>::new();
    for (mode, started_at, ended_at, policy_revision) in [
        (
            CanaryMode::Baseline,
            window.baseline_started_at,
            window.baseline_ended_at,
            query.observe_policy_revision,
        ),
        (
            CanaryMode::Enforced,
            window.observation_started_at,
            window.observation_ended_at,
            query.policy_revision,
        ),
    ] {
        for run in runs.iter().filter(|run| {
            run_matches_cohort(
                run,
                &project_id,
                &query.design_profile_id,
                query.design_profile_version,
                policy_revision,
                mode,
            )
        }) {
            let events = events_by_run.get(&run.id).map(Vec::as_slice).unwrap_or(&[]);
            let events_in_window = events
                .iter()
                .filter(|event| {
                    event_timestamp(event).is_some_and(|at| at >= started_at && at <= ended_at)
                })
                .collect::<Vec<_>>();
            if events_in_window.is_empty() {
                continue;
            }
            relevant_run_windows.insert(run.id.clone(), (started_at, ended_at));
            for event in &events_in_window {
                if let AgentEvent::MetricRecorded {
                    name,
                    value,
                    metadata,
                    timestamp,
                    ..
                } = event
                {
                    if !CANARY_METRIC_NAMES.contains(&name.as_str()) {
                        continue;
                    }
                    if metadata
                        .as_ref()
                        .and_then(|value| value.get("mode"))
                        .and_then(Value::as_str)
                        != Some(mode.effective_mode())
                        || metadata
                            .as_ref()
                            .and_then(|value| value.get("surface"))
                            .and_then(Value::as_str)
                            != Some("website")
                    {
                        return Err(conflict_error(anyhow::anyhow!(
                            "canary metric metadata does not match its frozen Run cohort"
                        )));
                    }
                    metric_event_count += 1;
                    *metric_totals.entry(name.clone()).or_default() += value;
                    if mode.sample_mode() == "enforced"
                        && matches!(
                            name.as_str(),
                            "design_context_a11y_required_fail_total"
                                | "design_context_responsive_required_fail_total"
                        )
                    {
                        required_failure_at
                            .entry(run.id.clone())
                            .and_modify(|existing| *existing = (*existing).min(*timestamp))
                            .or_insert(*timestamp);
                    }
                    if name == "design_context_fidelity_pass_rate"
                        && metadata
                            .as_ref()
                            .and_then(|value| value.get("status"))
                            .and_then(Value::as_str)
                            == Some("passed")
                        && metadata
                            .as_ref()
                            .and_then(|value| value.get("attempt"))
                            .and_then(Value::as_str)
                            == Some("repair")
                    {
                        repair_pass_at
                            .entry(run.id.clone())
                            .and_modify(|existing| *existing = (*existing).max(*timestamp))
                            .or_insert(*timestamp);
                    }
                }
            }
            if let Some(sample) = publish_sample(run, &events_in_window, mode, &project_id, &query)
            {
                samples.push(sample);
            }
        }
    }

    samples.sort_by(|left, right| {
        left["observedAt"]
            .as_str()
            .cmp(&right["observedAt"].as_str())
            .then_with(|| left["sampleId"].as_str().cmp(&right["sampleId"].as_str()))
    });
    let baseline_samples = samples
        .iter()
        .filter(|sample| sample["mode"] == "baseline")
        .collect::<Vec<_>>();
    let enforced_samples = samples
        .iter()
        .filter(|sample| sample["mode"] == "enforced")
        .collect::<Vec<_>>();
    let baseline_failure_rate = failure_rate(&baseline_samples);
    let enforced_failure_rate = failure_rate(&enforced_samples);
    let publish_failure_rate_delta_pp = (enforced_failure_rate - baseline_failure_rate) * 100.0;

    let verifier_unavailable_count = metric_total(
        &events_by_run,
        &relevant_run_windows,
        "design_context_verifier_unavailable_total",
        Some(("reason", "runtime_lost", false)),
    );
    let verifier_runtime_lost_count = metric_total(
        &events_by_run,
        &relevant_run_windows,
        "design_context_verifier_unavailable_total",
        Some(("reason", "runtime_lost", true)),
    );
    let unexpected_read_gate_block_count = metric_total(
        &events_by_run,
        &relevant_run_windows,
        "design_context_required_read_block_total",
        None,
    );
    let operations = state
        .store
        .project_profile_token_sync_operations(&project_id)
        .await
        .map_err(internal_error)?;
    let recovery_required_over_24h_count = operations
        .iter()
        .filter(|operation| {
            operation.target_design_profile_id == query.design_profile_id
                && operation.target_design_profile_version == query.design_profile_version
                && operation.status == ProfileTokenSyncOperationStatus::RecoveryRequired
                && operation.updated_at <= window.observation_ended_at - Duration::hours(24)
                && operation.created_at <= window.observation_ended_at
        })
        .count() as u64;
    let required_finding_count = required_failure_at.len() as u64;
    let repaired_required_finding_count = required_failure_at
        .iter()
        .filter(|(run_id, failed_at)| {
            repair_pass_at
                .get(*run_id)
                .is_some_and(|passed_at| passed_at > *failed_at)
        })
        .count() as u64;
    let required_finding_repair_rate = if required_finding_count == 0 {
        1.0
    } else {
        repaired_required_finding_count as f64 / required_finding_count as f64
    };

    let alerts = vec![
        alert(
            "publish_failure_rate_delta",
            publish_failure_rate_delta_pp > 2.0,
            publish_failure_rate_delta_pp,
            "max_2_percentage_points",
            "stop_expansion_or_disable_exact_policy",
        ),
        alert(
            "verifier_unavailable",
            verifier_unavailable_count > 0,
            verifier_unavailable_count as f64,
            "must_equal_0",
            "page_and_disable_affected_exact_policy",
        ),
        alert(
            "verifier_runtime_lost",
            verifier_runtime_lost_count > 0,
            verifier_runtime_lost_count as f64,
            "must_equal_0",
            "page_and_disable_affected_exact_policy",
        ),
        alert(
            "unexpected_read_gate_block",
            unexpected_read_gate_block_count > 0,
            unexpected_read_gate_block_count as f64,
            "must_equal_0",
            "pause_canary_tier",
        ),
        alert(
            "profile_sync_recovery_over_24h",
            recovery_required_over_24h_count > 0,
            recovery_required_over_24h_count as f64,
            "must_equal_0",
            "freeze_allowlist_expansion",
        ),
        alert(
            "required_finding_repair_rate",
            required_finding_repair_rate < 1.0,
            required_finding_repair_rate,
            "must_equal_1",
            "return_to_fixture_remediation",
        ),
    ];
    let alerts_triggered = alerts.iter().any(|value| value["triggered"] == true);
    let response = json!({
        "schemaVersion": "design-context-canary-operational-export@1",
        "generatedAt": Utc::now(),
        "source": {
            "kind": "runtime-durable-store",
            "projectRunCount": runs.len(),
            "cohortRunCount": relevant_run_windows.len(),
            "metricEventCount": metric_event_count,
        },
        "cohort": {
            "projectId": project_id,
            "designProfileId": query.design_profile_id,
            "designProfileVersion": query.design_profile_version,
            "observePolicyRevision": query.observe_policy_revision,
            "policyRevision": query.policy_revision,
        },
        "window": {
            "baselineStartedAt": window.baseline_started_at,
            "baselineEndedAt": window.baseline_ended_at,
            "observationStartedAt": window.observation_started_at,
            "observationEndedAt": window.observation_ended_at,
        },
        "publish": {
            "samples": samples,
            "baselinePublishCount": baseline_samples.len(),
            "enforcedPublishCount": enforced_samples.len(),
            "baselineFailureRate": baseline_failure_rate,
            "enforcedFailureRate": enforced_failure_rate,
            "publishFailureRateDeltaPp": publish_failure_rate_delta_pp,
        },
        "metrics": {
            "totals": metric_totals,
            "verifierUnavailableCount": verifier_unavailable_count,
            "verifierRuntimeLostCount": verifier_runtime_lost_count,
            "unexpectedReadGateBlockCount": unexpected_read_gate_block_count,
            "recoveryRequiredOver24hCount": recovery_required_over_24h_count,
            "requiredFindingCount": required_finding_count,
            "repairedRequiredFindingCount": repaired_required_finding_count,
            "requiredFindingRepairRate": required_finding_repair_rate,
        },
        "alerts": alerts,
        "alertsTriggered": alerts_triggered,
        "conclusionRecordedBy": query.conclusion_recorded_by,
    });
    state
        .store
        .append_audit_record(
            response["cohort"]["projectId"].as_str().unwrap_or_default(),
            "",
            "internal.design_context_canary.metrics_export",
            format!(
                "baselineSamples={} enforcedSamples={} alertsTriggered={alerts_triggered}",
                baseline_samples.len(),
                enforced_samples.len(),
            ),
            "allow",
            "aggregated from durable Runtime runs, events, and Profile Sync operations",
        )
        .await;
    Ok(Json(response))
}

fn validate_query(query: &DesignContextCanaryMetricsQuery) -> anyhow::Result<CanaryWindow> {
    if query.design_profile_id.trim().is_empty()
        || query.design_profile_version == 0
        || query.observe_policy_revision == 0
        || query.policy_revision <= query.observe_policy_revision
        || query.conclusion_recorded_by.trim().is_empty()
        || query.conclusion_recorded_by.len() > 128
    {
        return Err(anyhow::anyhow!(
            "invalid exact canary cohort or operator identity"
        ));
    }
    let parse = |value: &str, name: &str| {
        DateTime::parse_from_rfc3339(value)
            .map(|value| value.with_timezone(&Utc))
            .map_err(|_| anyhow::anyhow!("{name} must be an RFC3339 timestamp"))
    };
    let window = CanaryWindow {
        baseline_started_at: parse(&query.baseline_started_at, "baselineStartedAt")?,
        baseline_ended_at: parse(&query.baseline_ended_at, "baselineEndedAt")?,
        observation_started_at: parse(&query.observation_started_at, "observationStartedAt")?,
        observation_ended_at: parse(&query.observation_ended_at, "observationEndedAt")?,
    };
    if window.baseline_ended_at <= window.baseline_started_at
        || window.observation_started_at < window.baseline_ended_at
        || window.observation_ended_at <= window.observation_started_at
        || window.baseline_ended_at - window.baseline_started_at
            > Duration::days(MAX_CANARY_WINDOW_DAYS)
        || window.observation_ended_at - window.observation_started_at
            > Duration::days(MAX_CANARY_WINDOW_DAYS)
        || window.observation_ended_at > Utc::now() + Duration::minutes(5)
    {
        return Err(anyhow::anyhow!(
            "invalid or unsafe canary observation window"
        ));
    }
    Ok(window)
}

fn run_matches_cohort(
    run: &AgentRun,
    project_id: &str,
    profile_id: &str,
    profile_version: u32,
    policy_revision: u64,
    mode: CanaryMode,
) -> bool {
    run.project_id == project_id
        && run.design_profile_id.as_deref() == Some(profile_id)
        && run.design_profile_version == Some(profile_version)
        && run.design_profile_surface.as_deref() == Some("website")
        && run.design_context_effective_compatibility_mode.as_deref() == Some(mode.effective_mode())
        && matches!(
            run.design_context_enforcement_binding.as_ref(),
            Some(DesignContextEnforcementBinding {
                source,
                enabled,
                policy_revision: Some(revision),
                policy_updated_by: Some(updated_by),
            }) if source == "persistent"
                && *enabled == matches!(mode, CanaryMode::Enforced)
                && *revision == policy_revision
                && !updated_by.trim().is_empty()
        )
}

fn publish_sample(
    run: &AgentRun,
    events: &[&AgentEvent],
    mode: CanaryMode,
    project_id: &str,
    query: &DesignContextCanaryMetricsQuery,
) -> Option<Value> {
    let successful_publish_at = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::PreviewUpdated {
                version_id,
                timestamp,
                ..
            } if run.status == AgentRunStatus::Completed
                && run.output_version_id.as_deref() == Some(version_id.as_str()) =>
            {
                Some(*timestamp)
            }
            _ => None,
        })
        .max();
    let failed_publish_at = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolFailed {
                tool, timestamp, ..
            } if tool == "preview.publish" => Some(*timestamp),
            _ => None,
        })
        .max();
    let (observed_at, verdict) = if let Some(at) = successful_publish_at {
        (at, "pass")
    } else {
        (failed_publish_at?, "fail")
    };
    let dcp_caused_failure = verdict == "fail"
        && events.iter().any(|event| match event {
            AgentEvent::MetricRecorded {
                name,
                value,
                metadata,
                ..
            } => {
                *value > 0
                    && (matches!(
                        name.as_str(),
                        "design_context_required_read_block_total"
                            | "design_context_verifier_unavailable_total"
                            | "design_context_a11y_required_fail_total"
                            | "design_context_responsive_required_fail_total"
                    ) || (name == "design_context_fidelity_pass_rate"
                        && metadata
                            .as_ref()
                            .and_then(|value| value.get("status"))
                            .and_then(Value::as_str)
                            == Some("failed")))
            }
            _ => false,
        });
    let policy_revision = match mode {
        CanaryMode::Baseline => query.observe_policy_revision,
        CanaryMode::Enforced => query.policy_revision,
    };
    let observed_at = observed_at.to_rfc3339();
    Some(json!({
        "sampleId": sha256_hex(format!("{project_id}:{}:{}:{observed_at}", run.id, mode.sample_mode()).as_bytes()),
        "runId": run.id,
        "observedAt": observed_at,
        "mode": mode.sample_mode(),
        "publishVerdict": verdict,
        "dcpCausedFailure": dcp_caused_failure,
        "projectId": project_id,
        "designProfileId": query.design_profile_id,
        "designProfileVersion": query.design_profile_version,
        "policyRevision": policy_revision,
    }))
}

fn metric_total(
    events_by_run: &HashMap<String, Vec<AgentEvent>>,
    relevant_run_windows: &HashMap<String, (DateTime<Utc>, DateTime<Utc>)>,
    metric_name: &str,
    metadata_filter: Option<(&str, &str, bool)>,
) -> u64 {
    events_by_run
        .iter()
        .filter_map(|(run_id, events)| {
            relevant_run_windows
                .get(run_id)
                .map(|window| (events, window))
        })
        .flat_map(|(events, window)| events.iter().map(move |event| (event, window)))
        .filter_map(|(event, (started_at, ended_at))| match event {
            AgentEvent::MetricRecorded {
                name,
                value,
                metadata,
                timestamp,
                ..
            } if name == metric_name && *timestamp >= *started_at && *timestamp <= *ended_at => {
                let include = metadata_filter.map_or(true, |(key, expected, equal)| {
                    let matches = metadata
                        .as_ref()
                        .and_then(|value| value.get(key))
                        .and_then(Value::as_str)
                        == Some(expected);
                    matches == equal
                });
                include.then_some(*value)
            }
            _ => None,
        })
        .sum()
}

fn event_timestamp(event: &AgentEvent) -> Option<DateTime<Utc>> {
    serde_json::to_value(event)
        .ok()?
        .get("timestamp")?
        .as_str()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn failure_rate(samples: &[&Value]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples
        .iter()
        .filter(|sample| sample["publishVerdict"] == "fail")
        .count() as f64
        / samples.len() as f64
}

fn alert(code: &str, triggered: bool, actual: f64, threshold: &str, action: &str) -> Value {
    json!({
        "code": code,
        "severity": if triggered { "page" } else { "ok" },
        "triggered": triggered,
        "actual": actual,
        "threshold": threshold,
        "action": action,
    })
}
