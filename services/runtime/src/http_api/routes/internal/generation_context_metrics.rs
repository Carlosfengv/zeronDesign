use super::super::super::*;
use crate::{run_metrics::calculate_run_efficiency_metrics, types::AgentEvent};
use axum::response::IntoResponse;
use std::collections::BTreeMap;

const HISTOGRAM_BUCKETS_SECONDS: &[f64] = &[
    0.5, 1.0, 2.0, 3.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0,
];

pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/internal/metrics/generation-context",
        get(export_generation_context_metrics),
    )
}

async fn export_generation_context_metrics(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    if !metrics_admin_authorized(&state.config, &headers) {
        return Err(unauthorized(
            "Generation Context metrics require service authorization".to_string(),
        ));
    }
    let runs = state.store.all_runs().await.map_err(internal_error)?;
    let mut compile = BTreeMap::<(String, String, String), u64>::new();
    let mut first_mutation = BTreeMap::<(String, String), Vec<f64>>::new();
    let mut first_build = BTreeMap::<(String, String), Vec<f64>>::new();
    let mut cold_ready = BTreeMap::<(String, String), Vec<f64>>::new();
    let mut iframe_applied = BTreeMap::<(String, String), Vec<f64>>::new();
    let mut durable_snapshot = BTreeMap::<(String, String), Vec<f64>>::new();
    let mut unique_reads = BTreeMap::<(String, String), u64>::new();
    let mut duplicate_reads = BTreeMap::<(String, String), u64>::new();
    let mut exploration = BTreeMap::<(String, String, String), u64>::new();
    let mut replan = BTreeMap::<String, u64>::new();
    let mut successors = BTreeMap::<String, u64>::new();
    let mut out_of_scope = 0u64;
    let mut completion_boundary = 0u64;

    for run in runs {
        let events = state.store.events(&run.id).await;
        let metrics = calculate_run_efficiency_metrics(&run, &events);
        let phase = metrics.phase.clone();
        let template = metrics.template.unwrap_or_else(|| "unknown".to_string());
        if let Some(status) = run.generation_context_status.as_deref() {
            *compile
                .entry((status.to_string(), phase.clone(), template.clone()))
                .or_default() += 1;
        }
        push_millis(
            &mut first_mutation,
            (phase.clone(), template.clone()),
            metrics.time_to_first_source_mutation_ms,
        );
        push_millis(
            &mut first_build,
            (phase.clone(), template.clone()),
            metrics.time_to_first_greenfield_static_build_ms,
        );
        push_millis(
            &mut cold_ready,
            (phase.clone(), template.clone()),
            metrics.cold_dev_ready_ms,
        );
        push_millis(
            &mut iframe_applied,
            (phase.clone(), template.clone()),
            metrics.time_to_iframe_applied_ms,
        );
        push_millis(
            &mut durable_snapshot,
            (phase.clone(), template.clone()),
            metrics.time_to_durable_snapshot_ms,
        );
        *unique_reads
            .entry((phase.clone(), template.clone()))
            .or_default() += metrics
            .full_read_deliveries
            .saturating_sub(metrics.duplicate_full_read_deliveries);
        *duplicate_reads
            .entry((phase.clone(), template.clone()))
            .or_default() += metrics.duplicate_full_read_deliveries;
        for (tool, value) in [
            ("read", metrics.prebuild_fs_read_count),
            ("list", metrics.prebuild_fs_list_count),
            ("search", metrics.prebuild_fs_search_count),
        ] {
            *exploration
                .entry((tool.to_string(), phase.clone(), template.clone()))
                .or_default() += value;
        }
        for event in events {
            match event {
                AgentEvent::MetricRecorded {
                    name,
                    value,
                    metadata,
                    ..
                } if name == "edit_plan.replacement_required" => {
                    let reason = metadata
                        .as_ref()
                        .and_then(|value| value.get("errorKind"))
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    *replan.entry(reason.to_string()).or_default() += value;
                }
                AgentEvent::MetricRecorded {
                    name,
                    value,
                    metadata,
                    ..
                } if name == "run.successor_created" => {
                    let reason = metadata
                        .as_ref()
                        .and_then(|value| value.get("reason"))
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    *successors.entry(reason.to_string()).or_default() += value;
                }
                AgentEvent::ToolFailed { tool, metadata, .. } => {
                    let error_kind = metadata
                        .as_ref()
                        .and_then(|value| value.get("errorKind"))
                        .and_then(Value::as_str);
                    if error_kind == Some("edit.plan_scope_violation") {
                        out_of_scope = out_of_scope.saturating_add(1);
                    }
                    if tool == "run.complete" {
                        completion_boundary = completion_boundary.saturating_add(1);
                    }
                }
                _ => {}
            }
        }
    }

    let mut output = String::new();
    output.push_str("# TYPE generation_context_compile_total counter\n");
    for ((status, phase, template), value) in compile {
        line(
            &mut output,
            "generation_context_compile_total",
            &[
                ("status", &status),
                ("phase", &phase),
                ("template", &template),
            ],
            value,
        );
    }
    histogram(
        &mut output,
        "agent_time_to_first_mutation_seconds",
        first_mutation,
    );
    histogram(
        &mut output,
        "agent_time_to_first_greenfield_build_seconds",
        first_build,
    );
    histogram(&mut output, "cold_dev_ready_seconds", cold_ready);
    histogram(
        &mut output,
        "draft_hmr_iframe_applied_seconds",
        iframe_applied,
    );
    histogram(
        &mut output,
        "draft_snapshot_durable_seconds",
        durable_snapshot,
    );
    counters(&mut output, "agent_unique_read_total", unique_reads);
    counters(&mut output, "agent_duplicate_read_total", duplicate_reads);
    output.push_str("# TYPE agent_prebuild_exploration_total counter\n");
    for ((tool, phase, template), value) in exploration {
        line(
            &mut output,
            "agent_prebuild_exploration_total",
            &[("tool", &tool), ("phase", &phase), ("template", &template)],
            value,
        );
    }
    reason_counters(&mut output, "edit_impact_plan_replaced_total", replan);
    reason_counters(&mut output, "agent_successor_run_created_total", successors);
    output.push_str("# TYPE agent_out_of_scope_mutation_total counter\n");
    output.push_str(&format!(
        "agent_out_of_scope_mutation_total {out_of_scope}\n"
    ));
    output.push_str("# TYPE run_completion_boundary_violation_total counter\n");
    output.push_str(&format!(
        "run_completion_boundary_violation_total {completion_boundary}\n"
    ));
    Ok((
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        output,
    ))
}

fn metrics_admin_authorized(config: &crate::config::RuntimeConfig, headers: &HeaderMap) -> bool {
    if internal_admin_authorized(config, headers) {
        return true;
    }
    let Some(expected_token) = config.internal_admin_token.as_deref() else {
        return false;
    };
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        == Some(expected_token)
}

fn push_millis(
    series: &mut BTreeMap<(String, String), Vec<f64>>,
    labels: (String, String),
    value: Option<u64>,
) {
    if let Some(value) = value {
        series
            .entry(labels)
            .or_default()
            .push(value as f64 / 1000.0);
    }
}

fn counters(output: &mut String, name: &str, values: BTreeMap<(String, String), u64>) {
    output.push_str(&format!("# TYPE {name} counter\n"));
    for ((phase, template), value) in values {
        line(
            output,
            name,
            &[("phase", &phase), ("template", &template)],
            value,
        );
    }
}

fn reason_counters(output: &mut String, name: &str, values: BTreeMap<String, u64>) {
    output.push_str(&format!("# TYPE {name} counter\n"));
    for (reason, value) in values {
        line(output, name, &[("reason", &reason)], value);
    }
}

fn histogram(output: &mut String, name: &str, values: BTreeMap<(String, String), Vec<f64>>) {
    output.push_str(&format!("# TYPE {name} histogram\n"));
    for ((phase, template), values) in values {
        for bucket in HISTOGRAM_BUCKETS_SECONDS {
            let count = values.iter().filter(|value| **value <= *bucket).count();
            let le = bucket.to_string();
            line(
                output,
                &format!("{name}_bucket"),
                &[("phase", &phase), ("template", &template), ("le", &le)],
                count,
            );
        }
        line(
            output,
            &format!("{name}_bucket"),
            &[("phase", &phase), ("template", &template), ("le", "+Inf")],
            values.len(),
        );
        line(
            output,
            &format!("{name}_sum"),
            &[("phase", &phase), ("template", &template)],
            values.iter().sum::<f64>(),
        );
        line(
            output,
            &format!("{name}_count"),
            &[("phase", &phase), ("template", &template)],
            values.len(),
        );
    }
}

fn line<T: std::fmt::Display>(output: &mut String, name: &str, labels: &[(&str, &str)], value: T) {
    output.push_str(name);
    if !labels.is_empty() {
        output.push('{');
        for (index, (key, value)) in labels.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push_str(key);
            output.push_str("=\"");
            output.push_str(&escape_label(value));
            output.push('"');
        }
        output.push('}');
    }
    output.push(' ');
    output.push_str(&value.to_string());
    output.push('\n');
}

fn escape_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
}
