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
    let mut rules = profile
        .get("signatureRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let surface = ctx
        .run
        .design_profile_surface
        .as_deref()
        .unwrap_or("website");
    rules = rules
        .into_iter()
        .filter(|rule| fidelity_rule_applies(rule, surface))
        .collect::<Vec<_>>();
    let component_recipes = read_workspace_json(workspace, ctx, "inputs/component-recipes.json")
        .await
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    rules.extend(component_recipe_fidelity_rules(&component_recipes));
    rules.extend(dcp_craft_pack_fidelity_rules(ctx)?);
    let needs_dom = rules.iter().any(|rule| {
        matches!(
            rule.get("verification")
                .and_then(|verification| verification.get("kind"))
                .and_then(Value::as_str),
            Some("dom" | "computed-style" | "a11y" | "viewport")
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
    let browser_evidence = if rules.iter().any(|rule| {
        matches!(
            rule.get("verification")
                .and_then(|verification| verification.get("kind"))
                .and_then(Value::as_str),
            Some("computed-style" | "a11y" | "viewport")
        )
    }) {
        collect_browser_evidence(Some(ctx), preview_url, &rules).await
    } else {
        json!({ "ok": true, "results": {} })
    };
    let enforced = ctx
        .run
        .design_context_effective_compatibility_mode
        .as_deref()
        == Some("enforced");
    ensure_browser_evidence_available(
        enforced,
        &browser_evidence,
        ctx.run.design_context_verification_environment.as_ref(),
        ctx.run.design_context_verification_policy_id.as_deref(),
    )?;

    let mut assertions = Vec::new();
    let mut required_failed_rule_ids = Vec::new();
    for rule in rules {
        let rule_id = rule
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("unknown-rule")
            .to_string();
        let recipe_id = rule
            .get("_dcpRecipeId")
            .and_then(Value::as_str)
            .map(ToString::to_string);
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
                let values = browser_evidence
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
                let reference_values = browser_evidence
                    .get("results")
                    .and_then(|results| results.get(&rule_id))
                    .and_then(|result| result.get("referenceValues"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|value| value.as_str().map(ToString::to_string))
                    .collect::<Vec<_>>();
                let browser_error = browser_evidence.get("error").and_then(Value::as_str);
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
            "a11y" => {
                let result = browser_evidence
                    .get("results")
                    .and_then(|results| results.get(&rule_id));
                let violations = result
                    .and_then(|result| result.get("violations"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let browser_error = browser_evidence.get("error").and_then(Value::as_str);
                let actual = violations
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" | ");
                let passed = browser_error.is_none() && violations.is_empty();
                let reason = if let Some(error) = browser_error {
                    format!("browser a11y evidence unavailable: {error}")
                } else if passed {
                    "a11y baseline check passed".to_string()
                } else {
                    format!("a11y baseline found {} violation(s)", violations.len())
                };
                (actual.clone(), actual, passed, reason)
            }
            "viewport" => {
                let result = browser_evidence
                    .get("results")
                    .and_then(|results| results.get(&rule_id));
                let scroll_width = result
                    .and_then(|result| result.get("scrollWidth"))
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                let viewport_width = result
                    .and_then(|result| result.get("viewportWidth"))
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                let screenshot_hash = result
                    .and_then(|result| result.get("screenshotSha256"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let screenshot_uri = result
                    .and_then(|result| result.get("screenshotUri"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let browser_error = browser_evidence.get("error").and_then(Value::as_str);
                let passed = browser_error.is_none() && scroll_width <= viewport_width;
                let actual = format!(
                    "scrollWidth={scroll_width};viewportWidth={viewport_width};screenshotSha256={screenshot_hash};screenshotUri={screenshot_uri}"
                );
                let reason = if let Some(error) = browser_error {
                    format!("browser viewport evidence unavailable: {error}")
                } else if passed {
                    format!("no horizontal overflow at {viewport_width}px")
                } else {
                    format!("horizontal overflow: {scroll_width}px > {viewport_width}px")
                };
                (actual.clone(), actual, passed, reason)
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
            "recipeId": recipe_id,
            "priority": if required { "required" } else { "preferred" },
            "kind": kind,
            "route": verification.get("route").cloned().unwrap_or(json!("/")),
            "viewport": verification.get("viewport").cloned().unwrap_or(Value::Null),
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
                recipe_id
                    .as_deref()
                    .map(|recipe_id| format!("ruleId={rule_id};recipeId={recipe_id}"))
                    .unwrap_or_else(|| format!("ruleId={rule_id}")),
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
    let design_context = json!({
        "packageVersion": ctx.run.design_context_package_version,
        "contentHash": ctx.run.design_context_content_hash,
        "artifactManifestHash": ctx.run.design_context_artifact_manifest_hash,
        "materializationHash": ctx.run.design_context_materialization_hash,
        "compilerVersion": ctx.run.design_context_compiler_version,
        "briefHash": ctx.run.design_context_brief_hash,
        "verificationPolicyId": ctx.run.design_context_verification_policy_id,
        "expectedAppRoot": ctx.run.design_context_expected_app_root,
        "declaredEnforcementMode": ctx.run.design_context_declared_enforcement_mode,
        "effectiveCompatibilityMode": ctx.run.design_context_effective_compatibility_mode,
        "verificationEnvironment": ctx.run.design_context_verification_environment,
        "warnings": ctx.run.design_context_warnings,
    });
    let report = json!({
        "version": "design-profile-fidelity@2",
        "status": if required_failed_rule_ids.is_empty() { "passed" } else { "failed" },
        "runId": ctx.run.id,
        "designProfileId": ctx.run.design_profile_id,
        "designProfileVersion": ctx.run.design_profile_version,
        "effectiveProfileHash": ctx.run.design_profile_effective_hash,
        "surface": ctx.run.design_profile_surface,
        "template": ctx.run.design_profile_template,
        "designContext": design_context,
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
    let prior_fidelity_check_count = ctx
        .store
        .conversation_items(&ctx.project_id)
        .await
        .into_iter()
        .filter(|item| {
            item.run_id.as_deref() == Some(&ctx.run.id)
                && item.kind == "design_profile_fidelity_checked"
        })
        .count();
    crate::tools::runtime::record_design_context_metric(
        &ctx.store,
        &ctx.run,
        "design_context_fidelity_pass_rate",
        1,
        json!({
            "status": report.get("status").and_then(Value::as_str).unwrap_or("unknown"),
            "attempt": if prior_fidelity_check_count == 0 { "initial" } else { "repair" },
        }),
    )
    .await;
    for assertion in report["assertions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|assertion| assertion.get("passed").and_then(Value::as_bool) == Some(false))
    {
        let priority = assertion
            .get("priority")
            .and_then(Value::as_str)
            .unwrap_or("preferred");
        let kind = assertion
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        crate::tools::runtime::record_design_context_metric(
            &ctx.store,
            &ctx.run,
            "design_context_recipe_rule_fail_total",
            1,
            json!({
                "ruleId": assertion.get("ruleId").and_then(Value::as_str).unwrap_or("unknown-rule"),
                "recipeId": assertion.get("recipeId").and_then(Value::as_str),
                "kind": kind,
                "priority": priority,
            }),
        )
        .await;
        if priority == "required" && kind == "a11y" {
            crate::tools::runtime::record_design_context_metric(
                &ctx.store,
                &ctx.run,
                "design_context_a11y_required_fail_total",
                1,
                json!({
                    "ruleId": assertion.get("ruleId").and_then(Value::as_str).unwrap_or("unknown-rule"),
                    "severity": if enforced { "blocking" } else { "warning" },
                }),
            )
            .await;
        }
        if priority == "required" && kind == "viewport" {
            crate::tools::runtime::record_design_context_metric(
                &ctx.store,
                &ctx.run,
                "design_context_responsive_required_fail_total",
                1,
                json!({
                    "ruleId": assertion.get("ruleId").and_then(Value::as_str).unwrap_or("unknown-rule"),
                    "viewportPreset": match assertion.get("viewport").and_then(Value::as_u64) {
                        Some(375) => "375",
                        Some(768) => "768",
                        Some(1440) => "1440",
                        _ => "unknown",
                    },
                }),
            )
            .await;
        }
    }
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
                if enforced {
                    ReviewFindingSeverity::Blocking
                } else {
                    ReviewFindingSeverity::Warning
                },
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

fn ensure_browser_evidence_available(
    enforced: bool,
    browser_evidence: &Value,
    verification_environment: Option<&Value>,
    verification_policy_id: Option<&str>,
) -> Result<(), ToolError> {
    if !enforced || browser_evidence.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    Err(ToolError::typed_recoverable(
        format!(
            "Runtime browser verification became unavailable: {}",
            browser_evidence["error"]
                .as_str()
                .unwrap_or("unknown error")
        ),
        "design_verification_runtime_lost",
        json!({
            "verificationEnvironment": verification_environment,
            "policyId": verification_policy_id,
        }),
    ))
}

async fn collect_browser_evidence(
    ctx: Option<&ToolContext>,
    preview_url: &str,
    rules: &[Value],
) -> Value {
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
            matches!(
                verification.get("kind").and_then(Value::as_str),
                Some("computed-style" | "a11y" | "viewport")
            )
            .then(|| {
                    json!({
                        "ruleId": rule.get("id").and_then(Value::as_str).unwrap_or("unknown-rule"),
                        "kind": verification.get("kind").and_then(Value::as_str).unwrap_or_default(),
                        "route": verification.get("route").and_then(Value::as_str).unwrap_or("/"),
                        "selector": verification.get("selector").and_then(Value::as_str).unwrap_or_default(),
                        "property": verification.get("property").and_then(Value::as_str).unwrap_or_default(),
                        "referenceProperty": verification.get("referenceProperty").and_then(Value::as_str),
                        "excludeWithin": verification.get("excludeWithin").and_then(Value::as_str),
                        "check": verification.get("check").and_then(Value::as_str),
                        "viewport": verification.get("viewport").and_then(Value::as_u64),
                    })
                })
        })
        .collect::<Vec<_>>();
    if assertions.is_empty() {
        return json!({ "ok": true, "results": {} });
    }

    let mut input = json!({
        "url": preview_url,
        "assertions": assertions,
    });
    if let Some(ctx) = ctx {
        let screenshot_dir = ctx
            .runtime_storage_dir
            .join("screenshots")
            .join(safe_segment(&ctx.run.project_id))
            .join(safe_segment(&ctx.run.id))
            .join("verification");
        input["viewportScreenshotDir"] = json!(screenshot_dir);
        input["viewportScreenshotUriPrefix"] = json!(format!(
            "runtime://screenshots/{}/{}/verification",
            safe_segment(&ctx.run.project_id),
            safe_segment(&ctx.run.id),
        ));
        if let Some(browser_executable) = ctx
            .run
            .design_context_verification_environment
            .as_ref()
            .and_then(|environment| environment.get("browserExecutable"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            input["browserExecutable"] = json!(browser_executable);
        }
    }
    let collector_executable = ctx
        .and_then(|ctx| ctx.run.design_context_verification_environment.as_ref())
        .and_then(|environment| environment.get("browserCollectorExecutable"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            std::env::var("RUNTIME_BROWSER_COLLECTOR_EXECUTABLE")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "node".to_string());
    let mut command = TokioCommand::new(&collector_executable);
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

fn component_recipe_fidelity_rules(recipes: &[Value]) -> Vec<Value> {
    recipes
        .iter()
        .enumerate()
        .flat_map(|(recipe_index, recipe)| {
            let recipe_id = recipe
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("recipe-{recipe_index}"));
            let priority = recipe
                .get("priority")
                .and_then(Value::as_str)
                .unwrap_or("preferred");
            recipe
                .get("verification")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
                .map(move |(verification_index, verification)| {
                    json!({
                        "id": format!("recipe:{recipe_id}:{verification_index}"),
                        "_dcpRecipeId": recipe_id,
                        "priority": priority,
                        "category": "component",
                        "appliesTo": ["website"],
                        "verification": verification,
                    })
                })
        })
        .collect()
}

fn dcp_craft_pack_fidelity_rules(ctx: &ToolContext) -> Result<Vec<Value>, ToolError> {
    let Some(manifest) = crate::design_context::frozen_run_design_context_manifest(&ctx.run)
        .map_err(|error| {
            ToolError::typed_recoverable(
                format!("frozen Design Context identity is invalid: {error}"),
                "design_context.integrity_failed",
                json!({ "runId": ctx.run.id }),
            )
        })?
    else {
        return Ok(Vec::new());
    };
    let packs = manifest.payload.craft_packs;
    let required = ctx
        .run
        .design_context_effective_compatibility_mode
        .as_deref()
        == Some("enforced");
    let priority = if required { "required" } else { "preferred" };
    let mut rules = Vec::new();
    if packs.iter().any(|pack| pack == "accessibility-baseline") {
        for (id, check) in [
            ("image-alt", "image-alt"),
            ("button-name", "button-name"),
            ("link-name", "link-name"),
        ] {
            rules.push(json!({
                "id": format!("craft:accessibility-baseline:{id}"),
                "priority": priority,
                "category": "accessibility",
                "appliesTo": ["website"],
                "verification": { "kind": "a11y", "route": "/", "check": check },
            }));
        }
    }
    if packs.iter().any(|pack| pack == "responsive-layout") {
        for viewport in [375_u64, 768, 1440] {
            rules.push(json!({
                "id": format!("craft:responsive-layout:no-horizontal-overflow:{viewport}"),
                "priority": priority,
                "category": "responsive",
                "appliesTo": ["website"],
                "verification": {
                    "kind": "viewport",
                    "route": "/",
                    "viewport": viewport,
                    "check": "no-horizontal-overflow",
                },
            }));
        }
    }
    Ok(rules)
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
    use super::{
        compare_fidelity_ratio, compare_fidelity_value, component_recipe_fidelity_rules,
        ensure_browser_evidence_available,
    };
    use serde_json::json;

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

    #[test]
    fn component_recipe_verifications_are_promoted_to_fidelity_rules() {
        let rules = component_recipe_fidelity_rules(&[json!({
            "id": "navigation.primary",
            "priority": "required",
            "verification": [{
                "kind": "dom",
                "selector": "nav[data-primary]",
                "minMatches": 1
            }]
        })]);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["id"], "recipe:navigation.primary:0");
        assert_eq!(rules[0]["_dcpRecipeId"], "navigation.primary");
        assert_eq!(rules[0]["priority"], "required");
        assert_eq!(rules[0]["verification"]["kind"], "dom");
    }

    #[test]
    fn enforced_browser_runtime_loss_is_typed_and_observe_mode_keeps_going() {
        let evidence = json!({ "ok": false, "error": "worker exited" });
        let enforced = ensure_browser_evidence_available(
            true,
            &evidence,
            Some(&json!({ "registryVersion": "runtime-verifier-registry@1" })),
            Some("website-verification@1"),
        )
        .unwrap_err();
        match enforced {
            crate::tools::runtime::ToolError::RecoverableWithMetadata { error_kind, .. } => {
                assert_eq!(error_kind, "design_verification_runtime_lost");
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(ensure_browser_evidence_available(false, &evidence, None, None).is_ok());
    }
}

// remote-fs-boundary: allow-begin browser-evidence-test-fixtures
#[cfg(test)]
mod browser_evidence_tests {
    use super::{collect_browser_evidence, evaluate_design_profile_fidelity};
    use crate::{
        design_context::{
            compile_website_design_context, DesignContextCompileOptions, VerifierRegistry,
        },
        templates::{BuiltInTemplateRegistry, TemplateId, TemplateRegistry},
        tools::{runtime::ToolContext, sandbox::LocalWorkspaceBackend},
        types::{AgentPhase, Brief, DesignProfile},
        RuntimeStore,
    };
    use chrono::Utc;
    use serde_json::json;
    use std::path::Path;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn browser_evidence_collects_a11y_and_fixed_viewport_findings() {
        if !Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome").is_file() {
            return;
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut request = [0_u8; 4096];
                    let _ = stream.read(&mut request).await;
                    let html = "<!doctype html><style>body{margin:0;width:900px}</style><img src='/missing.png'><button></button><a href='/next'></a>";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        html.len(), html
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        let rules = vec![
            json!({
                "id": "a11y-image", "verification": { "kind": "a11y", "route": "/", "check": "image-alt" }
            }),
            json!({
                "id": "a11y-button", "verification": { "kind": "a11y", "route": "/", "check": "button-name" }
            }),
            json!({
                "id": "viewport", "verification": { "kind": "viewport", "route": "/", "viewport": 375, "check": "no-horizontal-overflow" }
            }),
        ];
        let storage = std::env::temp_dir().join(format!(
            "anydesign-browser-evidence-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let store = RuntimeStore::new();
        let run = store
            .create_run(
                "browser-evidence-project".to_string(),
                AgentPhase::Review,
                "review".to_string(),
                "fixture".to_string(),
                Vec::new(),
            )
            .await;
        let mut ctx = ToolContext::new(store, run.clone(), storage.join("workspace"));
        ctx.runtime_storage_dir = storage.clone();
        let evidence =
            collect_browser_evidence(Some(&ctx), &format!("http://{address}/"), &rules).await;
        server.abort();
        assert_eq!(evidence["ok"], json!(true));
        assert!(!evidence["results"]["a11y-image"]["violations"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(!evidence["results"]["a11y-button"]["violations"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(
            evidence["results"]["viewport"]["scrollWidth"]
                .as_u64()
                .unwrap()
                > evidence["results"]["viewport"]["viewportWidth"]
                    .as_u64()
                    .unwrap()
        );
        assert_eq!(
            evidence["results"]["viewport"]["screenshotSha256"]
                .as_str()
                .unwrap()
                .len(),
            64
        );
        assert!(evidence["results"]["viewport"]["screenshotUri"]
            .as_str()
            .unwrap()
            .starts_with("runtime://screenshots/browser-evidence-project/"));
        assert!(storage
            .join("screenshots")
            .join("browser-evidence-project")
            .join(&run.id)
            .join("verification")
            .join("viewport-375.png")
            .is_file());
        let _ = std::fs::remove_dir_all(storage);
    }

    #[tokio::test]
    async fn enforced_craft_packs_emit_required_a11y_and_responsive_findings() {
        if !Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome").is_file() {
            return;
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut request = [0_u8; 4096];
                    let _ = stream.read(&mut request).await;
                    let html = "<!doctype html><style>body{margin:0;width:900px}</style><img src='/missing.png'><button></button><a href='/next'></a>";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        html.len(), html
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        let storage = std::env::temp_dir().join(format!(
            "anydesign-enforced-craft-packs-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let workspace = storage.join("workspace");
        std::fs::create_dir_all(workspace.join("inputs")).unwrap();
        std::fs::write(
            workspace.join("inputs/design-profile.json"),
            r#"{"signatureRules":[]}"#,
        )
        .unwrap();
        let store = RuntimeStore::new();
        let run = store
            .create_run(
                "enforced-craft-packs".to_string(),
                AgentPhase::Build,
                "build".to_string(),
                "fixture".to_string(),
                Vec::new(),
            )
            .await;
        let template = BuiltInTemplateRegistry::built_in()
            .current(&TemplateId::parse("astro-website").unwrap())
            .unwrap();
        let profile = DesignProfile {
            id: "craft-pack-profile".to_string(),
            schema_version: "design-profile@2".to_string(),
            name: "Craft pack fixture".to_string(),
            status: "active".to_string(),
            version: 1,
            scope: json!({ "projectId": "enforced-craft-packs" }),
            source: json!({ "kind": "manual" }),
            product: json!({}),
            brand: json!({}),
            visual: json!({}),
            tokens: json!({}),
            runtime_token_mapping: json!({
                "color.primary": "#2563eb",
                "color.background": "#ffffff"
            }),
            extended_token_mapping: json!({}),
            components: json!({}),
            website_context: json!({
                "enforcementMode": "enforced",
                "craftPacks": ["accessibility-baseline", "responsive-layout"]
            }),
            content: json!({}),
            accessibility: json!({}),
            technical: json!({ "allowedTemplates": ["astro-website"] }),
            governance: json!({}),
            signature_rules: Vec::new(),
            overrides: json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let effective = profile.effective_for("website", "astro-website").unwrap();
        let brief = Brief {
            project_type: "website".to_string(),
            audience: "operators".to_string(),
            content_hierarchy: vec!["hero".to_string()],
            page_structure: json!(["hero"]),
            visual_direction: "clear".to_string(),
            recommended_template: "astro-website".to_string(),
            assumptions: Vec::new(),
            missing_information: Vec::new(),
        };
        let dcp = compile_website_design_context(
            &effective,
            &brief,
            &template,
            &DesignContextCompileOptions {
                enforcement_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        store
            .attach_run_effective_design_profile(
                &run.id,
                &profile,
                Some("website"),
                Some("astro-website"),
            )
            .await
            .unwrap();
        let run = store
            .attach_run_design_context(&run.id, &dcp, &VerifierRegistry::discover())
            .await
            .unwrap();
        let mut ctx = ToolContext::new(store, run, workspace);
        ctx.runtime_storage_dir = storage.clone();
        let report = evaluate_design_profile_fidelity(
            &LocalWorkspaceBackend,
            &ctx,
            &format!("http://{address}/"),
            "craft-pack-shot",
            &json!({ "versionId": "candidate-1" }),
        )
        .await
        .unwrap();
        server.abort();
        let required = report["requiredFailedRuleIds"].as_array().unwrap();
        for rule_id in [
            "craft:accessibility-baseline:image-alt",
            "craft:accessibility-baseline:button-name",
            "craft:accessibility-baseline:link-name",
            "craft:responsive-layout:no-horizontal-overflow:375",
            "craft:responsive-layout:no-horizontal-overflow:768",
        ] {
            assert!(
                required.iter().any(|value| value == rule_id),
                "missing {rule_id}"
            );
        }
        assert!(!required
            .iter()
            .any(|value| { value == "craft:responsive-layout:no-horizontal-overflow:1440" }));
        assert!(storage
            .join("screenshots")
            .join("enforced-craft-packs")
            .join(&ctx.run.id)
            .join("verification")
            .join("craft-responsive-layout-no-horizontal-overflow-375-375.png")
            .is_file());
        let events = ctx.store.events(&ctx.run.id).await;
        assert!(events.iter().any(|event| matches!(
            event,
            crate::types::AgentEvent::MetricRecorded {
                name,
                metadata: Some(metadata),
                ..
            } if name == "design_context_a11y_required_fail_total"
                && metadata["severity"] == "blocking"
                && metadata["mode"] == "enforced"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            crate::types::AgentEvent::MetricRecorded {
                name,
                metadata: Some(metadata),
                ..
            } if name == "design_context_responsive_required_fail_total"
                && metadata["viewportPreset"] == "375"
                && metadata["mode"] == "enforced"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            crate::types::AgentEvent::MetricRecorded {
                name,
                metadata: Some(metadata),
                ..
            } if name == "design_context_recipe_rule_fail_total"
                && metadata["kind"] == "viewport"
                && metadata.get("reason").is_none()
        )));
        let _ = std::fs::remove_dir_all(storage);
    }
}
// remote-fs-boundary: allow-end browser-evidence-test-fixtures
