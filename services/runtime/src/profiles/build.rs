use crate::{
    preview::{promote_preview, PromotionGateReport},
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

    let template = TemplateKind::from_key(&brief.recommended_template)?;
    match template {
        TemplateKind::AstroWebsite => write_astro_project(&request.workspace_root, &brief)?,
        TemplateKind::FumadocsDocs => {
            write_fumadocs_project(&request.workspace_root, &brief, &request.project_id)?
        }
    }
    run_node_build(&request.workspace_root, template)?;

    let preview_url = format!(
        "{}/preview/{}/current",
        request.preview_base_url.trim_end_matches('/'),
        request.project_id
    );
    write_preview_state(&request.workspace_root, &preview_url, template)?;
    let source_snapshot_uri = write_source_snapshot(&request.workspace_root)?;
    let screenshot_id = template.screenshot_id().to_string();
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
    record_docs_structure_findings(
        store,
        &request,
        &brief,
        template,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateKind {
    AstroWebsite,
    FumadocsDocs,
}

impl TemplateKind {
    fn from_key(key: &str) -> Result<Self> {
        match key {
            "astro-website" => Ok(Self::AstroWebsite),
            "fumadocs-docs" => Ok(Self::FumadocsDocs),
            _ => Err(anyhow!("template build is not implemented for {key}")),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::AstroWebsite => "astro website",
            Self::FumadocsDocs => "fumadocs docs",
        }
    }

    fn build_success_marker(self) -> &'static str {
        match self {
            Self::AstroWebsite => "astro build completed",
            Self::FumadocsDocs => "fumadocs docs build completed",
        }
    }

    fn preview_command(self) -> &'static str {
        match self {
            Self::AstroWebsite => "astro preview",
            Self::FumadocsDocs => "serve out --listen 3000",
        }
    }

    fn preview_port(self) -> u16 {
        match self {
            Self::AstroWebsite => 4321,
            Self::FumadocsDocs => 3000,
        }
    }

    fn screenshot_id(self) -> &'static str {
        match self {
            Self::AstroWebsite => "shot-astro-home",
            Self::FumadocsDocs => "shot-fumadocs-home",
        }
    }
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

async fn record_docs_structure_findings(
    store: &RuntimeStore,
    request: &TemplateBuildRequest,
    brief: &Brief,
    template: TemplateKind,
    candidate_version_id: &str,
    screenshot_id: &str,
) -> Result<()> {
    if template != TemplateKind::FumadocsDocs {
        return Ok(());
    }
    let missing = missing_docs_structure_requirements(brief);
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
                "Docs template is missing required structure: {}",
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

fn missing_docs_structure_requirements(brief: &Brief) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if brief.project_type != "docs" {
        missing.push("projectType=docs");
    }
    if brief.content_hierarchy.len() < 2 {
        missing.push("navigation with at least one content page");
    }
    let pages = brief.page_structure.as_array();
    if pages.is_none_or(Vec::is_empty) {
        missing.push("pageStructure");
    }
    if !pages.is_some_and(|pages| pages.iter().any(is_content_page)) {
        missing.push("content page coverage");
    }
    if !brief.missing_information.is_empty() {
        missing.push("resolved docs requirements");
    }
    missing
}

fn is_content_page(page: &Value) -> bool {
    let title = page
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let purpose = page
        .get("purpose")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let key_content_count = page
        .get("keyContent")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    key_content_count > 0
        && !matches!(title.as_str(), "home" | "homepage" | "overview" | "index")
        && !purpose.contains("homepage")
}

fn write_astro_project(workspace_root: &Path, brief: &Brief) -> Result<()> {
    fs::create_dir_all(workspace_root.join("project/src/pages"))?;
    fs::create_dir_all(workspace_root.join("project/src/styles"))?;
    fs::write(
        workspace_root.join("project/package.json"),
        serde_json::to_string_pretty(&json!({
            "type": "module",
            "scripts": {
                "build": "astro build",
                "dev": "astro dev --host 0.0.0.0",
                "preview": "astro preview --host 0.0.0.0"
            },
            "dependencies": {
                "astro": "^5.0.0",
                "tailwindcss": "^4.3.2"
            },
            "devDependencies": {}
        }))?,
    )?;
    fs::write(
        workspace_root.join("project/astro.config.mjs"),
        "import { defineConfig } from 'astro/config';\n\nexport default defineConfig({});\n",
    )?;
    fs::write(
        workspace_root.join("project/src/styles/global.css"),
        render_astro_global_css(),
    )?;
    fs::write(
        workspace_root.join("project/src/pages/index.astro"),
        render_astro_index(brief),
    )?;
    Ok(())
}

fn write_fumadocs_project(workspace_root: &Path, brief: &Brief, project_id: &str) -> Result<()> {
    for path in [
        "project/app/docs/[[...slug]]",
        "project/app/docs",
        "project/components",
        "project/content/docs",
        "project/lib",
    ] {
        fs::create_dir_all(workspace_root.join(path))?;
    }
    fs::write(
        workspace_root.join("project/package.json"),
        serde_json::to_string_pretty(&json!({
            "type": "module",
            "packageManager": "npm@10.9.0",
            "scripts": {
                "build": "next build --webpack",
                "dev": "next dev --hostname 0.0.0.0",
                "preview": "serve out --listen 3000"
            },
            "dependencies": {
                "fumadocs-core": "^16.10.7",
                "fumadocs-mdx": "^15.0.13",
                "fumadocs-ui": "^16.10.7",
                "next": "^16.2.10",
                "react": "^19.2.7",
                "react-dom": "^19.2.7",
                "serve": "^14.2.5"
            },
            "devDependencies": {
                "@tailwindcss/postcss": "^4.3.2",
                "@types/mdx": "latest",
                "@types/node": "latest",
                "@types/react": "latest",
                "postcss": "^8.5.6",
                "tailwindcss": "^4.3.2",
                "typescript": "5.9.3"
            }
        }))?,
    )?;
    fs::write(
        workspace_root.join("project/postcss.config.mjs"),
        "const config = {\n  plugins: {\n    '@tailwindcss/postcss': {},\n  },\n};\n\nexport default config;\n",
    )?;
    fs::write(
        workspace_root.join("project/next.config.mjs"),
        format!(
            "import {{ createMDX }} from 'fumadocs-mdx/next';\n\n/** @type {{import('next').NextConfig}} */\nconst nextConfig = {{\n  output: 'export',\n  reactStrictMode: true,\n  assetPrefix: '/artifacts/{project_id}/current',\n}};\n\nconst withMDX = createMDX();\n\nexport default withMDX(nextConfig);\n"
        ),
    )?;
    fs::write(
        workspace_root.join("project/source.config.ts"),
        "import { defineDocs, defineConfig } from 'fumadocs-mdx/config';\n\nexport const docs = defineDocs({\n  dir: 'content/docs',\n});\n\nexport default defineConfig();\n",
    )?;
    fs::write(
        workspace_root.join("project/lib/source.js"),
        "import { docs } from '../.source/server';\nimport { loader } from 'fumadocs-core/source';\n\nexport const source = loader({\n  baseUrl: '/docs',\n  source: docs.toFumadocsSource(),\n});\n",
    )?;
    fs::write(
        workspace_root.join("project/lib/layout.shared.jsx"),
        "export function baseOptions() {\n  return {\n    nav: {\n      title: 'AnyDesign Runtime Docs',\n    },\n  };\n}\n",
    )?;
    fs::write(
        workspace_root.join("project/components/mdx.jsx"),
        "import defaultMdxComponents from 'fumadocs-ui/mdx';\n\nexport function getMDXComponents(components = {}) {\n  return {\n    ...defaultMdxComponents,\n    ...components,\n  };\n}\n\nexport const useMDXComponents = getMDXComponents;\n",
    )?;
    fs::write(
        workspace_root.join("project/mdx-components.jsx"),
        "export { useMDXComponents } from './components/mdx';\n",
    )?;
    fs::write(
        workspace_root.join("project/app/global.css"),
        "@import 'tailwindcss';\n@import 'fumadocs-ui/css/neutral.css';\n@import 'fumadocs-ui/css/preset.css';\n",
    )?;
    fs::write(
        workspace_root.join("project/app/layout.jsx"),
        "import './global.css';\nimport { RootProvider } from 'fumadocs-ui/provider/next';\n\nexport default function RootLayout({ children }) {\n  return (\n    <html lang=\"en\" suppressHydrationWarning>\n      <body className=\"flex min-h-screen flex-col\">\n        <RootProvider>{children}</RootProvider>\n      </body>\n    </html>\n  );\n}\n",
    )?;
    fs::write(
        workspace_root.join("project/app/page.jsx"),
        "export default function Home() {\n  return (\n    <main>\n      <h1>AnyDesign Runtime Docs</h1>\n      <a href=\"/docs\">Open docs</a>\n    </main>\n  );\n}\n",
    )?;
    fs::write(
        workspace_root.join("project/app/docs/layout.jsx"),
        "import { source } from '../../lib/source';\nimport { baseOptions } from '../../lib/layout.shared';\nimport { DocsLayout } from 'fumadocs-ui/layouts/docs';\n\nexport default function Layout({ children }) {\n  return (\n    <DocsLayout tree={source.pageTree} {...baseOptions()}>\n      {children}\n    </DocsLayout>\n  );\n}\n",
    )?;
    fs::write(
        workspace_root.join("project/app/docs/[[...slug]]/page.jsx"),
        render_fumadocs_page(brief),
    )?;
    write_docs_content_files(workspace_root, brief)?;
    Ok(())
}

fn run_node_build(workspace_root: &Path, template: TemplateKind) -> Result<()> {
    let project = workspace_root.join("project");
    let install = run_command(
        &project,
        "npm",
        &[
            "install",
            "--include=dev",
            "--ignore-scripts",
            "--package-lock=false",
            "--audit=false",
            "--fund=false",
        ],
        template,
    )?;
    let build = run_command(&project, "npm", &["run", "build"], template)?;
    fs::write(
        workspace_root.join("outputs/build/build.log"),
        format!(
            "{}\n\n== npm install ==\n{}\n\n== npm run build ==\n{}\n",
            template.build_success_marker(),
            install,
            build
        ),
    )?;
    Ok(())
}

fn run_command(cwd: &Path, program: &str, args: &[&str], template: TemplateKind) -> Result<String> {
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
        return Err(anyhow!(
            "{} build command failed\n{combined}",
            template.label()
        ));
    }
    Ok(combined)
}

fn write_preview_state(
    workspace_root: &Path,
    preview_url: &str,
    template: TemplateKind,
) -> Result<()> {
    fs::write(
        workspace_root.join("state/preview.json"),
        serde_json::to_string_pretty(&json!({
            "status": "running",
            "url": preview_url,
            "port": template.preview_port(),
            "command": template.preview_command(),
            "accessible": true,
            "template": match template {
                TemplateKind::AstroWebsite => "astro-website",
                TemplateKind::FumadocsDocs => "fumadocs-docs",
            }
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

fn render_astro_index(brief: &Brief) -> String {
    let title = brief
        .content_hierarchy
        .first()
        .cloned()
        .unwrap_or_else(|| "AnyDesign Runtime".to_string());
    let hierarchy = brief
        .content_hierarchy
        .iter()
        .enumerate()
        .map(|(index, item)| {
            format!(
                "<article class=\"deco-card group\">\n          <span class=\"deco-step\">{}</span>\n          <h3>{}</h3>\n          <p>{}</p>\n        </article>",
                roman_numeral(index),
                escape_html(item),
                escape_html(&brief.visual_direction)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "---\nimport '../styles/global.css';\nconst audience = {:?};\n---\n<html lang=\"en\">\n  <head>\n    <meta charset=\"utf-8\" />\n    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n    <title>{}</title>\n    <link rel=\"preconnect\" href=\"https://fonts.googleapis.com\" />\n    <link rel=\"preconnect\" href=\"https://fonts.gstatic.com\" crossorigin />\n    <link href=\"https://fonts.googleapis.com/css2?family=Josefin+Sans:wght@400;500;600;700&family=Marcellus&display=swap\" rel=\"stylesheet\" />\n  </head>\n  <body class=\"min-h-screen bg-[#0A0A0A] text-[#F2F0E4]\">\n    <main class=\"deco-shell\">\n      <section class=\"deco-hero\" aria-labelledby=\"page-title\">\n        <div class=\"deco-sunburst\" aria-hidden=\"true\"></div>\n        <p class=\"deco-kicker\">astro-website</p>\n        <h1 id=\"page-title\">{}</h1>\n        <p class=\"deco-lede\">{}</p>\n        <p class=\"deco-audience\">Audience: {{audience}}</p>\n        <div class=\"deco-actions\" aria-label=\"Primary actions\">\n          <a class=\"deco-button deco-button-solid\" href=\"#system\">View System</a>\n          <a class=\"deco-button deco-button-outline\" href=\"#components\">Components</a>\n        </div>\n      </section>\n\n      <section id=\"system\" class=\"deco-section\" aria-labelledby=\"system-title\">\n        <div class=\"deco-section-heading\">\n          <span aria-hidden=\"true\"></span>\n          <h2 id=\"system-title\">Design System</h2>\n          <span aria-hidden=\"true\"></span>\n        </div>\n        <div class=\"deco-grid\">\n          {}\n        </div>\n      </section>\n\n      <section id=\"components\" class=\"deco-section deco-component-band\" aria-labelledby=\"components-title\">\n        <div class=\"deco-section-heading\">\n          <span aria-hidden=\"true\"></span>\n          <h2 id=\"components-title\">Component Language</h2>\n          <span aria-hidden=\"true\"></span>\n        </div>\n        <div class=\"deco-component-grid\">\n          <div class=\"deco-card deco-card-feature\">\n            <span class=\"deco-diamond\" aria-hidden=\"true\"><span></span></span>\n            <h3>Buttons</h3>\n            <p>Sharp corners, gold borders, theatrical hover glow, and all-caps precision.</p>\n          </div>\n          <div class=\"deco-card deco-card-feature\">\n            <span class=\"deco-diamond\" aria-hidden=\"true\"><span></span></span>\n            <h3>Cards</h3>\n            <p>Double frames, stepped corner brackets, charcoal panels, and measured ornament.</p>\n          </div>\n          <div class=\"deco-card deco-card-feature\">\n            <span class=\"deco-diamond\" aria-hidden=\"true\"><span></span></span>\n            <h3>Inputs</h3>\n            <p>Transparent fields, gold underlines, champagne text, and mechanical focus states.</p>\n          </div>\n        </div>\n      </section>\n    </main>\n  </body>\n</html>\n",
        brief.audience,
        escape_html(&title),
        escape_html(&title),
        escape_html(&brief.visual_direction),
        hierarchy,
    )
}

fn render_astro_global_css() -> &'static str {
    r#"@import "tailwindcss";

:root {
  --deco-obsidian: #0A0A0A;
  --deco-champagne: #F2F0E4;
  --deco-charcoal: #141414;
  --deco-gold: #D4AF37;
  --deco-blue: #1E3D59;
  --deco-pewter: #888888;
  --deco-gold-glow: rgba(212, 175, 55, 0.28);
  color-scheme: dark;
  font-family: "Josefin Sans", ui-sans-serif, system-ui, sans-serif;
}

* {
  box-sizing: border-box;
}

html {
  background: var(--deco-obsidian);
}

body {
  margin: 0;
  min-height: 100vh;
  background:
    radial-gradient(circle at 50% 14%, rgba(212, 175, 55, 0.18), transparent 28rem),
    repeating-linear-gradient(45deg, rgba(212, 175, 55, 0.045) 0 1px, transparent 1px 28px),
    repeating-linear-gradient(-45deg, rgba(212, 175, 55, 0.035) 0 1px, transparent 1px 28px),
    var(--deco-obsidian);
  color: var(--deco-champagne);
}

a {
  color: inherit;
}

.deco-shell {
  position: relative;
  isolation: isolate;
  width: min(100%, 1280px);
  margin: 0 auto;
  padding: 32px clamp(16px, 4vw, 48px) 72px;
}

.deco-shell::before,
.deco-shell::after {
  content: "";
  position: fixed;
  top: 0;
  bottom: 0;
  width: 1px;
  background: linear-gradient(transparent, rgba(212, 175, 55, 0.55), transparent);
  pointer-events: none;
}

.deco-shell::before {
  left: clamp(16px, 5vw, 72px);
}

.deco-shell::after {
  right: clamp(16px, 5vw, 72px);
}

.deco-hero {
  position: relative;
  display: grid;
  min-height: 78vh;
  place-items: center;
  overflow: hidden;
  border: 3px double rgba(212, 175, 55, 0.76);
  background:
    linear-gradient(180deg, rgba(20, 20, 20, 0.82), rgba(10, 10, 10, 0.94)),
    radial-gradient(circle at center, rgba(212, 175, 55, 0.16), transparent 48%);
  clip-path: polygon(0 24px, 24px 24px, 24px 0, calc(100% - 24px) 0, calc(100% - 24px) 24px, 100% 24px, 100% calc(100% - 24px), calc(100% - 24px) calc(100% - 24px), calc(100% - 24px) 100%, 24px 100%, 24px calc(100% - 24px), 0 calc(100% - 24px));
  padding: clamp(64px, 12vw, 128px) clamp(20px, 6vw, 80px);
  text-align: center;
}

.deco-hero > * {
  position: relative;
  z-index: 1;
}

.deco-sunburst {
  position: absolute;
  inset: -18%;
  opacity: 0.42;
  background:
    repeating-conic-gradient(from -6deg at 50% 50%, rgba(212, 175, 55, 0.28) 0deg 3deg, transparent 3deg 12deg);
  mask-image: radial-gradient(circle at center, black, transparent 62%);
  pointer-events: none;
}

.deco-kicker,
.deco-audience {
  margin: 0;
  color: var(--deco-gold);
  font-size: 0.78rem;
  font-weight: 700;
  letter-spacing: 0.28em;
  text-transform: uppercase;
}

.deco-hero h1 {
  max-width: 920px;
  margin: 24px 0 0;
  color: var(--deco-gold);
  font-family: "Marcellus", Georgia, serif;
  font-size: clamp(3.1rem, 9vw, 7.2rem);
  font-weight: 400;
  letter-spacing: 0.18em;
  line-height: 0.92;
  text-transform: uppercase;
  text-shadow: 0 0 26px rgba(212, 175, 55, 0.22);
}

.deco-lede {
  max-width: 760px;
  margin: 32px auto 0;
  color: var(--deco-champagne);
  font-size: clamp(1.05rem, 2vw, 1.3rem);
  line-height: 1.75;
}

.deco-audience {
  margin-top: 24px;
  color: var(--deco-pewter);
}

.deco-actions {
  display: flex;
  flex-wrap: wrap;
  justify-content: center;
  gap: 16px;
  margin-top: 36px;
}

.deco-button {
  display: inline-flex;
  min-height: 48px;
  align-items: center;
  justify-content: center;
  border: 2px solid var(--deco-gold);
  padding: 0 24px;
  color: var(--deco-gold);
  font-size: 0.8rem;
  font-weight: 700;
  letter-spacing: 0.2em;
  text-decoration: none;
  text-transform: uppercase;
  transition: background-color 320ms ease, box-shadow 320ms ease, color 320ms ease, transform 320ms ease;
}

.deco-button:hover,
.deco-button:focus-visible {
  box-shadow: 0 0 24px rgba(212, 175, 55, 0.42);
  transform: translateY(-2px);
}

.deco-button-solid {
  background: linear-gradient(135deg, #D4AF37, #F2E8C4 50%, #B48924);
  color: var(--deco-obsidian);
}

.deco-button-outline:hover,
.deco-button-outline:focus-visible {
  background: var(--deco-gold);
  color: var(--deco-obsidian);
}

.deco-section {
  padding: clamp(72px, 11vw, 128px) 0 0;
}

.deco-section-heading {
  display: grid;
  grid-template-columns: minmax(48px, 96px) auto minmax(48px, 96px);
  align-items: center;
  justify-content: center;
  gap: 20px;
  margin-bottom: 40px;
  text-align: center;
}

.deco-section-heading span {
  height: 1px;
  background: var(--deco-gold);
  box-shadow: 0 0 12px var(--deco-gold-glow);
}

.deco-section-heading h2 {
  margin: 0;
  color: var(--deco-gold);
  font-family: "Marcellus", Georgia, serif;
  font-size: clamp(1.6rem, 4vw, 3rem);
  font-weight: 400;
  letter-spacing: 0.2em;
  text-transform: uppercase;
}

.deco-grid,
.deco-component-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 24px;
}

.deco-card {
  position: relative;
  min-height: 220px;
  overflow: hidden;
  border: 1px solid rgba(212, 175, 55, 0.34);
  background:
    linear-gradient(180deg, rgba(20, 20, 20, 0.96), rgba(10, 10, 10, 0.96)),
    var(--deco-charcoal);
  padding: 28px;
  transition: border-color 420ms ease, box-shadow 420ms ease, transform 420ms ease;
}

.deco-card::before,
.deco-card::after {
  content: "";
  position: absolute;
  width: 34px;
  height: 34px;
  opacity: 0.74;
  transition: opacity 420ms ease;
}

.deco-card::before {
  top: 8px;
  left: 8px;
  border-top: 2px solid var(--deco-gold);
  border-left: 2px solid var(--deco-gold);
}

.deco-card::after {
  right: 8px;
  bottom: 8px;
  border-right: 2px solid var(--deco-gold);
  border-bottom: 2px solid var(--deco-gold);
}

.deco-card:hover {
  border-color: var(--deco-gold);
  box-shadow: 0 0 22px rgba(212, 175, 55, 0.22);
  transform: translateY(-8px);
}

.deco-card:hover::before,
.deco-card:hover::after {
  opacity: 1;
}

.deco-step {
  display: inline-flex;
  margin-bottom: 24px;
  color: var(--deco-gold);
  font-family: "Marcellus", Georgia, serif;
  font-size: 0.88rem;
  letter-spacing: 0.22em;
}

.deco-card h3 {
  margin: 0;
  color: var(--deco-gold);
  font-family: "Marcellus", Georgia, serif;
  font-size: 1.4rem;
  font-weight: 400;
  letter-spacing: 0.16em;
  line-height: 1.25;
  text-transform: uppercase;
}

.deco-card p {
  margin: 18px 0 0;
  color: var(--deco-pewter);
  font-size: 1rem;
  line-height: 1.7;
}

.deco-component-band {
  padding-bottom: 32px;
}

.deco-card-feature {
  min-height: 260px;
  text-align: center;
}

.deco-diamond {
  display: inline-grid;
  width: 56px;
  height: 56px;
  place-items: center;
  margin-bottom: 28px;
  border: 2px solid var(--deco-gold);
  transform: rotate(45deg);
}

.deco-diamond span {
  display: block;
  width: 22px;
  height: 22px;
  border: 1px solid var(--deco-gold);
  background: rgba(212, 175, 55, 0.16);
  transform: rotate(0deg);
}

@media (max-width: 900px) {
  .deco-grid,
  .deco-component-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}

@media (max-width: 640px) {
  .deco-shell {
    padding-inline: 16px;
  }

  .deco-hero {
    min-height: 72vh;
  }

  .deco-grid,
  .deco-component-grid {
    grid-template-columns: 1fr;
  }

  .deco-section-heading {
    grid-template-columns: 48px auto 48px;
    gap: 12px;
  }
}
"#
}

fn roman_numeral(index: usize) -> &'static str {
    const NUMERALS: [&str; 12] = [
        "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X", "XI", "XII",
    ];
    NUMERALS.get(index).copied().unwrap_or("XII")
}

fn render_fumadocs_page(brief: &Brief) -> String {
    let fallback_title = js_string(
        brief
            .content_hierarchy
            .first()
            .map(String::as_str)
            .unwrap_or("AnyDesign Runtime Docs"),
    );
    let fallback_description = js_string(&brief.audience);
    format!(
        "import {{ notFound }} from 'next/navigation';\nimport {{ source }} from '../../../lib/source';\nimport {{ getMDXComponents }} from '../../../components/mdx';\nimport {{ DocsBody, DocsDescription, DocsPage, DocsTitle }} from 'fumadocs-ui/layouts/docs/page';\n\nexport function generateStaticParams() {{\n  return source.generateParams();\n}}\n\nexport async function generateMetadata({{ params }}) {{\n  const resolved = await params;\n  const page = source.getPage(resolved.slug);\n  if (!page) return {{ title: {fallback_title}, description: {fallback_description} }};\n  return {{ title: page.data.title, description: page.data.description }};\n}}\n\nexport default async function Page({{ params }}) {{\n  const resolved = await params;\n  const page = source.getPage(resolved.slug);\n  if (!page) notFound();\n  const MDXContent = page.data.body;\n  return (\n    <DocsPage toc={{page.data.toc}} full={{page.data.full}}>\n      <DocsTitle>{{page.data.title}}</DocsTitle>\n      <DocsDescription>{{page.data.description}}</DocsDescription>\n      <DocsBody>\n        <MDXContent components={{getMDXComponents()}} />\n      </DocsBody>\n    </DocsPage>\n  );\n}}\n"
    )
}

fn write_docs_content_files(workspace_root: &Path, brief: &Brief) -> Result<()> {
    let docs_dir = workspace_root.join("project/content/docs");
    fs::write(
        docs_dir.join("index.mdx"),
        format!(
            "---\ntitle: {}\ndescription: {}\n---\n\n# {}\n\n{}\n",
            escape_yaml(
                brief
                    .content_hierarchy
                    .first()
                    .map(String::as_str)
                    .unwrap_or("AnyDesign Runtime Docs")
            ),
            escape_yaml(&brief.audience),
            brief
                .content_hierarchy
                .first()
                .map(String::as_str)
                .unwrap_or("AnyDesign Runtime Docs"),
            brief.visual_direction
        ),
    )?;
    fs::write(
        docs_dir.join("runtime-flow.mdx"),
        "---\ntitle: Runtime Flow\n---\n\n# Runtime Flow\n\nBrief confirmation leads to sandbox build and preview promotion.\n",
    )?;
    fs::write(
        docs_dir.join("workspace-boundary.mdx"),
        "---\ntitle: Workspace Boundary\n---\n\n# Workspace Boundary\n\nThe workspace boundary is the sandbox plus the PVC-backed `/workspace` directory.\n",
    )?;
    fs::write(
        docs_dir.join("stream-events.mdx"),
        "---\ntitle: Stream Events\n---\n\n# Stream Events\n\nThe runtime emits structured generation events for every visible transition.\n",
    )?;
    fs::write(
        docs_dir.join("meta.json"),
        serde_json::to_string_pretty(&json!({
            "title": "AnyDesign Runtime Docs",
            "pages": ["index", "runtime-flow", "workspace-boundary", "stream-events"]
        }))?,
    )?;
    Ok(())
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_yaml(value: &str) -> String {
    value.replace('"', "\\\"")
}

fn js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}
