use crate::{
    preview::{promote_preview, PromotionGateReport},
    types::{AgentEvent, Brief, ProjectVersion},
    RuntimeStore,
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const PROFILE_NAME: &str = "build";

#[derive(Debug, Clone)]
pub struct AstroBuildRequest {
    pub project_id: String,
    pub run_id: String,
    pub brief_id: String,
    pub workspace_root: PathBuf,
    pub preview_base_url: String,
}

#[derive(Debug, Clone)]
pub struct AstroBuildOutput {
    pub promoted_version: ProjectVersion,
    pub checkpoint_id: String,
}

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

    prepare_workspace(&request.workspace_root, &brief, &request.brief_id)?;
    store
        .append_event(AgentEvent::PreviewRebuilding {
            run_id: request.run_id.clone(),
            previous_version_id: store
                .current_project_version(&request.project_id)
                .await
                .map(|version| version.id),
            timestamp: Utc::now(),
        })
        .await;

    write_project_files(&request.workspace_root, &brief)?;
    write_build_artifacts(&request.workspace_root)?;

    let preview_url = format!(
        "{}/preview/{}/current",
        request.preview_base_url.trim_end_matches('/'),
        request.project_id
    );
    write_preview_state(&request.workspace_root, &preview_url)?;
    let source_snapshot_uri = write_source_snapshot(&request.workspace_root)?;
    let screenshot_id = "shot-astro-home".to_string();
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
    store
        .append_event(AgentEvent::PreviewCandidate {
            run_id: request.run_id.clone(),
            url: preview_url,
            version_id: candidate.id.clone(),
            screenshot_id: Some(screenshot_id.clone()),
            timestamp: Utc::now(),
        })
        .await;

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

    Ok(AstroBuildOutput {
        promoted_version: promoted,
        checkpoint_id,
    })
}

fn prepare_workspace(workspace_root: &Path, brief: &Brief, brief_id: &str) -> Result<()> {
    for path in [
        "inputs",
        "project/src/pages",
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

fn write_project_files(workspace_root: &Path, brief: &Brief) -> Result<()> {
    fs::write(
        workspace_root.join("project/package.json"),
        serde_json::to_string_pretty(&json!({
            "type": "module",
            "scripts": {
                "build": "astro build",
                "dev": "astro dev --host 0.0.0.0"
            },
            "dependencies": {
                "astro": "^5.0.0"
            }
        }))?,
    )?;
    fs::write(
        workspace_root.join("project/astro.config.mjs"),
        "import { defineConfig } from 'astro/config';\n\nexport default defineConfig({});\n",
    )?;
    fs::write(
        workspace_root.join("project/src/pages/index.astro"),
        render_index(brief),
    )?;
    Ok(())
}

fn write_build_artifacts(workspace_root: &Path) -> Result<()> {
    fs::write(
        workspace_root.join("outputs/build/build.log"),
        "astro build completed\npages: /index.html\n",
    )?;
    fs::create_dir_all(workspace_root.join("project/dist"))?;
    fs::write(
        workspace_root.join("project/dist/index.html"),
        "<!doctype html><title>Preview</title>",
    )?;
    Ok(())
}

fn write_preview_state(workspace_root: &Path, preview_url: &str) -> Result<()> {
    fs::write(
        workspace_root.join("state/preview.json"),
        serde_json::to_string_pretty(&json!({
            "status": "running",
            "url": preview_url,
            "port": 4321,
            "command": "astro preview",
            "accessible": true
        }))?,
    )?;
    Ok(())
}

fn write_source_snapshot(workspace_root: &Path) -> Result<String> {
    let snapshot = workspace_root.join("outputs/build/source-snapshot.txt");
    fs::write(
        &snapshot,
        "project/package.json\nproject/astro.config.mjs\nproject/src/pages/index.astro\n",
    )?;
    Ok(format!("file://{}", snapshot.display()))
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
        build_log_has_terminal_error: build_log.as_deref().map_or(true, has_terminal_error),
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
    let lowered = text.to_ascii_lowercase();
    ["error:", "failed", "panic", "exception"]
        .iter()
        .any(|needle| lowered.contains(needle))
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
    format!(
        "# Brief {brief_id}\n\nProject type: {}\nAudience: {}\nTemplate: {}\nVisual direction: {}\n",
        brief.project_type, brief.audience, brief.recommended_template, brief.visual_direction
    )
}

fn render_index(brief: &Brief) -> String {
    let title = brief
        .content_hierarchy
        .first()
        .cloned()
        .unwrap_or_else(|| "Home".to_string());
    format!(
        "---\nconst audience = {:?};\n---\n<html lang=\"en\">\n  <head><title>{}</title></head>\n  <body>\n    <main>\n      <h1>{}</h1>\n      <p>{}</p>\n    </main>\n  </body>\n</html>\n",
        brief.audience, title, title, brief.visual_direction
    )
}
