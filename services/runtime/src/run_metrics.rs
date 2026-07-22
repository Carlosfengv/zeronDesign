use crate::types::{AgentEvent, AgentRun, ObservationPurpose};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const RUN_EFFICIENCY_METRICS_SCHEMA: &str = "run-efficiency-metrics@1";
pub const RUN_EFFICIENCY_CALCULATOR_VERSION: &str = "run-efficiency-calculator@1";

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
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut cached_input_tokens = 0u64;
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
            AgentEvent::ModelUsage {
                input_tokens: input,
                output_tokens: output,
                cached_input_tokens: cached,
                ..
            } => {
                input_tokens = input_tokens.saturating_add(*input);
                output_tokens = output_tokens.saturating_add(*output);
                cached_input_tokens = cached_input_tokens.saturating_add(*cached);
            }
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
        input_tokens,
        output_tokens,
        cached_input_tokens,
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
            AgentPhase, ObservationOutcome, ObservationReceipt, ObservationView,
            OBSERVATION_RECEIPT_SCHEMA,
        },
    };
    use chrono::{Duration, Utc};

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
}
