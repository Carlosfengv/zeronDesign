use super::preview_fidelity::collect_browser_evidence;
use super::*;
use crate::{
    artifact_publisher::StagedArtifact,
    generation_contract::{
        ArtifactType, GenerationContract, ValidationCheckResult, ValidationCheckStatus,
        ValidationReport, VALIDATION_REPORT_SCHEMA,
    },
    runtime_storage::FileValidationReportStore,
};
use std::collections::HashSet;

pub(super) async fn collect_and_persist_generation_validation(
    workspace: &dyn WorkspaceBackend,
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
    let browser_rules = common_browser_rules(contract.artifact_type);
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
        &report_uri,
    )?;
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

fn common_browser_rules(artifact_type: ArtifactType) -> Vec<Value> {
    let route = match artifact_type {
        ArtifactType::Website => "/",
        ArtifactType::Docs => "/docs/",
    };
    vec![
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
    ]
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
    let mut routes = HashSet::new();
    manifest
        .get("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|file| file.get("path").and_then(Value::as_str))
        .filter_map(static_route_for_path)
        .any(|route| !routes.insert(route))
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
        let website = GenerationContract::website("astro-website", "dist");
        let docs = GenerationContract::docs("fumadocs-docs", "out");

        assert!(build(&website, passing_browser()).can_promote(&website));
        assert!(build(&docs, passing_browser()).can_promote(&docs));
    }

    #[test]
    fn validation_report_binds_candidate_artifact_contract_and_template() {
        let contract = GenerationContract::website("astro-website", "dist");
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
            let rules = common_browser_rules(artifact_type);
            assert!(rules
                .iter()
                .all(|rule| { rule["verification"]["route"].as_str() == Some(expected_route) }));
        }
    }

    #[test]
    fn unavailable_browser_blocks_required_render_checks() {
        let contract = GenerationContract::website("astro-website", "dist");
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
}
