use crate::types::{canonical_json_hash, design_signature_rule_capsule_line, DesignProfile};
use anyhow::{anyhow, Result};
use serde_json::Value;

pub fn render_design_profile_markdown(profile: &DesignProfile) -> Result<String> {
    let identity = vec![
        format!("- ID: {}", profile.id),
        format!("- Name: {}", profile.name),
        format!("- Schema: {}", profile.schema_version),
        format!("- Revision: {}", profile.version),
        format!("- Status: {}", profile.status),
    ];
    let identity = render_budgeted_entries("Identity", identity, 500, true)?;
    let source = vec![
        format!(
            "- Kind: {}",
            profile
                .source
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ),
        format!(
            "- Integrity: {}",
            profile
                .source
                .get("integrity")
                .and_then(Value::as_str)
                .unwrap_or("unverified")
        ),
        format!(
            "- Source artifact: {}",
            profile
                .source
                .get("primarySourceArtifactId")
                .and_then(Value::as_str)
                .unwrap_or("none")
        ),
        format!(
            "- Source hash: {}",
            profile
                .source
                .get("sourceHash")
                .and_then(Value::as_str)
                .unwrap_or("none")
        ),
    ];
    let source = render_budgeted_entries("Source Integrity", source, 500, true)?;

    let required_rules = profile
        .signature_rules
        .iter()
        .filter(|rule| rule.get("priority").and_then(Value::as_str) == Some("required"))
        .map(design_signature_rule_capsule_line)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| anyhow!(error))?;
    let required_rules = render_budgeted_entries(
        "Required Signature Rules",
        if required_rules.is_empty() {
            vec!["- None declared".to_string()]
        } else {
            required_rules
        },
        2_500,
        true,
    )?;

    let mut visual_entries = Vec::new();
    collect_scalar_entries("visual", &profile.visual, &mut visual_entries);
    let visual = render_budgeted_entries("Visual Direction", visual_entries, 1_200, false)?;

    let mut token_entries = Vec::new();
    collect_scalar_entries(
        "runtimeTokenMapping",
        &profile.runtime_token_mapping,
        &mut token_entries,
    );
    collect_scalar_entries(
        "extendedTokenMapping",
        &profile.extended_token_mapping,
        &mut token_entries,
    );
    collect_scalar_entries("tokens", &profile.tokens, &mut token_entries);
    let tokens = render_budgeted_entries("High-impact Tokens", token_entries, 2_000, false)?;

    let mut component_entries = Vec::new();
    collect_scalar_entries("components", &profile.components, &mut component_entries);
    let components = render_budgeted_entries(
        "Required Component Recipes",
        component_entries,
        1_800,
        false,
    )?;

    let mut content_entries = Vec::new();
    collect_scalar_entries("brand", &profile.brand, &mut content_entries);
    collect_scalar_entries("content", &profile.content, &mut content_entries);
    let content = render_budgeted_entries("Content and Voice", content_entries, 700, false)?;
    let mut accessibility_entries = Vec::new();
    collect_scalar_entries(
        "accessibility",
        &profile.accessibility,
        &mut accessibility_entries,
    );
    let accessibility =
        render_budgeted_entries("Accessibility", accessibility_entries, 400, false)?;
    let mut governance_entries = Vec::new();
    collect_scalar_entries("governance", &profile.governance, &mut governance_entries);
    let governance = render_budgeted_entries("Governance", governance_entries, 400, false)?;

    let extended_token_count = profile
        .extended_token_mapping
        .as_object()
        .map(|tokens| tokens.len())
        .unwrap_or(0);
    let mut gaps = vec![format!(
        "- Extended tokens declared: {extended_token_count}; see the versioned fidelity report for template support."
    )];
    gaps.push(format!("- Base profile hash: {}", profile.stable_hash()));
    gaps.push(format!(
        "- Overrides hash: {}",
        canonical_json_hash(&profile.overrides)
    ));
    let gaps = render_budgeted_entries("Runtime Capability Gaps", gaps, 500, true)?;

    let capsule = format!(
        "# Design Capsule\n\n{identity}\n\n{source}\n\n{required_rules}\n\n{visual}\n\n{tokens}\n\n{components}\n\n{content}\n\n{accessibility}\n\n{governance}\n\n{gaps}\n"
    );
    if capsule.chars().count() > 10_000 {
        return Err(anyhow!("Design Capsule exceeds the 10000-character budget"));
    }
    Ok(capsule)
}

fn render_budgeted_entries(
    heading: &str,
    entries: Vec<String>,
    budget: usize,
    required: bool,
) -> Result<String> {
    let mut rendered = format!("## {heading}\n\n");
    let mut used = 0usize;
    for entry in entries {
        let entry_chars = entry.chars().count() + 1;
        if used + entry_chars > budget {
            if required {
                return Err(anyhow!(
                    "Design Capsule section {heading} exceeds its budget"
                ));
            }
            continue;
        }
        rendered.push_str(&entry);
        rendered.push('\n');
        used += entry_chars;
    }
    if used == 0 {
        rendered.push_str("- No compact entries fit this section budget\n");
    }
    Ok(rendered.trim_end().to_string())
}

fn collect_scalar_entries(prefix: &str, value: &Value, entries: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort();
            for key in keys {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                collect_scalar_entries(&path, &object[key], entries);
            }
        }
        Value::Array(values) => {
            for value in values {
                entries.push(format!("- {prefix}: {value}"));
            }
        }
        Value::Null => {}
        value => entries.push(format!("- {prefix}: {value}")),
    }
}
