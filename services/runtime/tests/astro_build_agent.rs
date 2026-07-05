use anydesign_runtime::{
    preview::{check_promotion_gate, PromotionGateError},
    profiles::build::promotion_gate_report_from_workspace,
    profiles::build::{run_astro_build, AstroBuildRequest},
    types::{AgentPhase, Brief, ProjectVersionStatus},
    RuntimeStore,
};
use serde_json::json;
use std::{fs, path::PathBuf};

fn website_brief() -> Brief {
    Brief {
        project_type: "website".to_string(),
        audience: "internal design teams".to_string(),
        content_hierarchy: vec!["Design runtime".to_string(), "Workflow".to_string()],
        page_structure: json!([
            {
                "title": "Home",
                "purpose": "Explain the runtime",
                "keyContent": ["hero", "proof"]
            }
        ]),
        visual_direction: "calm technical confidence".to_string(),
        recommended_template: "astro-website".to_string(),
        assumptions: vec![],
        missing_information: vec![],
    }
}

#[tokio::test]
async fn confirmed_brief_generates_astro_project_candidate_and_promoted_preview() {
    let workspace = unique_temp_dir("astro-build");
    let store = RuntimeStore::with_checkpoint_dir(workspace.join("state/checkpoints"));
    let brief_run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(&brief_run.id, website_brief())
        .await
        .unwrap();
    let build_run = store
        .create_run_with_context(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
            Some(brief_id.clone()),
            None,
        )
        .await;

    let output = run_astro_build(
        &store,
        AstroBuildRequest {
            project_id: "project-1".to_string(),
            run_id: build_run.id.clone(),
            brief_id: brief_id.clone(),
            workspace_root: workspace.clone(),
            preview_base_url: "http://preview.local".to_string(),
        },
    )
    .await
    .unwrap();

    assert!(workspace.join("inputs/brief.md").exists());
    assert!(workspace.join("project/package.json").exists());
    assert!(workspace.join("project/astro.config.mjs").exists());
    assert!(workspace.join("project/src/pages/index.astro").exists());
    assert!(
        fs::read_to_string(workspace.join("project/src/pages/index.astro"))
            .unwrap()
            .contains("Design runtime")
    );
    assert!(
        fs::read_to_string(workspace.join("outputs/build/build.log"))
            .unwrap()
            .contains("astro build completed")
    );
    let preview_json = fs::read_to_string(workspace.join("state/preview.json")).unwrap();
    assert!(preview_json.contains("\"accessible\": true"));
    assert!(fs::read_to_string(workspace.join("state/context.md"))
        .unwrap()
        .contains(&output.promoted_version.id));

    assert_eq!(
        output.promoted_version.status,
        ProjectVersionStatus::Promoted
    );
    assert_eq!(
        output.promoted_version.preview_url,
        "http://preview.local/preview/project-1/current"
    );
    assert_eq!(
        store.current_project_version("project-1").await.unwrap().id,
        output.promoted_version.id
    );
    let run = store.get_run(&build_run.id).await.unwrap();
    assert_eq!(
        run.output_version_id,
        Some(output.promoted_version.id.clone())
    );
    assert_eq!(
        run.checkpoint_id.as_deref(),
        Some(output.checkpoint_id.as_str())
    );
    let checkpoint = store.get_checkpoint(&output.checkpoint_id).await.unwrap();
    assert_eq!(checkpoint.brief_version.as_deref(), Some(brief_id.as_str()));
    assert!(checkpoint
        .workspace_snapshot_uri
        .as_deref()
        .is_some_and(|uri| uri.ends_with("outputs/build/source-snapshot.txt")));
    assert_eq!(
        checkpoint.last_known_preview_url.as_deref(),
        Some("http://preview.local/preview/project-1/current")
    );
    let build_result = checkpoint
        .build_result
        .as_ref()
        .expect("first generation checkpoint should capture build result");
    assert_eq!(build_result.version_id, output.promoted_version.id);
    assert_eq!(build_result.status, ProjectVersionStatus::Promoted);
    assert_eq!(
        build_result.preview_url,
        "http://preview.local/preview/project-1/current"
    );
    assert!(build_result
        .source_snapshot_uri
        .as_deref()
        .is_some_and(|uri| uri.ends_with("outputs/build/source-snapshot.txt")));
    assert_eq!(
        build_result.screenshot_id.as_deref(),
        Some("shot-astro-home")
    );

    let event_types = store
        .events(&build_run.id)
        .await
        .into_iter()
        .map(|event| {
            serde_json::to_value(event).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        event_types,
        vec!["preview.rebuilding", "preview.candidate", "preview.updated"]
    );
}

#[tokio::test]
async fn astro_build_rejects_non_astro_template() {
    let workspace = unique_temp_dir("astro-build-reject");
    let store = RuntimeStore::with_checkpoint_dir(workspace.join("state/checkpoints"));
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let mut brief = website_brief();
    brief.recommended_template = "fumadocs-docs".to_string();
    let brief_id = store.write_brief(&run.id, brief).await.unwrap();

    let result = run_astro_build(
        &store,
        AstroBuildRequest {
            project_id: "project-1".to_string(),
            run_id: run.id,
            brief_id,
            workspace_root: workspace,
            preview_base_url: "http://preview.local".to_string(),
        },
    )
    .await;

    assert!(result
        .unwrap_err()
        .to_string()
        .contains("recommendedTemplate=astro-website"));
}

#[test]
fn workspace_promotion_gate_report_detects_failed_build_preview_and_screenshot_artifacts() {
    let workspace = unique_temp_dir("astro-build-gate");
    fs::create_dir_all(workspace.join("outputs/build")).unwrap();
    fs::create_dir_all(workspace.join("outputs/screenshots")).unwrap();
    fs::create_dir_all(workspace.join("state")).unwrap();

    fs::write(workspace.join("outputs/build/build.log"), "Build ok").unwrap();
    fs::write(
        workspace.join("state/preview.json"),
        json!({ "accessible": true }).to_string(),
    )
    .unwrap();
    fs::write(
        workspace.join("outputs/screenshots/shot-ok.json"),
        json!({ "blank": false }).to_string(),
    )
    .unwrap();
    let passing = promotion_gate_report_from_workspace(&workspace, Some("shot-ok"));
    assert!(check_promotion_gate(&passing).is_ok());

    fs::write(
        workspace.join("outputs/build/build.log"),
        "Error: missing import",
    )
    .unwrap();
    let build_failed = promotion_gate_report_from_workspace(&workspace, Some("shot-ok"));
    assert_eq!(
        check_promotion_gate(&build_failed).unwrap_err(),
        PromotionGateError::BuildFailed
    );

    fs::write(workspace.join("outputs/build/build.log"), "Build ok").unwrap();
    fs::write(
        workspace.join("state/preview.json"),
        json!({ "accessible": false }).to_string(),
    )
    .unwrap();
    let preview_failed = promotion_gate_report_from_workspace(&workspace, Some("shot-ok"));
    assert_eq!(
        check_promotion_gate(&preview_failed).unwrap_err(),
        PromotionGateError::PreviewUnreachable
    );

    fs::write(
        workspace.join("state/preview.json"),
        json!({ "accessible": true }).to_string(),
    )
    .unwrap();
    let missing_screenshot = promotion_gate_report_from_workspace(&workspace, Some("missing"));
    assert_eq!(
        check_promotion_gate(&missing_screenshot).unwrap_err(),
        PromotionGateError::ScreenshotMissing
    );

    fs::write(
        workspace.join("outputs/screenshots/shot-blank.json"),
        json!({ "blank": true }).to_string(),
    )
    .unwrap();
    let blank = promotion_gate_report_from_workspace(&workspace, Some("shot-blank"));
    assert_eq!(
        check_promotion_gate(&blank).unwrap_err(),
        PromotionGateError::BlankPage
    );
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "{prefix}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}
