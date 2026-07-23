use crate::types::{AgentEvent, AgentRun, ObservationPurpose};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const RUN_EFFICIENCY_METRICS_SCHEMA: &str = "run-efficiency-metrics@1";
pub const RUN_EFFICIENCY_CALCULATOR_VERSION: &str = "run-efficiency-calculator@1";
pub const RUN_MODEL_USAGE_SCHEMA: &str = "run-model-usage@1";
pub const RUN_PROMPT_EFFICIENCY_SCHEMA: &str = "run-prompt-efficiency@1";
pub const GENERATION_OPERATION_USAGE_SCHEMA: &str = "generation-operation-usage@1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunModelUsage {
    pub schema_version: String,
    pub run_id: String,
    pub model_service_id: Option<String>,
    pub model_display_name: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub total_tokens: u64,
    pub estimated: bool,
    pub turn_count: u32,
}

pub fn calculate_run_model_usage(run: &AgentRun, events: &[AgentEvent]) -> RunModelUsage {
    let mut turns = BTreeMap::<u32, (u64, u64, u64, bool)>::new();
    let mut display_name = None;
    for event in events {
        match event {
            AgentEvent::ModelUsage {
                turn,
                input_tokens,
                output_tokens,
                cached_input_tokens,
                estimated,
                ..
            } => {
                // Events are append-only. Keeping the last value makes replay and
                // recovery idempotent for the stable (runId, turn) identity.
                turns.insert(
                    *turn,
                    (
                        *input_tokens,
                        *output_tokens,
                        (*cached_input_tokens).min(*input_tokens),
                        *estimated,
                    ),
                );
            }
            AgentEvent::ModelExecution { snapshot, .. } => {
                if let Some(name) = snapshot
                    .get("displayName")
                    .and_then(Value::as_str)
                    .filter(|name| !name.trim().is_empty())
                {
                    display_name = Some(name.to_string());
                }
            }
            _ => {}
        }
    }
    let (input_tokens, output_tokens, cached_input_tokens, estimated) = turns.values().fold(
        (0u64, 0u64, 0u64, false),
        |(input_total, output_total, cached_total, any_estimated),
         (input, output, cached, turn_estimated)| {
            (
                input_total.saturating_add(*input),
                output_total.saturating_add(*output),
                cached_total.saturating_add(*cached),
                any_estimated || *turn_estimated,
            )
        },
    );
    let model_service_id = run.model.strip_prefix("resource:").map(str::to_string);
    RunModelUsage {
        schema_version: RUN_MODEL_USAGE_SCHEMA.to_string(),
        run_id: run.id.clone(),
        model_display_name: display_name.or_else(|| model_service_id.clone()),
        model_service_id,
        input_tokens,
        output_tokens,
        cached_input_tokens,
        total_tokens: input_tokens.saturating_add(output_tokens),
        estimated,
        turn_count: u32::try_from(turns.len()).unwrap_or(u32::MAX),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationOperationAttemptUsage {
    pub run_id: String,
    pub attempt: u32,
    pub status: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub total_tokens: u64,
    pub turn_count: u32,
    pub estimated: bool,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationOperationUsage {
    pub schema_version: String,
    pub project_id: String,
    pub operation_id: String,
    pub attempts: Vec<GenerationOperationAttemptUsage>,
    pub input_tokens: u64,
    pub uncached_input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub total_tokens: u64,
    pub turn_count: u32,
    pub automatic_continuation_count: u32,
    pub retry_amplification_basis_points: Option<u64>,
    pub estimated: bool,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub latency_ms: Option<u64>,
    pub status: String,
}

pub fn calculate_generation_operation_usage(
    project_id: &str,
    operation_id: &str,
    run_events: &[(AgentRun, Vec<AgentEvent>)],
) -> Option<GenerationOperationUsage> {
    let mut matching = run_events
        .iter()
        .filter(|(run, _)| {
            run.project_id == project_id && run.operation_id.as_deref() == Some(operation_id)
        })
        .collect::<Vec<_>>();
    matching.sort_by(|(left, _), (right, _)| {
        left.operation_attempt
            .cmp(&right.operation_attempt)
            .then_with(|| left.started_at.cmp(&right.started_at))
            .then_with(|| left.id.cmp(&right.id))
    });
    let first = matching.first()?;
    let started_at = matching
        .iter()
        .map(|(run, _)| run.started_at)
        .min()
        .unwrap_or(first.0.started_at);
    let all_terminal = matching.iter().all(|(run, _)| run.status.is_terminal());
    let completed_at = if all_terminal && matching.iter().all(|(run, _)| run.completed_at.is_some())
    {
        matching
            .iter()
            .filter_map(|(run, _)| run.completed_at)
            .max()
    } else {
        None
    };
    let latency_ms = completed_at.map(|completed_at| {
        u64::try_from((completed_at - started_at).num_milliseconds().max(0)).unwrap_or(u64::MAX)
    });
    let mut automatic_continuation_count = 0u32;
    let attempts = matching
        .into_iter()
        .map(|(run, events)| {
            automatic_continuation_count = automatic_continuation_count.saturating_add(
                events
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            AgentEvent::RunContinuationCreated {
                                automatic: true,
                                operation_id: event_operation_id,
                                ..
                            } if event_operation_id == operation_id
                        )
                    })
                    .count() as u32,
            );
            let usage = calculate_run_model_usage(run, events);
            GenerationOperationAttemptUsage {
                run_id: run.id.clone(),
                attempt: run.operation_attempt.max(1),
                status: serde_json::to_value(run.status)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_string))
                    .unwrap_or_else(|| "unknown".to_string()),
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cached_input_tokens: usage.cached_input_tokens,
                total_tokens: usage.total_tokens,
                turn_count: usage.turn_count,
                estimated: usage.estimated,
                started_at: run.started_at,
                completed_at: run.completed_at,
            }
        })
        .collect::<Vec<_>>();
    let input_tokens: u64 = attempts.iter().map(|attempt| attempt.input_tokens).sum();
    let output_tokens: u64 = attempts.iter().map(|attempt| attempt.output_tokens).sum();
    let cached_input_tokens: u64 = attempts
        .iter()
        .map(|attempt| attempt.cached_input_tokens)
        .sum();
    let total_tokens: u64 = attempts.iter().map(|attempt| attempt.total_tokens).sum();
    let uncached_input_tokens = input_tokens.saturating_sub(cached_input_tokens);
    let turn_count: u32 = attempts.iter().map(|attempt| attempt.turn_count).sum();
    let first_attempt_tokens = attempts.first().map_or(0, |attempt| attempt.total_tokens);
    let retry_amplification_basis_points = (attempts.len() > 1 && first_attempt_tokens > 0)
        .then(|| total_tokens.saturating_mul(10_000) / first_attempt_tokens);
    let estimated = attempts.iter().any(|attempt| attempt.estimated);
    let status = attempts
        .last()
        .map(|attempt| attempt.status.clone())
        .unwrap_or_else(|| "unknown".to_string());

    Some(GenerationOperationUsage {
        schema_version: GENERATION_OPERATION_USAGE_SCHEMA.to_string(),
        project_id: project_id.to_string(),
        operation_id: operation_id.to_string(),
        attempts,
        input_tokens,
        uncached_input_tokens,
        output_tokens,
        cached_input_tokens,
        total_tokens,
        turn_count,
        automatic_continuation_count,
        retry_amplification_basis_points,
        estimated,
        started_at,
        completed_at,
        latency_ms,
        status,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPromptEfficiency {
    pub schema_version: String,
    pub run_id: String,
    pub gross_input_tokens: u64,
    pub cached_input_tokens: u64,
    pub uncached_input_tokens: u64,
    pub output_tokens: u64,
    pub turn_count: u32,
    pub max_turn_input_tokens: u64,
    pub average_turn_input_tokens: u64,
    pub cache_hit_rate_basis_points: u64,
    pub generation_context_estimated_tokens: u64,
    pub generation_context_repeated_estimated_tokens: u64,
    pub prompt_compaction_count: u64,
    pub prompt_tokens_removed_by_compaction: u64,
    pub large_tool_argument_tokens_retained_peak: u64,
    pub retry_amplification_basis_points: Option<u64>,
    pub estimated: bool,
}

pub fn calculate_run_prompt_efficiency(
    run: &AgentRun,
    events: &[AgentEvent],
) -> RunPromptEfficiency {
    let mut usage_by_turn = BTreeMap::<u32, (u64, u64, u64, bool)>::new();
    let mut context_tokens_by_turn = BTreeMap::<u32, u64>::new();
    let mut prompt_compaction_count = 0u64;
    let mut prompt_tokens_removed_by_compaction = 0u64;
    let mut large_tool_argument_tokens_retained_peak = 0u64;
    for event in events {
        match event {
            AgentEvent::ModelUsage {
                turn,
                input_tokens,
                output_tokens,
                cached_input_tokens,
                estimated,
                ..
            } => {
                usage_by_turn.insert(
                    *turn,
                    (
                        *input_tokens,
                        *output_tokens,
                        (*cached_input_tokens).min(*input_tokens),
                        *estimated,
                    ),
                );
            }
            AgentEvent::PromptComposition {
                turn,
                generation_context_tokens,
                ..
            } => {
                context_tokens_by_turn.insert(*turn, *generation_context_tokens);
            }
            AgentEvent::MetricRecorded { name, value, .. }
                if name == "prompt.compaction_tokens_removed" =>
            {
                prompt_compaction_count = prompt_compaction_count.saturating_add(1);
                prompt_tokens_removed_by_compaction =
                    prompt_tokens_removed_by_compaction.saturating_add(*value);
            }
            AgentEvent::MetricRecorded { name, value, .. }
                if name == "prompt.large_tool_argument_tokens_retained_peak" =>
            {
                large_tool_argument_tokens_retained_peak =
                    large_tool_argument_tokens_retained_peak.max(*value);
            }
            _ => {}
        }
    }
    let gross_input_tokens = usage_by_turn
        .values()
        .map(|(input, _, _, _)| *input)
        .sum::<u64>();
    let cached_input_tokens = usage_by_turn
        .values()
        .map(|(_, _, cached, _)| *cached)
        .sum::<u64>();
    let output_tokens = usage_by_turn
        .values()
        .map(|(_, output, _, _)| *output)
        .sum::<u64>();
    let max_turn_input_tokens = usage_by_turn
        .values()
        .map(|(input, _, _, _)| *input)
        .max()
        .unwrap_or_default();
    let turn_count = u32::try_from(usage_by_turn.len()).unwrap_or(u32::MAX);
    let generation_context_estimated_tokens = context_tokens_by_turn
        .values()
        .copied()
        .max()
        .unwrap_or_default();
    RunPromptEfficiency {
        schema_version: RUN_PROMPT_EFFICIENCY_SCHEMA.to_string(),
        run_id: run.id.clone(),
        gross_input_tokens,
        cached_input_tokens,
        uncached_input_tokens: gross_input_tokens.saturating_sub(cached_input_tokens),
        output_tokens,
        turn_count,
        max_turn_input_tokens,
        average_turn_input_tokens: if turn_count == 0 {
            0
        } else {
            gross_input_tokens / u64::from(turn_count)
        },
        cache_hit_rate_basis_points: if gross_input_tokens == 0 {
            0
        } else {
            cached_input_tokens.saturating_mul(10_000) / gross_input_tokens
        },
        generation_context_estimated_tokens,
        generation_context_repeated_estimated_tokens: context_tokens_by_turn.values().sum(),
        prompt_compaction_count,
        prompt_tokens_removed_by_compaction,
        large_tool_argument_tokens_retained_peak,
        retry_amplification_basis_points: None,
        estimated: usage_by_turn
            .values()
            .any(|(_, _, _, estimated)| *estimated),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunEfficiencyMetrics {
    pub schema_version: String,
    pub calculator_version: String,
    pub run_id: String,
    pub project_id: String,
    pub phase: String,
    pub model: String,
    pub template: Option<String>,
    pub status: String,
    pub total_duration_ms: Option<u64>,
    pub time_to_first_model_turn_ms: Option<u64>,
    pub time_to_first_source_mutation_ms: Option<u64>,
    pub model_turn_at_first_source_mutation: Option<u32>,
    pub time_to_first_greenfield_static_build_ms: Option<u64>,
    pub cold_dev_ready_ms: Option<u64>,
    pub time_to_iframe_applied_ms: Option<u64>,
    pub time_to_durable_snapshot_ms: Option<u64>,
    pub time_to_draft_ready_ms: Option<u64>,
    pub prebuild_fs_read_count: u64,
    pub prebuild_fs_list_count: u64,
    pub prebuild_fs_search_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub context_read_deliveries: u64,
    pub source_read_deliveries: u64,
    pub diagnostic_read_deliveries: u64,
    pub verification_read_deliveries: u64,
    pub full_read_deliveries: u64,
    pub duplicate_full_read_deliveries: u64,
    pub duplicate_full_read_rate_basis_points: u64,
    pub duplicate_read_estimated_tokens: u64,
    pub out_of_scope_mutation_count: u64,
    pub first_build_succeeded: bool,
    pub required_fidelity_passed: Option<bool>,
}

pub fn calculate_run_efficiency_metrics(
    run: &AgentRun,
    events: &[AgentEvent],
) -> RunEfficiencyMetrics {
    let first_build_at = events.iter().find_map(|event| match event {
        AgentEvent::ToolCompleted {
            tool, timestamp, ..
        } if tool == "project.build" => Some(*timestamp),
        _ => None,
    });
    let mut prebuild_fs_read_count = 0u64;
    let mut prebuild_fs_list_count = 0u64;
    let mut prebuild_fs_search_count = 0u64;
    for event in events {
        let AgentEvent::ToolStarted {
            tool, timestamp, ..
        } = event
        else {
            continue;
        };
        if first_build_at.is_some_and(|build_at| *timestamp >= build_at) {
            continue;
        }
        match tool.as_str() {
            "fs.read" => prebuild_fs_read_count += 1,
            "fs.list" => prebuild_fs_list_count += 1,
            "fs.search" => prebuild_fs_search_count += 1,
            _ => {}
        }
    }

    let first_model_turn_at = events.iter().find_map(|event| match event {
        AgentEvent::ModelTurnStarted { timestamp, .. } => Some(*timestamp),
        _ => None,
    });
    let usage = calculate_run_model_usage(run, events);
    let mut context_read_deliveries = 0u64;
    let mut source_read_deliveries = 0u64;
    let mut diagnostic_read_deliveries = 0u64;
    let mut verification_read_deliveries = 0u64;
    let mut full_read_deliveries = 0u64;
    let mut duplicate_full_read_deliveries = 0u64;
    let mut duplicate_read_estimated_tokens = 0u64;
    let mut out_of_scope_mutation_count = 0u64;
    for event in events {
        match event {
            AgentEvent::ObservationReceipt { receipt, .. } => {
                match receipt.purpose {
                    ObservationPurpose::Context => context_read_deliveries += 1,
                    ObservationPurpose::Source => source_read_deliveries += 1,
                    ObservationPurpose::Diagnostic => diagnostic_read_deliveries += 1,
                    ObservationPurpose::Verification => verification_read_deliveries += 1,
                    ObservationPurpose::RuntimeInternal => {}
                }
                if receipt.view == crate::types::ObservationView::Full {
                    full_read_deliveries += 1;
                    if receipt.duplicate_delivery {
                        duplicate_full_read_deliveries += 1;
                        duplicate_read_estimated_tokens = duplicate_read_estimated_tokens
                            .saturating_add(receipt.estimated_tokens);
                    }
                }
            }
            AgentEvent::ToolFailed { metadata, .. }
                if metadata
                    .as_ref()
                    .and_then(|value| value.get("errorKind"))
                    .and_then(Value::as_str)
                    == Some("edit.plan_scope_violation") =>
            {
                out_of_scope_mutation_count = out_of_scope_mutation_count.saturating_add(1);
            }
            _ => {}
        }
    }
    let duplicate_full_read_rate_basis_points = if full_read_deliveries == 0 {
        0
    } else {
        duplicate_full_read_deliveries
            .saturating_mul(10_000)
            .div_ceil(full_read_deliveries)
    };
    let metric = |name: &str| -> Option<(u64, Option<&Value>)> {
        events.iter().find_map(|event| match event {
            AgentEvent::MetricRecorded {
                name: candidate,
                value,
                metadata,
                ..
            } if candidate == name => Some((*value, metadata.as_ref())),
            _ => None,
        })
    };
    let source_mutation = metric("efficiency.time_to_first_source_mutation_ms");
    let required_fidelity_passed = events.iter().rev().find_map(|event| match event {
        AgentEvent::MetricRecorded { name, metadata, .. }
            if name == "design_context_fidelity_pass_rate" =>
        {
            metadata
                .as_ref()
                .and_then(|value| value.get("status"))
                .and_then(Value::as_str)
                .map(|status| status == "passed")
        }
        _ => None,
    });
    let completed_at = run.completed_at.or_else(|| {
        events.iter().rev().find_map(|event| match event {
            AgentEvent::RunCompleted { timestamp, .. } => Some(*timestamp),
            _ => None,
        })
    });

    RunEfficiencyMetrics {
        schema_version: RUN_EFFICIENCY_METRICS_SCHEMA.to_string(),
        calculator_version: RUN_EFFICIENCY_CALCULATOR_VERSION.to_string(),
        run_id: run.id.clone(),
        project_id: run.project_id.clone(),
        phase: serde_json::to_value(run.phase)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| format!("{:?}", run.phase).to_lowercase()),
        model: run.model.clone(),
        template: run
            .project_state_snapshot
            .as_ref()
            .map(|state| state.template_key.clone())
            .or_else(|| run.design_profile_template.clone()),
        status: serde_json::to_value(run.status)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| format!("{:?}", run.status).to_lowercase()),
        total_duration_ms: completed_at.map(|completed| {
            completed
                .signed_duration_since(run.started_at)
                .num_milliseconds()
                .max(0) as u64
        }),
        time_to_first_model_turn_ms: first_model_turn_at.map(|started| {
            started
                .signed_duration_since(run.started_at)
                .num_milliseconds()
                .max(0) as u64
        }),
        time_to_first_source_mutation_ms: source_mutation.map(|metric| metric.0),
        model_turn_at_first_source_mutation: source_mutation
            .and_then(|metric| metric.1)
            .and_then(|metadata| metadata.get("turn"))
            .and_then(Value::as_u64)
            .and_then(|turn| u32::try_from(turn).ok()),
        time_to_first_greenfield_static_build_ms: metric(
            "efficiency.time_to_first_greenfield_static_build_ms",
        )
        .map(|metric| metric.0),
        cold_dev_ready_ms: metric("efficiency.cold_dev_ready_ms").map(|metric| metric.0),
        time_to_iframe_applied_ms: metric("efficiency.time_to_iframe_applied_ms")
            .map(|metric| metric.0),
        time_to_durable_snapshot_ms: metric("efficiency.time_to_durable_snapshot_ms")
            .map(|metric| metric.0),
        time_to_draft_ready_ms: metric("efficiency.time_to_draft_ready_ms").map(|metric| metric.0),
        prebuild_fs_read_count,
        prebuild_fs_list_count,
        prebuild_fs_search_count,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        context_read_deliveries,
        source_read_deliveries,
        diagnostic_read_deliveries,
        verification_read_deliveries,
        full_read_deliveries,
        duplicate_full_read_deliveries,
        duplicate_full_read_rate_basis_points,
        duplicate_read_estimated_tokens,
        out_of_scope_mutation_count,
        first_build_succeeded: first_build_at.is_some(),
        required_fidelity_passed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        conversation::RuntimeStore,
        types::{
            AgentPhase, AgentRunStatus, ObservationOutcome, ObservationReceipt, ObservationView,
            OBSERVATION_RECEIPT_SCHEMA,
        },
    };
    use chrono::{Duration, Utc};

    #[tokio::test]
    async fn operation_usage_aggregates_attempts_without_double_counting_turns() {
        let store = RuntimeStore::new();
        let mut first = store
            .create_run(
                "project-1".to_string(),
                AgentPhase::Build,
                "build".to_string(),
                "fixture".to_string(),
                vec![],
            )
            .await;
        first.operation_id = Some("operation-1".to_string());
        first.operation_attempt = 1;
        first.status = AgentRunStatus::Partial;
        first.completed_at = Some(first.started_at + Duration::milliseconds(10));
        let mut second = store
            .create_run(
                "project-1".to_string(),
                AgentPhase::Build,
                "build".to_string(),
                "fixture".to_string(),
                vec![],
            )
            .await;
        second.operation_id = Some("operation-1".to_string());
        second.operation_attempt = 2;
        second.status = AgentRunStatus::Completed;
        second.started_at = first.started_at + Duration::milliseconds(20);
        second.completed_at = Some(first.started_at + Duration::milliseconds(50));
        let usage = |run_id: &str, turn, input, output| AgentEvent::ModelUsage {
            run_id: run_id.to_string(),
            turn,
            input_tokens: input,
            output_tokens: output,
            cached_input_tokens: 0,
            estimated: false,
            timestamp: Utc::now(),
        };
        let report = calculate_generation_operation_usage(
            "project-1",
            "operation-1",
            &[
                (
                    first.clone(),
                    vec![usage(&first.id, 1, 100, 10), usage(&first.id, 1, 120, 12)],
                ),
                (
                    second.clone(),
                    vec![
                        AgentEvent::RunContinuationCreated {
                            run_id: second.id.clone(),
                            operation_id: "operation-1".to_string(),
                            predecessor_run_id: first.id.clone(),
                            continuation_snapshot_id: "continuation-1".to_string(),
                            attempt: 2,
                            automatic: true,
                            timestamp: Utc::now(),
                        },
                        usage(&second.id, 1, 30, 3),
                    ],
                ),
            ],
        )
        .expect("operation usage");

        assert_eq!(report.attempts.len(), 2);
        assert_eq!(report.input_tokens, 150);
        assert_eq!(report.output_tokens, 15);
        assert_eq!(report.turn_count, 2);
        assert_eq!(report.automatic_continuation_count, 1);
        assert_eq!(report.retry_amplification_basis_points, Some(12_500));
        assert_eq!(report.latency_ms, Some(50));
    }

    #[tokio::test]
    async fn calculator_reports_prebuild_and_duplicate_delivery_metrics() {
        let store = RuntimeStore::new();
        let run = store
            .create_run(
                "project-1".to_string(),
                AgentPhase::Build,
                "build".to_string(),
                "fixture".to_string(),
                vec![],
            )
            .await;
        let start = run.started_at;
        let receipt = |duplicate_delivery, read_count| ObservationReceipt {
            schema_version: OBSERVATION_RECEIPT_SCHEMA.to_string(),
            run_id: run.id.clone(),
            normalized_path: "project/app/page.tsx".to_string(),
            content_sha256: "a".repeat(64),
            context_window_epoch: 0,
            view: ObservationView::Full,
            last_outcome: ObservationOutcome::ContentReturned,
            first_read_turn: 1,
            last_read_turn: 1,
            read_count,
            purpose: ObservationPurpose::Source,
            delivered_bytes: 40,
            estimated_tokens: 10,
            duplicate_delivery,
        };
        let events = vec![
            AgentEvent::ModelTurnStarted {
                run_id: run.id.clone(),
                turn: 1,
                timestamp: start + Duration::milliseconds(5),
            },
            AgentEvent::ToolStarted {
                run_id: run.id.clone(),
                tool: "fs.read".to_string(),
                summary: String::new(),
                tool_use_id: "read-1".to_string(),
                timestamp: start + Duration::milliseconds(10),
            },
            AgentEvent::ObservationReceipt {
                run_id: run.id.clone(),
                receipt: receipt(false, 1),
                timestamp: Utc::now(),
            },
            AgentEvent::ObservationReceipt {
                run_id: run.id.clone(),
                receipt: receipt(true, 2),
                timestamp: Utc::now(),
            },
            AgentEvent::ModelUsage {
                run_id: run.id.clone(),
                turn: 1,
                input_tokens: 100,
                output_tokens: 20,
                cached_input_tokens: 5,
                estimated: false,
                timestamp: Utc::now(),
            },
            AgentEvent::ToolCompleted {
                run_id: run.id.clone(),
                tool: "project.build".to_string(),
                summary: String::new(),
                tool_use_id: "build-1".to_string(),
                metadata: None,
                timestamp: start + Duration::milliseconds(20),
            },
            AgentEvent::MetricRecorded {
                run_id: run.id.clone(),
                name: "efficiency.time_to_iframe_applied_ms".to_string(),
                value: 15,
                metadata: None,
                timestamp: Utc::now(),
            },
            AgentEvent::MetricRecorded {
                run_id: run.id.clone(),
                name: "design_context_fidelity_pass_rate".to_string(),
                value: 1,
                metadata: Some(serde_json::json!({ "status": "passed" })),
                timestamp: Utc::now(),
            },
            AgentEvent::ToolFailed {
                run_id: run.id.clone(),
                tool: "fs.write".to_string(),
                error: "out of scope".to_string(),
                tool_use_id: "write-out-of-scope".to_string(),
                recoverable: false,
                metadata: Some(serde_json::json!({
                    "errorKind": "edit.plan_scope_violation"
                })),
                timestamp: Utc::now(),
            },
        ];

        let metrics = calculate_run_efficiency_metrics(&run, &events);
        assert_eq!(metrics.time_to_first_model_turn_ms, Some(5));
        assert_eq!(metrics.prebuild_fs_read_count, 1);
        assert_eq!(metrics.input_tokens, 100);
        assert_eq!(metrics.full_read_deliveries, 2);
        assert_eq!(metrics.duplicate_full_read_deliveries, 1);
        assert_eq!(metrics.duplicate_full_read_rate_basis_points, 5_000);
        assert_eq!(metrics.duplicate_read_estimated_tokens, 10);
        assert!(metrics.first_build_succeeded);
        assert_eq!(metrics.time_to_iframe_applied_ms, Some(15));
        assert_eq!(metrics.required_fidelity_passed, Some(true));
        assert_eq!(metrics.out_of_scope_mutation_count, 1);
    }

    #[tokio::test]
    async fn model_usage_keeps_latest_event_per_turn() {
        let store = RuntimeStore::new();
        let run = store
            .create_run(
                "project-1".to_string(),
                AgentPhase::Build,
                "build".to_string(),
                "resource:model-service-1".to_string(),
                vec![],
            )
            .await;
        let event = |turn, input_tokens, output_tokens, estimated| AgentEvent::ModelUsage {
            run_id: run.id.clone(),
            turn,
            input_tokens,
            output_tokens,
            cached_input_tokens: 5,
            estimated,
            timestamp: Utc::now(),
        };
        let usage = calculate_run_model_usage(
            &run,
            &[
                event(1, 100, 20, true),
                event(1, 110, 25, false),
                event(2, 50, 10, false),
            ],
        );
        assert_eq!(usage.model_service_id.as_deref(), Some("model-service-1"));
        assert_eq!(usage.input_tokens, 160);
        assert_eq!(usage.output_tokens, 35);
        assert_eq!(usage.total_tokens, 195);
        assert_eq!(usage.turn_count, 2);
        assert!(!usage.estimated);
    }

    #[tokio::test]
    async fn prompt_efficiency_separates_gross_cached_and_uncached_usage() {
        let store = RuntimeStore::new();
        let run = store
            .create_run(
                "project-prompt-efficiency".to_string(),
                AgentPhase::Build,
                "build".to_string(),
                "fixture".to_string(),
                vec![],
            )
            .await;
        let usage = |turn, input, cached, output| AgentEvent::ModelUsage {
            run_id: run.id.clone(),
            turn,
            input_tokens: input,
            output_tokens: output,
            cached_input_tokens: cached,
            estimated: false,
            timestamp: Utc::now(),
        };
        let composition = |turn, generation_context_tokens| AgentEvent::PromptComposition {
            run_id: run.id.clone(),
            turn,
            estimated_input_tokens: 100,
            system_tokens: 10,
            message_tokens: 20,
            tool_definition_tokens: 30,
            generation_context_tokens,
            static_prefix_hash: "a".repeat(64),
            tool_set_hash_version: Some(crate::types::TOOL_SET_HASH_VERSION.to_string()),
            tool_set_hash: "b".repeat(64),
            timestamp: Utc::now(),
        };
        let metrics = calculate_run_prompt_efficiency(
            &run,
            &[
                usage(1, 100, 40, 10),
                composition(1, 7),
                usage(2, 200, 250, 20),
                composition(2, 7),
            ],
        );

        assert_eq!(metrics.gross_input_tokens, 300);
        assert_eq!(metrics.cached_input_tokens, 240);
        assert_eq!(metrics.uncached_input_tokens, 60);
        assert_eq!(metrics.output_tokens, 30);
        assert_eq!(metrics.turn_count, 2);
        assert_eq!(metrics.max_turn_input_tokens, 200);
        assert_eq!(metrics.average_turn_input_tokens, 150);
        assert_eq!(metrics.cache_hit_rate_basis_points, 8_000);
        assert_eq!(metrics.generation_context_estimated_tokens, 7);
        assert_eq!(metrics.generation_context_repeated_estimated_tokens, 14);
    }
}
