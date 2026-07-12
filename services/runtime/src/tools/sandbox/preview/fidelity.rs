use super::*;

pub(super) async fn reject_unchanged_fidelity_republish(
    ctx: &ToolContext,
    build: &Value,
) -> Result<(), ToolError> {
    let current_fingerprint = build
        .get("sourceFingerprint")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if current_fingerprint.is_empty() {
        return Ok(());
    }
    let previous_failed_report = ctx
        .store
        .conversation_items(&ctx.project_id)
        .await
        .into_iter()
        .rev()
        .filter(|item| {
            item.run_id.as_deref() == Some(&ctx.run.id)
                && item.kind == "design_profile_fidelity_checked"
        })
        .filter_map(|item| item.metadata)
        .find(|report| {
            report
                .get("requiredFailedRuleIds")
                .and_then(Value::as_array)
                .is_some_and(|ids| !ids.is_empty())
        });
    let Some(previous_failed_report) = previous_failed_report else {
        return Ok(());
    };
    if previous_failed_report
        .get("sourceFingerprint")
        .and_then(Value::as_str)
        != Some(current_fingerprint)
    {
        return Ok(());
    }
    Err(ToolError::typed_recoverable(
        "preview.publish blocked because project source is unchanged since the failed DesignProfile fidelity check",
        "design_profile.no_source_change_after_fidelity_failure",
        json!({
            "sourceFingerprint": current_fingerprint,
            "requiredFailedRuleIds": previous_failed_report["requiredFailedRuleIds"],
            "suggestedAction": "Read state/design-profile-fidelity.json, edit project source to address the reported selector/property failures, then call preview.publish again. Inspecting or rebuilding unchanged source does not count as a repair."
        }),
    ))
}

pub(super) async fn evaluate_design_profile_fidelity(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    preview_url: &str,
    screenshot_id: &str,
    published: &Value,
) -> Result<Value, ToolError> {
    let Some(profile) = read_workspace_json(workspace, ctx, "inputs/design-profile.json").await
    else {
        return Ok(json!({
            "status": "not_applicable",
            "assertions": [],
            "requiredFailedRuleIds": []
        }));
    };
    let rules = profile
        .get("signatureRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let surface = ctx
        .run
        .design_profile_surface
        .as_deref()
        .unwrap_or("website");
    let rules = rules
        .into_iter()
        .filter(|rule| fidelity_rule_applies(rule, surface))
        .collect::<Vec<_>>();
    let needs_dom = rules.iter().any(|rule| {
        matches!(
            rule.get("verification")
                .and_then(|verification| verification.get("kind"))
                .and_then(Value::as_str),
            Some("dom" | "computed-style")
        )
    });
    let html = if needs_dom && !preview_url.is_empty() {
        match reqwest::get(preview_url).await {
            Ok(response) => match response.error_for_status() {
                Ok(response) => response.text().await.ok(),
                Err(_) => None,
            },
            Err(_) => None,
        }
    } else {
        None
    };
    let contract = read_workspace_json(workspace, ctx, "state/style-contract.json").await;
    let token_file_content = if let Some(token_file) = contract
        .as_ref()
        .and_then(|contract| contract.get("tokenFile"))
        .and_then(Value::as_str)
    {
        let path = resolve_path(token_file, &ctx.workspace_root);
        workspace.read_to_string(ctx, &path).await.ok()
    } else {
        None
    };
    let computed_style_evidence = if rules.iter().any(|rule| {
        rule.get("verification")
            .and_then(|verification| verification.get("kind"))
            .and_then(Value::as_str)
            == Some("computed-style")
    }) {
        collect_computed_style_evidence(preview_url, &rules).await
    } else {
        json!({ "ok": true, "results": {} })
    };

    let mut assertions = Vec::new();
    let mut required_failed_rule_ids = Vec::new();
    for rule in rules {
        let rule_id = rule
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("unknown-rule")
            .to_string();
        let required = rule.get("priority").and_then(Value::as_str) == Some("required");
        let category = rule
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or("component");
        let verification = rule
            .get("verification")
            .cloned()
            .unwrap_or_else(|| json!({ "kind": "missing" }));
        let kind = verification
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("missing");
        let expected = verification
            .get("expected")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let comparator = verification
            .get("comparator")
            .and_then(|value| value.get("kind"))
            .and_then(Value::as_str)
            .unwrap_or("exact");
        let (actual, normalized_actual, passed, reason) = match kind {
            "token" => {
                let token = verification
                    .get("token")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let css_variable = contract
                    .as_ref()
                    .and_then(|contract| contract.get("tokens"))
                    .and_then(|tokens| tokens.get(token))
                    .and_then(Value::as_str);
                let actual = css_variable
                    .and_then(|variable| {
                        token_file_content
                            .as_deref()
                            .and_then(|content| read_css_variable_value(content, variable))
                    })
                    .unwrap_or_default();
                let (passed, normalized, reason) = compare_fidelity_value(
                    actual,
                    expected,
                    comparator,
                    verification.get("comparator"),
                );
                (actual.to_string(), normalized, passed, reason)
            }
            "dom" => {
                let selector = verification
                    .get("selector")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let min_matches = verification
                    .get("minMatches")
                    .and_then(Value::as_u64)
                    .unwrap_or(1);
                let count = html
                    .as_deref()
                    .and_then(|html| {
                        Selector::parse(selector).ok().map(|selector| {
                            ParsedHtml::parse_document(html).select(&selector).count() as u64
                        })
                    })
                    .unwrap_or(0);
                (
                    count.to_string(),
                    count.to_string(),
                    count >= min_matches,
                    format!("matched {count}, required {min_matches}"),
                )
            }
            "source-pattern" => {
                let pattern = verification
                    .get("pattern")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let regex = Regex::new(pattern).ok();
                let mut matches = 0usize;
                for path in verification
                    .get("paths")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    let path = resolve_path(path, &ctx.workspace_root);
                    if let Ok(content) = workspace.read_to_string(ctx, &path).await {
                        if regex.as_ref().is_some_and(|regex| regex.is_match(&content)) {
                            matches += 1;
                        }
                    }
                }
                let visual_only = required
                    && matches!(
                        category,
                        "color"
                            | "typography"
                            | "spacing"
                            | "component"
                            | "composition"
                            | "imagery"
                    );
                (
                    matches.to_string(),
                    matches.to_string(),
                    matches > 0 && !visual_only,
                    if visual_only {
                        "source-pattern cannot alone satisfy a required visual rule".to_string()
                    } else {
                        format!("pattern matched {matches} path(s)")
                    },
                )
            }
            "computed-style" => {
                let values = computed_style_evidence
                    .get("results")
                    .and_then(|results| results.get(&rule_id))
                    .and_then(|result| result.get("values"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|value| value.as_str().map(ToString::to_string))
                    .collect::<Vec<_>>();
                let min_matches = verification
                    .get("minMatches")
                    .and_then(Value::as_u64)
                    .unwrap_or(1) as usize;
                let reference_values = computed_style_evidence
                    .get("results")
                    .and_then(|results| results.get(&rule_id))
                    .and_then(|result| result.get("referenceValues"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|value| value.as_str().map(ToString::to_string))
                    .collect::<Vec<_>>();
                let browser_error = computed_style_evidence.get("error").and_then(Value::as_str);
                let comparisons = values
                    .iter()
                    .enumerate()
                    .map(|(index, value)| {
                        if comparator == "numeric-ratio" {
                            compare_fidelity_ratio(
                                value,
                                reference_values
                                    .get(index)
                                    .map(String::as_str)
                                    .unwrap_or_default(),
                                verification.get("comparator"),
                            )
                        } else {
                            compare_fidelity_value(
                                value,
                                expected,
                                comparator,
                                verification.get("comparator"),
                            )
                        }
                    })
                    .collect::<Vec<_>>();
                let enough_matches = if comparator == "forbidden-anywhere" {
                    true
                } else {
                    values.len() >= min_matches
                };
                let match_policy = verification
                    .get("matchPolicy")
                    .and_then(Value::as_str)
                    .unwrap_or("all");
                let comparisons_pass = if match_policy == "any" {
                    comparisons.iter().any(|(passed, _, _)| *passed)
                } else {
                    comparisons.iter().all(|(passed, _, _)| *passed)
                };
                let passed = browser_error.is_none() && enough_matches && comparisons_pass;
                let normalized = comparisons
                    .iter()
                    .map(|(_, normalized, _)| normalized.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ");
                let reason = if let Some(error) = browser_error {
                    format!("browser-computed evidence unavailable: {error}")
                } else if !enough_matches {
                    format!("matched {}, required {min_matches}", values.len())
                } else if passed {
                    format!(
                        "browser-computed comparison passed for {} match(es)",
                        values.len()
                    )
                } else {
                    format!(
                        "browser-computed comparison failed for {} match(es)",
                        values.len()
                    )
                };
                (values.join(" | "), normalized, passed, reason)
            }
            "visual-review" => (
                screenshot_id.to_string(),
                screenshot_id.to_string(),
                false,
                "visual-review rubric requires a Review finding".to_string(),
            ),
            _ => (
                String::new(),
                String::new(),
                false,
                "unsupported or missing verification kind".to_string(),
            ),
        };
        if required && !passed {
            required_failed_rule_ids.push(rule_id.clone());
        }
        let assertion = json!({
            "ruleId": rule_id,
            "priority": if required { "required" } else { "preferred" },
            "kind": kind,
            "route": verification.get("route").cloned().unwrap_or(json!("/")),
            "selector": verification.get("selector").cloned().unwrap_or(Value::Null),
            "property": verification.get("property").cloned().unwrap_or(Value::Null),
            "rawActual": actual,
            "normalizedActual": normalized_actual,
            "expected": expected,
            "comparator": comparator,
            "passed": passed,
            "reason": reason,
        });
        ctx.store
            .append_audit_record(
                &ctx.project_id,
                &ctx.run.id,
                "design_profile.fidelity_assertion",
                assertion.to_string(),
                if passed { "allow" } else { "deny" },
                format!("ruleId={rule_id}"),
            )
            .await;
        assertions.push(assertion);
    }
    required_failed_rule_ids.sort();
    let output_version_id = published
        .get("versionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let style_contract = read_workspace_json(workspace, ctx, "state/style-contract.json")
        .await
        .unwrap_or_else(|| json!({}));
    let report = json!({
        "version": "design-profile-fidelity@1",
        "runId": ctx.run.id,
        "designProfileId": ctx.run.design_profile_id,
        "designProfileVersion": ctx.run.design_profile_version,
        "effectiveProfileHash": ctx.run.design_profile_effective_hash,
        "surface": ctx.run.design_profile_surface,
        "template": ctx.run.design_profile_template,
        "outputVersionId": output_version_id,
        "sourceFingerprint": read_workspace_json(workspace, ctx, "outputs/build/latest.json")
            .await
            .and_then(|build| build.get("sourceFingerprint").cloned()),
        "repairContext": {
            "styleContractPath": "/workspace/state/style-contract.json",
            "tokenFile": style_contract.get("tokenFile").cloned(),
            "globalCssFile": style_contract.get("globalCssFile").cloned(),
            "componentRoot": style_contract.get("componentRoot").cloned(),
            "instructions": [
                "Edit source that is imported by the current page; do not create a standalone CSS file unless the page imports it.",
                "Prefer the declared globalCssFile for selector and computed-style repairs.",
                "Use only tokens declared by the Style Contract."
            ]
        },
        "previewUrl": preview_url,
        "screenshotId": screenshot_id,
        "assertions": assertions,
        "requiredFailedRuleIds": required_failed_rule_ids,
        "checkedAt": Utc::now(),
    });
    write_workspace_json(
        workspace,
        ctx,
        "state/design-profile-fidelity.json",
        &report,
    )
    .await?;
    ctx.store
        .append_conversation_item(
            &ctx.project_id,
            Some(&ctx.run.id),
            "design_profile_fidelity_checked",
            Some("assistant"),
            format!(
                "DesignProfile fidelity checked: {} required failure(s).",
                required_failed_rule_ids.len()
            ),
            Some(report.clone()),
        )
        .await;
    for assertion in report["assertions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|assertion| {
            assertion.get("priority").and_then(Value::as_str) == Some("required")
                && assertion.get("passed").and_then(Value::as_bool) == Some(false)
        })
    {
        let rule_id = assertion
            .get("ruleId")
            .and_then(Value::as_str)
            .unwrap_or("unknown-rule");
        let reason = assertion
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("fidelity assertion failed");
        let _ = ctx
            .store
            .record_review_finding(
                &ctx.project_id,
                &ctx.run.id,
                output_version_id,
                ReviewFindingSeverity::Blocking,
                ReviewFindingCategory::Visual,
                format!("DesignProfile rule {rule_id} failed: {reason}"),
                Some(ReviewFindingEvidence {
                    screenshot_id: (!screenshot_id.is_empty()).then(|| screenshot_id.to_string()),
                    file_path: Some("state/design-profile-fidelity.json".to_string()),
                    log_excerpt: Some(assertion.to_string()),
                }),
                true,
            )
            .await;
    }
    Ok(report)
}

async fn collect_computed_style_evidence(preview_url: &str, rules: &[Value]) -> Value {
    if preview_url.trim().is_empty() {
        return json!({
            "ok": false,
            "error": "preview URL is missing",
            "results": {}
        });
    }
    let assertions = rules
        .iter()
        .filter_map(|rule| {
            let verification = rule.get("verification")?;
            (verification.get("kind").and_then(Value::as_str) == Some("computed-style"))
                .then(|| {
                    json!({
                        "ruleId": rule.get("id").and_then(Value::as_str).unwrap_or("unknown-rule"),
                        "route": verification.get("route").and_then(Value::as_str).unwrap_or("/"),
                        "selector": verification.get("selector").and_then(Value::as_str).unwrap_or_default(),
                        "property": verification.get("property").and_then(Value::as_str).unwrap_or_default(),
                        "referenceProperty": verification.get("referenceProperty").and_then(Value::as_str),
                        "excludeWithin": verification.get("excludeWithin").and_then(Value::as_str),
                    })
                })
        })
        .collect::<Vec<_>>();
    if assertions.is_empty() {
        return json!({ "ok": true, "results": {} });
    }

    let input = json!({
        "url": preview_url,
        "assertions": assertions,
    });
    let mut command = TokioCommand::new("node");
    command
        .arg("--input-type=module")
        .arg("--eval")
        .arg(include_str!(
            "../../../../scripts/collect-computed-styles.mjs"
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return json!({
                "ok": false,
                "error": format!("failed to start browser evidence collector: {error}"),
                "results": {}
            });
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(error) = stdin.write_all(input.to_string().as_bytes()).await {
            return json!({
                "ok": false,
                "error": format!("failed to write browser evidence input: {error}"),
                "results": {}
            });
        }
    }
    let output = match time::timeout(Duration::from_secs(30), child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return json!({
                "ok": false,
                "error": format!("browser evidence collector failed: {error}"),
                "results": {}
            });
        }
        Err(_) => {
            return json!({
                "ok": false,
                "error": "browser evidence collector timed out",
                "results": {}
            });
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    match serde_json::from_str::<Value>(stdout.trim()) {
        Ok(value) if output.status.success() => value,
        Ok(value) => json!({
            "ok": false,
            "error": value.get("error").and_then(Value::as_str).map(ToString::to_string).unwrap_or_else(|| {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                if stderr.is_empty() {
                    "browser evidence collector exited unsuccessfully".to_string()
                } else {
                    format!("browser evidence collector exited unsuccessfully: {stderr}")
                }
            }),
            "results": value.get("results").cloned().unwrap_or_else(|| json!({}))
        }),
        Err(error) => json!({
            "ok": false,
            "error": format!("invalid browser evidence output: {error}"),
            "results": {}
        }),
    }
}

fn fidelity_rule_applies(rule: &Value, surface: &str) -> bool {
    match rule.get("appliesTo") {
        Some(Value::String(value)) => value == "all",
        Some(Value::Array(values)) => values.iter().any(|value| value.as_str() == Some(surface)),
        _ => false,
    }
}

fn read_css_variable_value<'a>(content: &'a str, css_variable: &str) -> Option<&'a str> {
    let marker = format!("{css_variable}:");
    let start = content.find(&marker)? + marker.len();
    let end = start + content[start..].find(';')?;
    Some(content[start..end].trim())
}

fn compare_fidelity_value(
    actual: &str,
    expected: &str,
    comparator: &str,
    comparator_value: Option<&Value>,
) -> (bool, String, String) {
    let normalized_actual = normalize_fidelity_value(actual);
    let normalized_expected = normalize_fidelity_value(expected);
    let passed = match comparator {
        "contains" => normalized_actual.contains(&normalized_expected),
        "color-equivalent" => normalize_css_color(actual) == normalize_css_color(expected),
        "numeric-tolerance" => {
            let tolerance = comparator_value
                .and_then(|value| value.get("tolerance"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            match (parse_css_number(actual), parse_css_number(expected)) {
                (Some(actual), Some(expected)) => (actual - expected).abs() <= tolerance,
                _ => false,
            }
        }
        "forbidden-anywhere" => {
            !normalized_actual.contains(&normalized_expected)
                && normalize_css_color(actual) != normalize_css_color(expected)
        }
        _ => normalized_actual == normalized_expected,
    };
    (
        passed,
        normalized_actual,
        if passed {
            "comparison passed".to_string()
        } else {
            format!("comparison failed against normalized expected {normalized_expected}")
        },
    )
}

fn compare_fidelity_ratio(
    actual: &str,
    reference: &str,
    comparator_value: Option<&Value>,
) -> (bool, String, String) {
    let expected_ratio = comparator_value
        .and_then(|value| value.get("ratio"))
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let tolerance = comparator_value
        .and_then(|value| value.get("tolerance"))
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let ratio = match (parse_css_number(actual), parse_css_number(reference)) {
        (Some(actual), Some(reference)) if reference.abs() > f64::EPSILON => {
            Some(actual / reference)
        }
        _ => None,
    };
    let passed = ratio.is_some_and(|ratio| (ratio - expected_ratio).abs() <= tolerance);
    let normalized = ratio
        .map(|ratio| format!("{ratio:.4}"))
        .unwrap_or_else(|| "invalid-ratio".to_string());
    let reason = if passed {
        "ratio comparison passed".to_string()
    } else {
        format!(
            "ratio comparison failed for {actual} / {reference}; expected {expected_ratio} +/- {tolerance}"
        )
    };
    (passed, normalized, reason)
}

fn normalize_fidelity_value(value: &str) -> String {
    value
        .trim()
        .trim_matches(['\'', '"'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn normalize_css_color(value: &str) -> String {
    let value = normalize_fidelity_value(value);
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() == 3 {
            return format!("#{0}{0}{1}{1}{2}{2}", &hex[0..1], &hex[1..2], &hex[2..3]);
        }
        if hex.len() == 6 {
            return value;
        }
    }
    if let Some(captures) = Regex::new(
        r"^rgba?\(\s*([0-9.]+)\s*,\s*([0-9.]+)\s*,\s*([0-9.]+)(?:\s*,\s*([0-9.]+))?\s*\)$",
    )
    .expect("valid CSS color regex")
    .captures(&value)
    {
        let channels = [1, 2, 3]
            .into_iter()
            .filter_map(|index| captures.get(index)?.as_str().parse::<f64>().ok())
            .map(|channel| channel.round().clamp(0.0, 255.0) as u8)
            .collect::<Vec<_>>();
        if channels.len() == 3 {
            let alpha = captures
                .get(4)
                .and_then(|value| value.as_str().parse::<f64>().ok())
                .unwrap_or(1.0);
            if (alpha - 1.0).abs() < 0.001 {
                return format!("#{:02x}{:02x}{:02x}", channels[0], channels[1], channels[2]);
            }
            return format!(
                "rgba({},{},{},{alpha:.3})",
                channels[0], channels[1], channels[2]
            );
        }
    }
    value
}

fn parse_css_number(value: &str) -> Option<f64> {
    let number = value
        .trim()
        .chars()
        .take_while(|character| character.is_ascii_digit() || matches!(character, '.' | '-'))
        .collect::<String>();
    number.parse().ok()
}

#[cfg(test)]
mod fidelity_comparator_tests {
    use super::{compare_fidelity_ratio, compare_fidelity_value};

    #[test]
    fn color_equivalent_matches_browser_rgb_with_hex() {
        assert!(
            compare_fidelity_value("rgb(102, 58, 243)", "#663af3", "color-equivalent", None,).0
        );
    }

    #[test]
    fn forbidden_anywhere_rejects_equivalent_browser_color() {
        assert!(
            !compare_fidelity_value("rgb(102, 58, 243)", "#663af3", "forbidden-anywhere", None,).0
        );
        assert!(
            compare_fidelity_value("rgba(0, 0, 0, 0)", "#663af3", "forbidden-anywhere", None,).0
        );
    }

    #[test]
    fn numeric_ratio_compares_css_length_roles_across_font_sizes() {
        let comparator = serde_json::json!({
            "kind": "numeric-ratio",
            "ratio": 0.10,
            "tolerance": 0.01
        });
        assert!(compare_fidelity_ratio("1.5px", "15px", Some(&comparator)).0);
        assert!(compare_fidelity_ratio("1.7px", "17px", Some(&comparator)).0);
        assert!(!compare_fidelity_ratio("normal", "17px", Some(&comparator)).0);
    }
}
