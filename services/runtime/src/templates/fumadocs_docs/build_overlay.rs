use crate::templates::{BuildOverlayRequest, RenderedFile};
use serde_json::{json, Value};

pub(super) fn render(request: &BuildOverlayRequest) -> Vec<RenderedFile> {
    let title = request
        .content_hierarchy
        .first()
        .map(String::as_str)
        .unwrap_or("AnyDesign Runtime Docs");
    vec![
        RenderedFile {
            path: "next.config.mjs".to_string(),
            content: "import { createMDX } from 'fumadocs-mdx/next';\n\n/** @type {import('next').NextConfig} */\nconst nextConfig = {\n  output: 'export',\n  reactStrictMode: true,\n};\n\nconst withMDX = createMDX();\n\nexport default withMDX(nextConfig);\n"
                .to_string(),
        },
        RenderedFile {
            path: "app/docs/[[...slug]]/page.jsx".to_string(),
            content: render_page(title, &request.audience),
        },
        RenderedFile {
            path: "content/docs/index.mdx".to_string(),
            content: format!(
                "---\ntitle: {}\ndescription: {}\n---\n\n# {}\n\n{}\n",
                escape_yaml(title),
                escape_yaml(&request.audience),
                title,
                request.visual_direction
            ),
        },
        RenderedFile {
            path: "content/docs/runtime-flow.mdx".to_string(),
            content: "---\ntitle: Runtime Flow\n---\n\n# Runtime Flow\n\nBrief confirmation leads to sandbox build and preview promotion.\n".to_string(),
        },
        RenderedFile {
            path: "content/docs/workspace-boundary.mdx".to_string(),
            content: "---\ntitle: Workspace Boundary\n---\n\n# Workspace Boundary\n\nThe workspace boundary is the sandbox plus the PVC-backed `/workspace` directory.\n".to_string(),
        },
        RenderedFile {
            path: "content/docs/stream-events.mdx".to_string(),
            content: "---\ntitle: Stream Events\n---\n\n# Stream Events\n\nThe runtime emits structured generation events for every visible transition.\n".to_string(),
        },
        RenderedFile {
            path: "content/docs/meta.json".to_string(),
            content: serde_json::to_string_pretty(&json!({
                "title": "AnyDesign Runtime Docs",
                "pages": ["index", "runtime-flow", "workspace-boundary", "stream-events"]
            }))
            .expect("static docs metadata must serialize"),
        },
    ]
}

pub(super) fn validate(request: &BuildOverlayRequest) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if request.project_type != "docs" {
        missing.push("projectType=docs");
    }
    if request.content_hierarchy.len() < 2 {
        missing.push("navigation with at least one content page");
    }
    let pages = request.page_structure.as_array();
    if pages.is_none_or(Vec::is_empty) {
        missing.push("pageStructure");
    }
    if !pages.is_some_and(|pages| pages.iter().any(is_content_page)) {
        missing.push("content page coverage");
    }
    if !request.missing_information.is_empty() {
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

fn render_page(title: &str, audience: &str) -> String {
    let fallback_title = js_string(title);
    let fallback_description = js_string(audience);
    format!(
        "import {{ notFound }} from 'next/navigation';\nimport {{ source }} from '../../../lib/source';\nimport {{ getMDXComponents }} from '../../../components/mdx';\nimport {{ DocsBody, DocsDescription, DocsPage, DocsTitle }} from 'fumadocs-ui/layouts/docs/page';\n\nexport function generateStaticParams() {{\n  return source.generateParams();\n}}\n\nexport async function generateMetadata({{ params }}) {{\n  const resolved = await params;\n  const page = source.getPage(resolved.slug);\n  if (!page) return {{ title: {fallback_title}, description: {fallback_description} }};\n  return {{ title: page.data.title, description: page.data.description }};\n}}\n\nexport default async function Page({{ params }}) {{\n  const resolved = await params;\n  const page = source.getPage(resolved.slug);\n  if (!page) notFound();\n  const MDXContent = page.data.body;\n  return (\n    <DocsPage toc={{page.data.toc}} full={{page.data.full}}>\n      <DocsTitle>{{page.data.title}}</DocsTitle>\n      <DocsDescription>{{page.data.description}}</DocsDescription>\n      <DocsBody>\n        <MDXContent components={{getMDXComponents()}} />\n      </DocsBody>\n    </DocsPage>\n  );\n}}\n"
    )
}

fn escape_yaml(value: &str) -> String {
    value.replace('"', "\\\"")
}

fn js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}
