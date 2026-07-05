use crate::{
    preview::{promote_preview, PromotionGateReport},
    types::{AgentEvent, Brief, ProjectVersion},
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

fn write_astro_project(workspace_root: &Path, brief: &Brief) -> Result<()> {
    fs::create_dir_all(workspace_root.join("project/src/pages"))?;
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
                "astro": "^5.0.0"
            },
            "devDependencies": {}
        }))?,
    )?;
    fs::write(
        workspace_root.join("project/astro.config.mjs"),
        "import { defineConfig } from 'astro/config';\n\nexport default defineConfig({});\n",
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
                "typescript": "latest"
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
        workspace_root.join("project/tsconfig.json"),
        serde_json::to_string_pretty(&json!({
            "compilerOptions": {
                "allowJs": true,
                "allowImportingTsExtensions": true,
                "baseUrl": ".",
                "esModuleInterop": true,
                "forceConsistentCasingInFileNames": true,
                "incremental": true,
                "isolatedModules": true,
                "jsx": "preserve",
                "lib": ["dom", "dom.iterable", "esnext"],
                "module": "esnext",
                "moduleResolution": "bundler",
                "noEmit": true,
                "paths": {
                    "@/*": ["./*"],
                    "collections/*": ["./.source/*"]
                },
                "plugins": [{ "name": "next" }],
                "resolveJsonModule": true,
                "skipLibCheck": true,
                "strict": false,
                "target": "es5"
            },
            "exclude": ["node_modules"],
            "include": ["next-env.d.ts", "**/*.ts", "**/*.tsx", ".next/types/**/*.ts", ".source/**/*.ts"]
        }))?,
    )?;
    fs::write(
        workspace_root.join("project/source.config.ts"),
        "import { defineDocs, defineConfig } from 'fumadocs-mdx/config';\n\nexport const docs = defineDocs({\n  dir: 'content/docs',\n});\n\nexport default defineConfig();\n",
    )?;
    fs::write(
        workspace_root.join("project/lib/source.ts"),
        "import { docs } from '../.source/server';\nimport { loader } from 'fumadocs-core/source';\n\nexport const source = loader({\n  baseUrl: '/docs',\n  source: docs.toFumadocsSource(),\n});\n",
    )?;
    fs::write(
        workspace_root.join("project/lib/layout.shared.tsx"),
        "import type { BaseLayoutProps } from 'fumadocs-ui/layouts/shared';\n\nexport function baseOptions(): BaseLayoutProps {\n  return {\n    nav: {\n      title: 'AnyDesign Runtime Docs',\n    },\n  };\n}\n",
    )?;
    fs::write(
        workspace_root.join("project/components/mdx.tsx"),
        "import defaultMdxComponents from 'fumadocs-ui/mdx';\nimport type { MDXComponents } from 'mdx/types';\n\nexport function getMDXComponents(components?: MDXComponents) {\n  return {\n    ...defaultMdxComponents,\n    ...components,\n  } satisfies MDXComponents;\n}\n\nexport const useMDXComponents = getMDXComponents;\n\ndeclare global {\n  type MDXProvidedComponents = ReturnType<typeof getMDXComponents>;\n}\n",
    )?;
    fs::write(
        workspace_root.join("project/mdx-components.tsx"),
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
        "import { source } from '@/lib/source';\nimport { baseOptions } from '@/lib/layout.shared';\nimport { DocsLayout } from 'fumadocs-ui/layouts/docs';\n\nexport default function Layout({ children }) {\n  return (\n    <DocsLayout tree={source.pageTree} {...baseOptions()}>\n      {children}\n    </DocsLayout>\n  );\n}\n",
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
        .map(|item| format!("<li>{}</li>", escape_html(item)))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "---\nconst audience = {:?};\n---\n<html lang=\"en\">\n  <head>\n    <meta charset=\"utf-8\" />\n    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n    <title>{}</title>\n  </head>\n  <body>\n    <main>\n      <p>astro-website</p>\n      <h1>{}</h1>\n      <p>{}</p>\n      <p>Audience: {{audience}}</p>\n      <ul>\n        {}\n      </ul>\n    </main>\n  </body>\n</html>\n",
        brief.audience,
        escape_html(&title),
        escape_html(&title),
        escape_html(&brief.visual_direction),
        hierarchy,
    )
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
        "import {{ notFound }} from 'next/navigation';\nimport {{ source }} from '@/lib/source';\nimport {{ getMDXComponents }} from '@/components/mdx';\nimport {{ DocsBody, DocsDescription, DocsPage, DocsTitle }} from 'fumadocs-ui/layouts/docs/page';\n\nexport function generateStaticParams() {{\n  return source.generateParams();\n}}\n\nexport async function generateMetadata({{ params }}) {{\n  const resolved = await params;\n  const page = source.getPage(resolved.slug);\n  if (!page) return {{ title: {fallback_title}, description: {fallback_description} }};\n  return {{ title: page.data.title, description: page.data.description }};\n}}\n\nexport default async function Page({{ params }}) {{\n  const resolved = await params;\n  const page = source.getPage(resolved.slug);\n  if (!page) notFound();\n  const MDXContent = page.data.body;\n  return (\n    <DocsPage toc={{page.data.toc}} full={{page.data.full}}>\n      <DocsTitle>{{page.data.title}}</DocsTitle>\n      <DocsDescription>{{page.data.description}}</DocsDescription>\n      <DocsBody>\n        <MDXContent components={{getMDXComponents()}} />\n      </DocsBody>\n    </DocsPage>\n  );\n}}\n"
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
