use super::*;
use crate::{
    acceptance_contract::{validate_staged_candidate, AcceptanceReport},
    artifact_publisher::FileArtifactPublisher,
    runtime_storage::FileAcceptanceReportStore,
};

pub(super) async fn collect_and_persist_acceptance(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    candidate_version_id: &str,
    candidate_manifest_hash: &str,
) -> Result<(AcceptanceReport, String), ToolError> {
    let brief_id = ctx.run.brief_version.as_deref().ok_or_else(|| {
        ToolError::Terminal("candidate acceptance requires a frozen Brief".to_string())
    })?;
    if !ctx.store.is_brief_confirmed(brief_id).await {
        return Err(ToolError::Terminal(
            "candidate acceptance requires a confirmed Brief".to_string(),
        ));
    }
    let contract = ctx
        .store
        .get_acceptance_contract(brief_id)
        .await
        .ok_or_else(|| {
            ToolError::Terminal("candidate acceptance contract is missing".to_string())
        })?;
    let staged_root = FileArtifactPublisher::staged_version_root(
        &ctx.runtime_storage_dir,
        &ctx.project_id,
        candidate_version_id,
    );
    let report = validate_staged_candidate(
        &staged_root,
        &ctx.run.id,
        candidate_version_id,
        candidate_manifest_hash,
        &contract,
    )
    .map_err(|error| ToolError::Terminal(format!("candidate acceptance failed: {error}")))?;
    let report_value = serde_json::to_value(&report).map_err(|error| {
        ToolError::Terminal(format!("acceptance report serialization failed: {error}"))
    })?;
    let report_uri = FileAcceptanceReportStore::new(&ctx.runtime_storage_dir)
        .write(&ctx.project_id, &report)
        .map_err(|error| {
            ToolError::Terminal(format!("acceptance report persistence failed: {error}"))
        })?;
    write_workspace_json(
        workspace,
        ctx,
        "state/acceptance-report.json",
        &report_value,
    )
    .await?;
    let failed_check_ids = report
        .checks
        .iter()
        .filter(|check| check.status == crate::acceptance_contract::AcceptanceCheckStatus::Failed)
        .map(|check| check.id.clone())
        .collect::<Vec<_>>();
    let source_fingerprint = read_workspace_json(workspace, ctx, "outputs/build/latest.json")
        .await
        .and_then(|build| build.get("sourceFingerprint").cloned());
    ctx.store
        .append_conversation_item(
            &ctx.project_id,
            Some(&ctx.run.id),
            "acceptance_validation_checked",
            Some("assistant"),
            format!(
                "Acceptance validation checked: {} failure(s).",
                failed_check_ids.len()
            ),
            Some(json!({
                "status": if report.passed() { "passed" } else { "failed" },
                "sourceFingerprint": source_fingerprint,
                "candidateVersionId": candidate_version_id,
                "candidateManifestHash": candidate_manifest_hash,
                "failedCheckIds": failed_check_ids,
                "reportUri": report_uri.clone(),
            })),
        )
        .await;
    Ok((report, report_uri))
}
