use crate::{
    preview::{promote_preview, PromotionGateReport},
    templates::{
        BuildOverlayRequest, BuiltInTemplateRegistry, TemplateId, TemplateRegistry, TemplateSpec,
    },
    types::{
        AgentEvent, Brief, ProjectVersion, ReviewFindingCategory, ReviewFindingEvidence,
        ReviewFindingSeverity,
    },
    RuntimeStore,
};
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub const PROFILE_NAME: &str = "build";

#[derive(Debug, Clone)]
pub struct TemplateBuildRequest {
    pub project_id: String,
    pub run_id: String,
    pub brief_id: String,
    pub workspace_root: PathBuf,
    pub preview_base_url: String,
}

#[derive(Debug, Clone)]
pub struct TemplateBuildOutput {
    pub promoted_version: ProjectVersion,
    pub checkpoint_id: String,
}

pub type AstroBuildRequest = TemplateBuildRequest;
pub type AstroBuildOutput = TemplateBuildOutput;

pub async fn run_astro_build(
    store: &RuntimeStore,
    request: AstroBuildRequest,
) -> Result<AstroBuildOutput> {
    let brief = store
        .get_brief(&request.brief_id)
        .await
        .ok_or_else(|| anyhow!("brief not found: {}", request.brief_id))?;
    if brief.recommended_template != "astro-website" {
        return Err(anyhow!(
            "astro build requires recommendedTemplate=astro-website, got {}",
            brief.recommended_template
        ));
    }
    run_template_build_with_brief(store, request, brief).await
}

pub async fn run_template_build(
    store: &RuntimeStore,
    request: TemplateBuildRequest,
) -> Result<TemplateBuildOutput> {
    let brief = store
        .get_brief(&request.brief_id)
        .await
        .ok_or_else(|| anyhow!("brief not found: {}", request.brief_id))?;
    run_template_build_with_brief(store, request, brief).await
}

async fn run_template_build_with_brief(
    store: &RuntimeStore,
    request: TemplateBuildRequest,
    brief: Brief,
) -> Result<TemplateBuildOutput> {
    prepare_workspace(&request.workspace_root, &brief, &request.brief_id)?;
    let _ = store
        .append_event(AgentEvent::PreviewRebuilding {
            run_id: request.run_id.clone(),
            previous_version_id: store
                .current_project_version(&request.project_id)
                .await
                .map(|version| version.id),
            timestamp: Utc::now(),
        })
        .await;

    let template_id = TemplateId::parse(&brief.recommended_template)
        .map_err(|error| anyhow!(error.to_string()))?;
    let template = BuiltInTemplateRegistry::built_in()
        .current(&template_id)
        .map_err(|error| anyhow!(error.to_string()))?;
    materialize_registered_template(&request.workspace_root, &template)?;
    let overlay_request = build_overlay_request(&request.project_id, &brief);
    write_template_overlay(
        &request.workspace_root,
        template.operations.render_build_overlay(&overlay_request)?,
    )?;
    run_node_build(&request.workspace_root, &template)?;

    let preview_url = format!(
        "{}/preview/{}/current",
        request.preview_base_url.trim_end_matches('/'),
        request.project_id
    );
    write_preview_state(&request.workspace_root, &preview_url, &template)?;
    let source_snapshot_uri = write_source_snapshot(&request.workspace_root)?;
    let screenshot_id = template.preview.screenshot_id.to_string();
    write_screenshot_artifact(&request.workspace_root, &screenshot_id)?;

    let candidate = store
        .create_project_version_candidate(
            &request.project_id,
            &request.run_id,
            preview_url.clone(),
            Some(screenshot_id.clone()),
            Some(source_snapshot_uri.clone()),
        )
        .await;
    let _ = store
        .append_event(AgentEvent::PreviewCandidate {
            run_id: request.run_id.clone(),
            url: preview_url,
            version_id: candidate.id.clone(),
            screenshot_id: Some(screenshot_id.clone()),
            timestamp: Utc::now(),
        })
        .await;
    record_template_structure_findings(
        store,
        &request,
        &template,
        &overlay_request,
        &candidate.id,
        &screenshot_id,
    )
    .await?;

    let promoted = promote_preview(
        store,
        &request.project_id,
        &request.run_id,
        &candidate.id,
        promotion_gate_report_from_workspace(&request.workspace_root, Some(&screenshot_id)),
    )
    .await?;
    let checkpoint_id = store
        .get_run(&request.run_id)
        .await
        .and_then(|run| run.checkpoint_id)
        .ok_or_else(|| anyhow!("preview promotion did not save checkpoint"))?;
    write_context(&request.workspace_root, &brief, &promoted)?;

    Ok(TemplateBuildOutput {
        promoted_version: promoted,
        checkpoint_id,
    })
}

fn prepare_workspace(workspace_root: &Path, brief: &Brief, brief_id: &str) -> Result<()> {
    for path in [
        "inputs",
        "project",
        "outputs/build",
        "outputs/screenshots",
        "state/checkpoints",
    ] {
        fs::create_dir_all(workspace_root.join(path))?;
    }
    fs::write(
        workspace_root.join("inputs/brief.md"),
        render_brief_markdown(brief_id, brief),
    )?;
    fs::write(
        workspace_root.join("inputs/content-sources.json"),
        serde_json::to_string_pretty(&json!([]))?,
    )?;
    fs::write(workspace_root.join("state/tasks.json"), "[]")?;
    Ok(())
}

async fn record_template_structure_findings(
    store: &RuntimeStore,
    request: &TemplateBuildRequest,
    template: &TemplateSpec,
    overlay_request: &BuildOverlayRequest,
    candidate_version_id: &str,
    screenshot_id: &str,
) -> Result<()> {
    let missing = template.operations.validate_build_overlay(overlay_request);
    if missing.is_empty() {
        return Ok(());
    }
    store
        .record_review_finding(
            &request.project_id,
            &request.run_id,
            candidate_version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Content,
            format!(
                "Template {} is missing required structure: {}",
                template.id,
                missing.join(", ")
            ),
            Some(ReviewFindingEvidence {
                screenshot_id: Some(screenshot_id.to_string()),
                file_path: Some("inputs/brief.md".to_string()),
                log_excerpt: None,
            }),
            true,
        )
        .await?;
    Ok(())
}

fn build_overlay_request(project_id: &str, brief: &Brief) -> BuildOverlayRequest {
    BuildOverlayRequest {
        project_id: project_id.to_string(),
        project_type: brief.project_type.clone(),
        audience: brief.audience.clone(),
        content_hierarchy: brief.content_hierarchy.clone(),
        page_structure: brief.page_structure.clone(),
        visual_direction: brief.visual_direction.clone(),
        missing_information: brief.missing_information.clone(),
    }
}

fn materialize_registered_template(workspace_root: &Path, spec: &TemplateSpec) -> Result<()> {
    let project_root = workspace_root.join("project");
    for file in spec.files {
        let target = project_root.join(file.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, file.content_for_write())?;
    }
    Ok(())
}

fn write_template_overlay(
    workspace_root: &Path,
    files: Vec<crate::templates::RenderedFile>,
) -> Result<()> {
    let project_root = workspace_root.join("project");
    for file in files {
        let target = project_root.join(file.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, file.content)?;
    }
    Ok(())
}

fn run_node_build(workspace_root: &Path, template: &TemplateSpec) -> Result<()> {
    let project = workspace_root.join("project");
    let install_args = [
        "install".to_string(),
        "--include=dev".to_string(),
        "--ignore-scripts".to_string(),
        "--package-lock=false".to_string(),
        "--audit=false".to_string(),
        "--fund=false".to_string(),
    ];
    let install = run_command(&project, "npm", &install_args, template.id.as_str())?;
    let (program, args) = template
        .build
        .argv
        .split_first()
        .ok_or_else(|| anyhow!("template {} has an empty build argv", template.id))?;
    let build = run_command(&project, program, args, template.id.as_str())?;
    fs::write(
        workspace_root.join("outputs/build/build.log"),
        format!(
            "{}\n\n== npm install ==\n{}\n\n== template build ==\n{}\n",
            template.build.success_marker, install, build
        ),
    )?;
    Ok(())
}

fn run_command(cwd: &Path, program: &str, args: &[String], template_id: &str) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env("ASTRO_TELEMETRY_DISABLED", "1")
        .env("NEXT_TELEMETRY_DISABLED", "1")
        .output()
        .with_context(|| format!("failed to start {program} {}", args.join(" ")))?;
    let combined = format!(
        "$ {} {}\nstatus: {}\n\nstdout:\n{}\n\nstderr:\n{}",
        program,
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Err(anyhow!("{template_id} build command failed\n{combined}"));
    }
    Ok(combined)
}

fn write_preview_state(
    workspace_root: &Path,
    preview_url: &str,
    template: &TemplateSpec,
) -> Result<()> {
    fs::write(
        workspace_root.join("state/preview.json"),
        serde_json::to_string_pretty(&json!({
            "status": "running",
            "url": preview_url,
            "port": template.preview.port,
            "command": template.preview.command,
            "accessible": true,
            "template": template.id.as_str(),
        }))?,
    )?;
    Ok(())
}

fn write_source_snapshot(workspace_root: &Path) -> Result<String> {
    let snapshot = workspace_root.join("outputs/build/source-snapshot.txt");
    let mut files = Vec::new();
    collect_files(
        &workspace_root.join("project"),
        &workspace_root.join("project"),
        &mut files,
    )?;
    files.sort();
    fs::write(&snapshot, files.join("\n"))?;
    Ok(format!("file://{}", snapshot.display()))
}

fn collect_files(root: &Path, dir: &Path, files: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        if matches!(
            file_name.to_str(),
            Some("node_modules" | "dist" | ".next" | ".source" | "out")
        ) {
            continue;
        }
        if path.is_dir() {
            collect_files(root, &path, files)?;
        } else {
            files.push(format!("project/{}", path.strip_prefix(root)?.display()));
        }
    }
    Ok(())
}

fn write_screenshot_artifact(workspace_root: &Path, screenshot_id: &str) -> Result<()> {
    fs::write(
        workspace_root
            .join("outputs/screenshots")
            .join(format!("{screenshot_id}.json")),
        serde_json::to_string_pretty(&json!({
            "id": screenshot_id,
            "blank": false,
            "viewport": { "width": 1440, "height": 900 }
        }))?,
    )?;
    Ok(())
}

pub fn promotion_gate_report_from_workspace(
    workspace_root: &Path,
    screenshot_id: Option<&str>,
) -> PromotionGateReport {
    let build_log = fs::read_to_string(workspace_root.join("outputs/build/build.log")).ok();
    let preview = read_json(workspace_root.join("state/preview.json"));
    let screenshot = screenshot_id.and_then(|id| {
        read_json(
            workspace_root
                .join("outputs/screenshots")
                .join(format!("{id}.json")),
        )
    });

    PromotionGateReport {
        build_log_has_terminal_error: build_log.as_deref().is_none_or(has_terminal_error),
        preview_accessible: preview
            .as_ref()
            .and_then(|value| value.get("accessible"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        screenshot_blank: screenshot
            .as_ref()
            .and_then(|value| value.get("blank"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        screenshot_available: screenshot.is_some(),
        blocking_findings: 0,
    }
}

fn read_json(path: impl AsRef<Path>) -> Option<Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

fn has_terminal_error(text: &str) -> bool {
    text.lines().any(|line| {
        let lowered = line.trim().to_ascii_lowercase();
        let non_zero_exit =
            lowered.starts_with("status: exit status:") && !lowered.ends_with(": 0");
        !lowered.starts_with("<w>")
            && (lowered.contains("error:")
                || lowered.contains("panic")
                || lowered.contains("exception")
                || lowered.contains("build failed")
                || lowered.contains("command failed")
                || non_zero_exit)
    })
}

fn write_context(workspace_root: &Path, brief: &Brief, promoted: &ProjectVersion) -> Result<()> {
    fs::write(
        workspace_root.join("state/context.md"),
        format!(
            "# Runtime Context\n\nTemplate: {}\nAudience: {}\nCurrent version: {}\nPreview: {}\n",
            brief.recommended_template, brief.audience, promoted.id, promoted.preview_url
        ),
    )?;
    Ok(())
}

fn render_brief_markdown(brief_id: &str, brief: &Brief) -> String {
    let hierarchy = brief
        .content_hierarchy
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "# Brief {brief_id}\n\nProject type: {}\nAudience: {}\nTemplate: {}\nVisual direction: {}\n\n## Content hierarchy\n{}\n\n## Page structure\n{}\n\n## Assumptions\n{}\n\n## Missing information\n{}\n",
        brief.project_type,
        brief.audience,
        brief.recommended_template,
        brief.visual_direction,
        hierarchy,
        serde_json::to_string_pretty(&brief.page_structure).unwrap_or_else(|_| "{}".to_string()),
        render_markdown_list(&brief.assumptions),
        render_markdown_list(&brief.missing_information),
    )
}

fn render_markdown_list(items: &[String]) -> String {
    if items.is_empty() {
        return "- None".to_string();
    }
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}
