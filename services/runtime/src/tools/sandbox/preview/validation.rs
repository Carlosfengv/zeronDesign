use super::preview_fidelity::{collect_browser_evidence, internal_browser_evidence_url};
use super::*;
use crate::{
    artifact_publisher::StagedArtifact,
    artifact_routes::{equivalent_next_not_found_alias, ArtifactRouteManifest},
    generation_contract::{
        GenerationContract, ValidationCheckResult, ValidationCheckStatus, ValidationReport,
        VALIDATION_REPORT_SCHEMA,
    },
    runtime_storage::FileValidationReportStore,
};
use std::collections::{BTreeMap, HashMap};

pub(super) async fn collect_and_persist_generation_validation(
    workspace: &dyn WorkspaceBackend,
    command: Option<&dyn SandboxCommandBackend>,
    ctx: &ToolContext,
    template: &TemplateSpec,
    candidate_version_id: &str,
    candidate_manifest: &str,
    latest_build: &Value,
    staged_artifact: &StagedArtifact,
    preview_url: &str,
) -> Result<(ValidationReport, String), ToolError> {
    let contract = template
        .generation_contract()
        .map_err(|error| ToolError::Terminal(format!("generation contract is invalid: {error}")))?;
    let browser_rules = common_browser_rules(&contract)?;
    let mut entry_route_probe =
        collect_entry_route_probe(workspace, ctx, &contract, latest_build, preview_url).await;
    if entry_route_probe.get("status").and_then(Value::as_str) == Some("failed")
        && entry_route_probe.get("owner").and_then(Value::as_str) == Some("serving")
    {
        let previous_probe = entry_route_probe.clone();
        let restart = match super::preview_lifecycle::restart_static_candidate_preview(
            workspace,
            command,
            ctx,
            latest_build,
            preview_url,
        )
        .await
        {
            Ok(evidence) => evidence,
            Err(error) => json!({
                "attempt": 1,
                "status": "failed",
                "owner": "serving",
                "diagnostic": format!("{error:?}").chars().take(512).collect::<String>(),
            }),
        };
        entry_route_probe =
            collect_entry_route_probe(workspace, ctx, &contract, latest_build, preview_url).await;
        entry_route_probe["previousProbe"] = previous_probe;
        entry_route_probe["servingRestart"] = restart;
    }
    let correlation_digest = crate::types::sha256_hex(
        format!(
            "{}:{}:{}",
            ctx.run.id,
            candidate_version_id,
            latest_build
                .get("candidateManifestHash")
                .and_then(Value::as_str)
                .unwrap_or_default()
        )
        .as_bytes(),
    );
    entry_route_probe["correlationId"] =
        json!(format!("entry-route-probe-{}", &correlation_digest[..16]));
    ctx.store
        .append_conversation_item(
            &ctx.project_id,
            Some(&ctx.run.id),
            "entry_route_probe_checked",
            Some("assistant"),
            format!(
                "Entry route probe {} (owner: {}).",
                entry_route_probe["status"].as_str().unwrap_or("failed"),
                entry_route_probe["owner"].as_str().unwrap_or("unknown")
            ),
            Some(entry_route_probe.clone()),
        )
        .await;
    let browser_evidence = collect_browser_evidence(Some(ctx), preview_url, &browser_rules).await;
    let report_uri =
        FileValidationReportStore::uri(&ctx.project_id, &ctx.run.id, candidate_version_id);
    let report = build_validation_report(
        &ctx.run.id,
        candidate_version_id,
        &contract,
        template.version.as_str(),
        candidate_manifest,
        latest_build,
        staged_artifact,
        browser_evidence,
        entry_route_probe.clone(),
        &report_uri,
    )?;
    let failure_owners = validation_failure_owners(&report, &entry_route_probe);
    ctx.store
        .append_conversation_item(
            &ctx.project_id,
            Some(&ctx.run.id),
            "validation_failure_owner_shadowed",
            Some("assistant"),
            format!(
                "Validation failure ownership shadowed for {} check(s).",
                failure_owners.len()
            ),
            Some(json!({
                "mode": "shadow",
                "legacyDecision": "source_repair",
                "failureOwners": failure_owners,
                "correlationId": entry_route_probe.get("correlationId").cloned(),
                "candidateVersionId": candidate_version_id,
                "candidateManifestHash": latest_build.get("candidateManifestHash").cloned(),
            })),
        )
        .await;
    let report_value = serde_json::to_value(&report).map_err(|error| {
        ToolError::Terminal(format!("validation report serialization failed: {error}"))
    })?;
    let persisted_uri = FileValidationReportStore::new(&ctx.runtime_storage_dir)
        .write(&ctx.project_id, &report)
        .map_err(|error| {
            ToolError::Terminal(format!("validation report persistence failed: {error}"))
        })?;
    write_workspace_json(
        workspace,
        ctx,
        "state/validation-report.json",
        &report_value,
    )
    .await?;
    let failed_check_ids = report
        .promotion_blockers(&contract)
        .into_iter()
        .map(|blocker| blocker.check_id)
        .collect::<Vec<_>>();
    ctx.store
        .append_conversation_item(
            &ctx.project_id,
            Some(&ctx.run.id),
            "generation_validation_checked",
            Some("assistant"),
            format!(
                "Generation validation checked: {} required failure(s).",
                failed_check_ids.len()
            ),
            Some(json!({
                "status": if failed_check_ids.is_empty() { "passed" } else { "failed" },
                "sourceFingerprint": latest_build.get("sourceFingerprint").cloned(),
                "candidateVersionId": candidate_version_id,
                "candidateManifestHash": latest_build.get("candidateManifestHash").cloned(),
                "failedCheckIds": failed_check_ids,
                "reportUri": persisted_uri.clone(),
            })),
        )
        .await;
    Ok((report, persisted_uri))
}

fn common_browser_rules(contract: &GenerationContract) -> Result<Vec<Value>, ToolError> {
    let route = contract
        .effective_route_contract()
        .map_err(|error| ToolError::Terminal(format!("route contract is invalid: {error}")))?
        .entry_route;
    Ok(vec![
        json!({
            "id": "validation:page-health",
            "verification": { "kind": "page", "check": "health", "route": route }
        }),
        json!({
            "id": "validation:a11y-image-alt",
            "verification": { "kind": "a11y", "check": "image-alt", "route": route }
        }),
        json!({
            "id": "validation:a11y-button-name",
            "verification": { "kind": "a11y", "check": "button-name", "route": route }
        }),
        json!({
            "id": "validation:a11y-link-name",
            "verification": { "kind": "a11y", "check": "link-name", "route": route }
        }),
        json!({
            "id": "validation:viewport-mobile",
            "verification": { "kind": "viewport", "viewport": 375, "route": route }
        }),
        json!({
            "id": "validation:viewport-desktop",
            "verification": { "kind": "viewport", "viewport": 1440, "route": route }
        }),
    ])
}

async fn collect_entry_route_probe(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    contract: &GenerationContract,
    latest_build: &Value,
    preview_url: &str,
) -> Value {
    let route_contract = match contract.effective_route_contract() {
        Ok(contract) => contract,
        Err(error) => {
            return json!({ "status": "failed", "owner": "artifact", "reason": "route_contract_invalid", "diagnostic": error });
        }
    };
    let Some(route_manifest_path) = latest_build
        .get("artifactRouteManifestPath")
        .and_then(Value::as_str)
    else {
        return json!({ "status": "failed", "owner": "artifact", "reason": "route_manifest_missing" });
    };
    let route_manifest_text = match workspace
        .read_to_string(ctx, &resolve_path(route_manifest_path, &ctx.workspace_root))
        .await
    {
        Ok(text) => text,
        Err(error) => {
            return json!({ "status": "failed", "owner": "artifact", "reason": "route_manifest_unreadable", "diagnostic": error.to_string() });
        }
    };
    let route_manifest: ArtifactRouteManifest = match serde_json::from_str(&route_manifest_text) {
        Ok(manifest) => manifest,
        Err(error) => {
            return json!({ "status": "failed", "owner": "artifact", "reason": "route_manifest_invalid", "diagnostic": error.to_string() });
        }
    };
    if let Err(error) = route_manifest.validate() {
        return json!({ "status": "failed", "owner": "artifact", "reason": "route_manifest_invalid", "diagnostic": error.to_string() });
    }
    let expected_route_manifest_hash = latest_build
        .get("artifactRouteManifestHash")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if crate::types::sha256_hex(route_manifest_text.as_bytes()) != expected_route_manifest_hash {
        return json!({ "status": "failed", "owner": "artifact", "reason": "route_manifest_hash_mismatch" });
    }
    let Some(target) = route_manifest.resolve(&route_contract.entry_route) else {
        return json!({ "status": "failed", "owner": "artifact", "reason": "entry_route_unresolved", "route": route_contract.entry_route });
    };
    let base = internal_browser_evidence_url(Some(ctx), preview_url);
    let probe_url = format!(
        "{}/{}",
        base.trim_end_matches('/'),
        route_contract.entry_route.trim_start_matches('/')
    );
    let (request_url, host_header) = match entry_route_request_target(
        ctx.remote_workspace,
        &ctx.runtime_browser_proxy_base_url,
        &probe_url,
    ) {
        Ok(target) => target,
        Err(error) => {
            return json!({ "status": "failed", "owner": "runtime", "reason": "probe_target_invalid", "route": route_contract.entry_route, "diagnostic": error });
        }
    };
    let client = reqwest::Client::new();
    let mut request = client.get(request_url).timeout(Duration::from_secs(10));
    if let Some(host_header) = host_header {
        request = request.header(reqwest::header::HOST, host_header);
    }
    let response = match request.timeout(Duration::from_secs(10)).send().await {
        Ok(response) => response,
        Err(error) => {
            return json!({ "status": "failed", "owner": "serving", "reason": "request_failed", "route": route_contract.entry_route, "diagnostic": error.to_string() });
        }
    };
    let http_status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let resolved_path = response
        .headers()
        .get("x-anydesign-artifact-path")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let resolved_hash = response
        .headers()
        .get("x-anydesign-artifact-sha256")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let candidate_hash = response
        .headers()
        .get("x-anydesign-candidate-manifest-hash")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let expected_candidate_hash = latest_build
        .get("candidateManifestHash")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let passed = response.status().is_success()
        && content_type.starts_with("text/html")
        && resolved_path == target.file
        && resolved_hash == target.sha256
        && candidate_hash == expected_candidate_hash;
    json!({
        "status": if passed { "passed" } else { "failed" },
        "owner": if passed { "none" } else { "serving" },
        "reason": if passed { "route_manifest_match" } else { "route_probe_mismatch" },
        "route": route_contract.entry_route,
        "httpStatus": http_status,
        "contentType": content_type,
        "resolvedArtifactPath": resolved_path,
        "resolvedArtifactSha256": resolved_hash,
        "candidateManifestHash": candidate_hash,
    })
}

fn entry_route_request_target(
    remote_workspace: bool,
    capture_base_url: &str,
    probe_url: &str,
) -> Result<(reqwest::Url, Option<String>), String> {
    let probe = reqwest::Url::parse(probe_url)
        .map_err(|error| format!("entry route probe URL is invalid: {error}"))?;
    if !remote_workspace {
        return Ok((probe, None));
    }
    let host = probe
        .host_str()
        .ok_or_else(|| "entry route probe URL has no host".to_string())?;
    let host_header = probe
        .port()
        .map(|port| format!("{host}:{port}"))
        .unwrap_or_else(|| host.to_string());
    let mut direct = reqwest::Url::parse(capture_base_url)
        .map_err(|error| format!("Runtime capture base URL is invalid: {error}"))?;
    direct.set_path(probe.path());
    direct.set_query(probe.query());
    direct.set_fragment(None);
    Ok((direct, Some(host_header)))
}

pub(super) fn validation_failure_owners(
    report: &ValidationReport,
    entry_route_probe: &Value,
) -> BTreeMap<String, String> {
    let mut owners = BTreeMap::new();
    let entry_failure_owner =
        (entry_route_probe.get("status").and_then(Value::as_str) != Some("passed")).then(|| {
            entry_route_probe
                .get("owner")
                .and_then(Value::as_str)
                .unwrap_or("runtime")
        });
    if let Some(owner) = entry_failure_owner {
        owners.insert("entry-route".to_string(), owner.to_string());
    }
    for check in &report.checks {
        if check.status == ValidationCheckStatus::Passed {
            continue;
        }
        let owner = match check.id.as_str() {
            "build" => "build",
            "artifact-integrity" | "duplicate-slugs" => "artifact",
            _ if check.status == ValidationCheckStatus::Unavailable => "runtime",
            _ => entry_failure_owner.unwrap_or("source"),
        };
        owners.insert(check.id.clone(), owner.to_string());
    }
    owners
}

fn build_validation_report(
    run_id: &str,
    candidate_version_id: &str,
    contract: &GenerationContract,
    template_version: &str,
    candidate_manifest: &str,
    latest_build: &Value,
    staged_artifact: &StagedArtifact,
    browser_evidence: Value,
    entry_route_probe: Value,
    report_uri: &str,
) -> Result<ValidationReport, ToolError> {
    let candidate_manifest_value: Value =
        serde_json::from_str(candidate_manifest).map_err(|error| {
            ToolError::typed_recoverable(
                format!("candidate manifest is not valid JSON: {error}"),
                "artifact.candidate_mismatch",
                json!({ "candidateVersionId": candidate_version_id }),
            )
        })?;
    let page = browser_evidence
        .get("results")
        .and_then(|results| results.get("validation:page-health"));
    let mobile = browser_evidence
        .get("results")
        .and_then(|results| results.get("validation:viewport-mobile"));
    let desktop = browser_evidence
        .get("results")
        .and_then(|results| results.get("validation:viewport-desktop"));
    let browser_available = browser_evidence.get("ok").and_then(Value::as_bool) == Some(true);
    let browser_error = browser_evidence
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("browser evidence is unavailable");
    let mut checks = Vec::new();

    let build_success = latest_build
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    append_check(
        &mut checks,
        report_uri,
        "build",
        build_success,
        "latest Runtime-owned build did not succeed",
    );
    let artifact_integrity = staged_artifact.file_count > 0
        && is_sha256(&staged_artifact.candidate_manifest_hash)
        && is_sha256(&staged_artifact.artifact_manifest_hash)
        && candidate_manifest_value
            .get("files")
            .and_then(Value::as_array)
            .is_some_and(|files| !files.is_empty());
    append_check(
        &mut checks,
        report_uri,
        "artifact-integrity",
        artifact_integrity,
        "staged artifact or candidate manifest integrity evidence is incomplete",
    );

    append_browser_check(
        &mut checks,
        report_uri,
        browser_available,
        browser_error,
        "desktop-render",
        viewport_rendered(desktop),
        "desktop viewport did not produce a screenshot and valid metrics",
    );
    append_browser_check(
        &mut checks,
        report_uri,
        browser_available,
        browser_error,
        "mobile-render",
        viewport_rendered(mobile),
        "mobile viewport did not produce a screenshot and valid metrics",
    );
    append_browser_check(
        &mut checks,
        report_uri,
        browser_available,
        browser_error,
        "font-coverage",
        page.is_some_and(|page| {
            page.get("fontsReady").and_then(Value::as_bool) == Some(true)
                && page
                    .get("bodyFontFamily")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty())
                && page.get("textLength").and_then(Value::as_u64).unwrap_or(0) > 0
        }),
        "document fonts were not ready, no body font was resolved, or the page has no text",
    );
    let accessibility_passed = [
        "validation:a11y-image-alt",
        "validation:a11y-button-name",
        "validation:a11y-link-name",
    ]
    .iter()
    .all(|id| {
        browser_evidence
            .get("results")
            .and_then(|results| results.get(*id))
            .and_then(|result| result.get("violations"))
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
    });
    append_browser_check(
        &mut checks,
        report_uri,
        browser_available,
        browser_error,
        "accessibility",
        accessibility_passed,
        "images, buttons, or links are missing accessible names",
    );
    append_browser_check(
        &mut checks,
        report_uri,
        browser_available,
        browser_error,
        "responsive-layout",
        viewport_has_no_overflow(mobile) && viewport_has_no_overflow(desktop),
        "desktop or mobile layout has horizontal overflow",
    );
    let links_passed = page.is_some_and(|page| {
        empty_array(page, "brokenLinks") && empty_array(page, "missingAnchors")
    });
    append_browser_check(
        &mut checks,
        report_uri,
        browser_available,
        browser_error,
        "link-integrity",
        links_passed,
        "same-origin links or fragment anchors are broken",
    );
    append_browser_check(
        &mut checks,
        report_uri,
        browser_available,
        browser_error,
        "internal-links",
        links_passed,
        "documentation contains broken internal links or fragment anchors",
    );
    append_browser_check(
        &mut checks,
        report_uri,
        browser_available,
        browser_error,
        "console-errors",
        page.is_some_and(|page| empty_array(page, "consoleErrors")),
        "browser console or page runtime reported errors",
    );
    append_browser_check(
        &mut checks,
        report_uri,
        browser_available,
        browser_error,
        "metadata",
        page.is_some_and(|page| {
            page.get("title")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
                && page.get("hasViewportMeta").and_then(Value::as_bool) == Some(true)
                && page
                    .get("lang")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty())
        }),
        "page title, language, or viewport metadata is missing",
    );

    let output_matches = latest_build.get("staticOutputName").and_then(Value::as_str)
        == Some(contract.build.output_directory.as_str());
    append_check(
        &mut checks,
        report_uri,
        "mdx-compile",
        build_success && output_matches,
        "documentation MDX/static export did not complete with the contracted output",
    );
    append_browser_check(
        &mut checks,
        report_uri,
        browser_available,
        browser_error,
        "navigation",
        page.is_some_and(|page| {
            page.get("navLinkCount")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 0
                || page
                    .get("internalLinkCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    > 0
        }),
        "documentation navigation contains no internal destinations",
    );
    append_check(
        &mut checks,
        report_uri,
        "duplicate-slugs",
        !candidate_has_duplicate_routes(&candidate_manifest_value),
        "candidate contains duplicate case-insensitive documentation routes",
    );
    append_browser_check(
        &mut checks,
        report_uri,
        browser_available,
        browser_error,
        "heading-anchors",
        page.is_some_and(|page| {
            empty_array(page, "duplicateIds") && empty_array(page, "missingAnchors")
        }),
        "documentation contains duplicate IDs or unresolved heading anchors",
    );
    append_browser_check(
        &mut checks,
        report_uri,
        browser_available,
        browser_error,
        "code-blocks",
        page.is_some_and(|page| empty_array(page, "emptyCodeBlocks")),
        "documentation contains empty code/pre blocks",
    );
    append_browser_check(
        &mut checks,
        report_uri,
        browser_available,
        browser_error,
        "search-index",
        page.and_then(|page| page.get("searchControlPresent"))
            .and_then(Value::as_bool)
            == Some(true),
        "documentation search control/index entry point is unavailable",
    );

    let report = ValidationReport {
        schema_version: VALIDATION_REPORT_SCHEMA.to_string(),
        run_id: run_id.to_string(),
        candidate_version_id: candidate_version_id.to_string(),
        candidate_manifest_hash: staged_artifact.candidate_manifest_hash.clone(),
        artifact_manifest_hash: staged_artifact.artifact_manifest_hash.clone(),
        generation_contract_digest: contract.digest().map_err(|error| {
            ToolError::Terminal(format!("generation contract digest failed: {error}"))
        })?,
        template_version: template_version.to_string(),
        checks,
        evidence: json!({
            "contract": contract,
            "build": latest_build,
            "candidateManifest": candidate_manifest_value,
            "artifact": staged_artifact,
            "entryRouteProbe": entry_route_probe,
            "browser": browser_evidence,
        }),
    };
    report.validate().map_err(|error| {
        ToolError::Terminal(format!("generated validation report is invalid: {error}"))
    })?;
    Ok(report)
}

fn append_check(
    checks: &mut Vec<ValidationCheckResult>,
    report_uri: &str,
    id: &str,
    passed: bool,
    message: &str,
) {
    checks.push(ValidationCheckResult {
        id: id.to_string(),
        status: if passed {
            ValidationCheckStatus::Passed
        } else {
            ValidationCheckStatus::Failed
        },
        message: (!passed).then(|| message.to_string()),
        evidence: passed
            .then(|| vec![format!("{report_uri}#{id}")])
            .unwrap_or_default(),
    });
}

fn append_browser_check(
    checks: &mut Vec<ValidationCheckResult>,
    report_uri: &str,
    browser_available: bool,
    browser_error: &str,
    id: &str,
    passed: bool,
    message: &str,
) {
    if browser_available {
        append_check(checks, report_uri, id, passed, message);
    } else {
        checks.push(ValidationCheckResult {
            id: id.to_string(),
            status: ValidationCheckStatus::Unavailable,
            message: Some(browser_error.to_string()),
            evidence: vec![],
        });
    }
}

fn viewport_rendered(value: Option<&Value>) -> bool {
    value.is_some_and(|value| {
        value
            .get("screenshotSha256")
            .and_then(Value::as_str)
            .is_some_and(is_sha256)
            && value
                .get("viewportWidth")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                >= 320
    })
}

fn viewport_has_no_overflow(value: Option<&Value>) -> bool {
    value.is_some_and(|value| {
        let viewport = value
            .get("viewportWidth")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let document = value
            .get("scrollWidth")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX);
        let body = value
            .get("bodyScrollWidth")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX);
        viewport > 0 && document <= viewport + 1 && body <= viewport + 1
    })
}

fn empty_array(value: &Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn candidate_has_duplicate_routes(manifest: &Value) -> bool {
    let mut routes = HashMap::<String, (&str, &str)>::new();
    for file in manifest
        .get("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(path) = file.get("path").and_then(Value::as_str) else {
            continue;
        };
        let Some(route) = static_route_for_path(path) else {
            continue;
        };
        let sha256 = file
            .get("sha256")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some((existing_path, existing_sha256)) = routes.get(&route).copied() {
            if equivalent_next_not_found_alias(existing_path, existing_sha256, path, sha256)
                .is_none()
            {
                return true;
            }
            continue;
        }
        routes.insert(route, (path, sha256));
    }
    false
}

fn static_route_for_path(path: &str) -> Option<String> {
    let normalized = path.trim_start_matches('/').to_ascii_lowercase();
    if normalized == "index.html" {
        return Some("/".to_string());
    }
    if let Some(prefix) = normalized.strip_suffix("/index.html") {
        return Some(format!("/{}", prefix.trim_matches('/')));
    }
    normalized
        .strip_suffix(".html")
        .map(|prefix| format!("/{}", prefix.trim_matches('/')))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation_contract::ArtifactType;

    fn build(contract: &GenerationContract, browser: Value) -> ValidationReport {
        let manifest = json!({
            "schemaVersion": "candidate-manifest@1",
            "buildId": "build-1",
            "files": [
                { "path": "index.html", "bytes": 100, "sha256": "a".repeat(64) },
                { "path": "docs/index.html", "bytes": 100, "sha256": "b".repeat(64) }
            ]
        });
        build_validation_report(
            "run-1",
            "version-1",
            contract,
            "test-template@1",
            &serde_json::to_string(&manifest).unwrap(),
            &json!({
                "success": true,
                "staticOutputName": contract.build.output_directory,
            }),
            &StagedArtifact {
                project_id: "project-1".to_string(),
                version_id: "version-1".to_string(),
                candidate_manifest_hash: "c".repeat(64),
                artifact_manifest_hash: "d".repeat(64),
                staged_uri: "runtime://staged/one".to_string(),
                file_count: 2,
            },
            browser,
            json!({
                "status": "passed",
                "owner": "none",
                "reason": "route_manifest_match"
            }),
            "runtime://validation-reports/project-1/run-1/version-1.json",
        )
        .unwrap()
    }

    fn passing_browser() -> Value {
        json!({
            "ok": true,
            "results": {
                "validation:page-health": {
                    "title": "Docs",
                    "lang": "en",
                    "hasViewportMeta": true,
                    "textLength": 100,
                    "fontsReady": true,
                    "bodyFontFamily": "Arial",
                    "internalLinkCount": 2,
                    "navLinkCount": 1,
                    "brokenLinks": [],
                    "missingAnchors": [],
                    "duplicateIds": [],
                    "emptyCodeBlocks": [],
                    "searchControlPresent": true,
                    "consoleErrors": []
                },
                "validation:a11y-image-alt": { "violations": [] },
                "validation:a11y-button-name": { "violations": [] },
                "validation:a11y-link-name": { "violations": [] },
                "validation:viewport-mobile": {
                    "screenshotSha256": "e".repeat(64),
                    "viewportWidth": 375,
                    "scrollWidth": 375,
                    "bodyScrollWidth": 375
                },
                "validation:viewport-desktop": {
                    "screenshotSha256": "f".repeat(64),
                    "viewportWidth": 1440,
                    "scrollWidth": 1440,
                    "bodyScrollWidth": 1440
                }
            }
        })
    }

    #[test]
    fn website_and_docs_profiles_pass_from_shared_evidence() {
        let website = GenerationContract::website("next-app", "dist");
        let docs = GenerationContract::docs("fumadocs-docs", "out");

        assert!(build(&website, passing_browser()).can_promote(&website));
        assert!(build(&docs, passing_browser()).can_promote(&docs));
    }

    #[test]
    fn validation_report_binds_candidate_artifact_contract_and_template() {
        let contract = GenerationContract::website("next-app", "dist");
        let report = build(&contract, passing_browser());

        assert_eq!(report.candidate_manifest_hash, "c".repeat(64));
        assert_eq!(report.artifact_manifest_hash, "d".repeat(64));
        assert_eq!(
            report.generation_contract_digest,
            contract.digest().unwrap()
        );
        assert_eq!(report.template_version, "test-template@1");
    }

    #[test]
    fn browser_validation_rules_target_the_artifact_entry_route() {
        for (artifact_type, expected_route) in
            [(ArtifactType::Website, "/"), (ArtifactType::Docs, "/docs/")]
        {
            let contract = match artifact_type {
                ArtifactType::Website => GenerationContract::website("next-app", "dist"),
                ArtifactType::Docs => GenerationContract::docs("fumadocs-docs", "out"),
            };
            let rules = common_browser_rules(&contract).unwrap();
            assert!(rules
                .iter()
                .all(|rule| { rule["verification"]["route"].as_str() == Some(expected_route) }));
        }
    }

    #[test]
    fn unavailable_browser_blocks_required_render_checks() {
        let contract = GenerationContract::website("next-app", "dist");
        let report = build(
            &contract,
            json!({ "ok": false, "error": "chromium unavailable", "results": {} }),
        );

        assert!(!report.can_promote(&contract));
        assert!(report.promotion_blockers(&contract).iter().any(|blocker| {
            blocker.check_id == "desktop-render"
                && blocker.status == ValidationCheckStatus::Unavailable
        }));
    }

    #[test]
    fn duplicate_static_routes_block_docs_promotion() {
        let contract = GenerationContract::docs("fumadocs-docs", "out");
        let mut manifest = json!({
            "schemaVersion": "candidate-manifest@1",
            "buildId": "build-1",
            "files": [
                { "path": "Docs/index.html" },
                { "path": "docs.html" }
            ]
        });
        assert!(candidate_has_duplicate_routes(&manifest));
        manifest["files"][1]["path"] = json!("guide.html");
        assert!(!candidate_has_duplicate_routes(&manifest));
        assert!(contract
            .required_checks
            .contains(&"duplicate-slugs".to_string()));
    }

    #[test]
    fn equivalent_next_not_found_aliases_do_not_fail_duplicate_slug_validation() {
        let manifest = json!({
            "schemaVersion": "candidate-manifest@1",
            "buildId": "build-1",
            "files": [
                { "path": "docs/index.html", "sha256": "a".repeat(64) },
                { "path": "404.html", "sha256": "b".repeat(64) },
                { "path": "404/index.html", "sha256": "b".repeat(64) }
            ]
        });
        assert!(!candidate_has_duplicate_routes(&manifest));
    }

    #[test]
    fn remote_entry_route_probe_connects_to_capture_listener_with_lease_host() {
        let (url, host) = entry_route_request_target(
            true,
            "http://127.0.0.1:8081",
            "http://lease-123.preview.local:8081/docs/",
        )
        .unwrap();

        assert_eq!(url.as_str(), "http://127.0.0.1:8081/docs/");
        assert_eq!(host.as_deref(), Some("lease-123.preview.local:8081"));
    }

    #[test]
    fn failure_owner_shadow_distinguishes_serving_runtime_artifact_and_source() {
        let contract = GenerationContract::docs("fumadocs-docs", "out");
        let mut report = build(&contract, passing_browser());
        report
            .checks
            .iter_mut()
            .find(|check| check.id == "artifact-integrity")
            .unwrap()
            .status = ValidationCheckStatus::Failed;
        report
            .checks
            .iter_mut()
            .find(|check| check.id == "metadata")
            .unwrap()
            .status = ValidationCheckStatus::Failed;
        report
            .checks
            .iter_mut()
            .find(|check| check.id == "desktop-render")
            .unwrap()
            .status = ValidationCheckStatus::Unavailable;

        let owners =
            validation_failure_owners(&report, &json!({ "status": "failed", "owner": "serving" }));
        assert_eq!(owners["entry-route"], "serving");
        assert_eq!(owners["artifact-integrity"], "artifact");
        assert_eq!(owners["metadata"], "serving");
        assert_eq!(owners["desktop-render"], "runtime");

        let owners =
            validation_failure_owners(&report, &json!({ "status": "passed", "owner": "none" }));
        assert_eq!(owners["metadata"], "source");
    }
}
