mod build_overlay;

use super::{
    BuildOverlayRequest, BuildSpec, FrameworkId, ManifestHash, MutationPolicySpec, PreviewSpec,
    RenderPageRequest, RenderedFile, SandboxExecutionProfileId, SandboxExecutionProfileRef,
    SandboxExecutionProfileVersion, StyleContractSpec, StyleTokenSpec, TemplateCapabilities,
    TemplateFile, TemplateFileRole, TemplateId, TemplateOperationError, TemplateOperations,
    TemplateSpec, TemplateVersion, TemplateWriteMode,
};
use crate::artifact_manifest::ArtifactDeliverySpec;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

macro_rules! asset {
    ($path:literal, $role:expr) => {
        TemplateFile {
            path: $path,
            content: include_str!(concat!("files/", $path)),
            trim_final_newline: false,
            role: $role,
            write_mode: TemplateWriteMode::ReplaceOnInit,
        }
    };
}

pub static FILES: &[TemplateFile] = &[
    asset!("package.json", TemplateFileRole::PackageManifest),
    asset!("package-lock.json", TemplateFileRole::Lockfile),
    asset!("astro.config.mjs", TemplateFileRole::FrameworkConfig),
    asset!("tsconfig.json", TemplateFileRole::FrameworkConfig),
    asset!("src/pages/index.astro", TemplateFileRole::Source),
    asset!("src/styles/tokens.css", TemplateFileRole::Style),
    asset!("src/styles/global.css", TemplateFileRole::Style),
    asset!("src/components/ui/Button.astro", TemplateFileRole::Source),
];

pub static STYLE_TOKENS: &[StyleTokenSpec] = &[
    StyleTokenSpec {
        name: "color.background",
        css_variable: "--runtime-bg",
    },
    StyleTokenSpec {
        name: "color.surface",
        css_variable: "--runtime-surface",
    },
    StyleTokenSpec {
        name: "color.surfaceStrong",
        css_variable: "--runtime-surface-strong",
    },
    StyleTokenSpec {
        name: "color.text",
        css_variable: "--runtime-text",
    },
    StyleTokenSpec {
        name: "color.muted",
        css_variable: "--runtime-muted",
    },
    StyleTokenSpec {
        name: "color.primary",
        css_variable: "--runtime-primary",
    },
    StyleTokenSpec {
        name: "color.primaryContrast",
        css_variable: "--runtime-primary-contrast",
    },
    StyleTokenSpec {
        name: "color.action",
        css_variable: "--runtime-action",
    },
    StyleTokenSpec {
        name: "color.actionContrast",
        css_variable: "--runtime-action-contrast",
    },
    StyleTokenSpec {
        name: "color.authSubmit",
        css_variable: "--runtime-auth-submit",
    },
    StyleTokenSpec {
        name: "color.border",
        css_variable: "--runtime-border",
    },
    StyleTokenSpec {
        name: "radius.card",
        css_variable: "--runtime-radius-card",
    },
    StyleTokenSpec {
        name: "radius.control",
        css_variable: "--runtime-radius-control",
    },
    StyleTokenSpec {
        name: "font.sans",
        css_variable: "--runtime-font-sans",
    },
    StyleTokenSpec {
        name: "shadow.soft",
        css_variable: "--runtime-shadow-soft",
    },
    StyleTokenSpec {
        name: "font.display",
        css_variable: "--runtime-font-display",
    },
    StyleTokenSpec {
        name: "font.mono",
        css_variable: "--runtime-font-mono",
    },
    StyleTokenSpec {
        name: "type.display.size",
        css_variable: "--runtime-type-display-size",
    },
    StyleTokenSpec {
        name: "type.display.lineHeight",
        css_variable: "--runtime-type-display-line-height",
    },
    StyleTokenSpec {
        name: "type.display.letterSpacing",
        css_variable: "--runtime-type-display-tracking",
    },
    StyleTokenSpec {
        name: "type.body.letterSpacing",
        css_variable: "--runtime-type-body-tracking",
    },
    StyleTokenSpec {
        name: "spacing.pageGutter",
        css_variable: "--runtime-spacing-page-gutter",
    },
    StyleTokenSpec {
        name: "spacing.section",
        css_variable: "--runtime-spacing-section",
    },
    StyleTokenSpec {
        name: "spacing.cardPadding",
        css_variable: "--runtime-spacing-card-padding",
    },
    StyleTokenSpec {
        name: "spacing.gridCell",
        css_variable: "--runtime-spacing-grid-cell",
    },
    StyleTokenSpec {
        name: "radius.input",
        css_variable: "--runtime-radius-input",
    },
    StyleTokenSpec {
        name: "radius.badge",
        css_variable: "--runtime-radius-badge",
    },
    StyleTokenSpec {
        name: "radius.largeCard",
        css_variable: "--runtime-radius-large-card",
    },
    StyleTokenSpec {
        name: "gradient.display",
        css_variable: "--runtime-gradient-display",
    },
    StyleTokenSpec {
        name: "gradient.ambient",
        css_variable: "--runtime-gradient-ambient",
    },
    StyleTokenSpec {
        name: "shadow.cardStrong",
        css_variable: "--runtime-shadow-card-strong",
    },
];

struct AstroWebsiteOperations;

static OPERATIONS: AstroWebsiteOperations = AstroWebsiteOperations;

impl TemplateOperations for AstroWebsiteOperations {
    fn name(&self) -> &'static str {
        "astro-website"
    }

    fn supports_render_page(&self) -> bool {
        true
    }

    fn render_page(
        &self,
        request: &RenderPageRequest,
    ) -> Result<Vec<RenderedFile>, TemplateOperationError> {
        let relative_page_path = page_relative_path(&request.route)?;
        Ok(vec![RenderedFile {
            path: format!("src/pages/{}", relative_page_path.to_string_lossy()),
            content: render_page(request, &relative_page_path),
        }])
    }

    fn render_build_overlay(
        &self,
        request: &BuildOverlayRequest,
    ) -> Result<Vec<RenderedFile>, TemplateOperationError> {
        Ok(vec![
            RenderedFile {
                path: "src/styles/global.css".to_string(),
                content: build_overlay::render_global_css().to_string(),
            },
            RenderedFile {
                path: "src/pages/index.astro".to_string(),
                content: build_overlay::render_index(request),
            },
        ])
    }
}

pub fn spec() -> TemplateSpec {
    TemplateSpec {
        id: TemplateId::parse("astro-website").unwrap(),
        version: TemplateVersion::parse("astro-website@runtime-p3").unwrap(),
        manifest_sha256: ManifestHash::parse(
            "7374f4f493c49752bbcbdad49992b02d089f79c1f01784c42fa7224668136e3f",
        )
        .unwrap(),
        framework: FrameworkId::parse("astro").unwrap(),
        surface: "website",
        default_title: "AnyDesign Runtime Website",
        sandbox_execution_profile: SandboxExecutionProfileRef {
            id: SandboxExecutionProfileId::parse("astro-website").unwrap(),
            version: SandboxExecutionProfileVersion::parse("0.1.0").unwrap(),
        },
        files: FILES,
        inspection_files: &[
            "package.json",
            "astro.config.mjs",
            "src/styles/tokens.css",
            "src/styles/global.css",
            "src/pages/index.astro",
            "src/components/ui/Button.astro",
        ],
        build: BuildSpec {
            argv: vec!["npm".to_string(), "run".to_string(), "build".to_string()],
            timeout_ms: 120_000,
            success_marker: "astro build completed",
        },
        preview: PreviewSpec {
            output_directories: vec!["dist".to_string()],
            port: 4321,
            command: "astro preview",
            screenshot_id: "shot-astro-home",
        },
        artifact_delivery: ArtifactDeliverySpec::HOST_ROOT,
        capabilities: TemplateCapabilities {
            structured_page_write: true,
            mdx_document_write: false,
            static_export: true,
        },
        mutation_policy: MutationPolicySpec::ALLOW_ALL,
        style: StyleContractSpec {
            version: "runtime-style-contract@p3",
            token_file: "src/styles/tokens.css",
            global_css_file: "src/styles/global.css",
            component_root: "src/components/ui",
            tailwind_version: "4",
            tailwind_entry_import: "@import \"tailwindcss\"",
            tokens: STYLE_TOKENS,
        },
        operations: &OPERATIONS,
    }
}

fn page_relative_path(route: &str) -> Result<PathBuf, TemplateOperationError> {
    let normalized = route.trim();
    if !normalized.starts_with('/') || normalized.contains('?') || normalized.contains('#') {
        return Err(TemplateOperationError {
            error_kind: "project.route_invalid",
            message: "route must begin with '/' and must not contain query or fragment".to_string(),
        });
    }
    if normalized == "/" {
        return Ok(PathBuf::from("index.astro"));
    }
    let mut path = PathBuf::new();
    for part in normalized.trim_matches('/').split('/') {
        if part.is_empty()
            || !part.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
        {
            return Err(TemplateOperationError {
                error_kind: "project.route_invalid",
                message: "route segments may only contain ASCII letters, numbers, '-' or '_'"
                    .to_string(),
            });
        }
        path.push(part);
    }
    path.set_extension("astro");
    Ok(path)
}

fn render_page(request: &RenderPageRequest, relative_page_path: &Path) -> String {
    let escaped_title = html_escape(&request.title);
    let global_css_import = global_css_import(relative_page_path);
    let rendered_sections = request
        .sections
        .iter()
        .enumerate()
        .map(|(index, section)| render_section(index, section))
        .collect::<Vec<_>>()
        .join("\n\n");
    let style_class = match request.style_profile.as_str() {
        "saas" | "enterprise" | "docs" => request.style_profile.as_str(),
        _ => "saas",
    };
    format!(
        r#"---
import '{global_css_import}';
const title = '{title_js}';
---
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>{{title}}</title>
  </head>
  <body class="runtime-page {style_class}">
    <main class="runtime-shell">
      <header class="runtime-hero">
        <div class="runtime-kicker">{route}</div>
        <h1>{escaped_title}</h1>
      </header>
      <div class="runtime-sections">
{rendered_sections}
      </div>
    </main>
  </body>
</html>
"#,
        title_js = js_string_escape(&request.title),
        route = html_escape(&request.route),
    )
}

fn global_css_import(relative_page_path: &Path) -> String {
    let parent_depth = relative_page_path
        .parent()
        .map(|parent| {
            parent
                .components()
                .filter(|component| matches!(component, Component::Normal(_)))
                .count()
        })
        .unwrap_or(0);
    format!("{}styles/global.css", "../".repeat(parent_depth + 1))
}

fn render_section(index: usize, section: &Value) -> String {
    let kind = section
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("section");
    let heading = section
        .get("heading")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Section {}", index + 1));
    let body = section.get("body").and_then(Value::as_str).unwrap_or("");
    let visual = section
        .get("visual")
        .and_then(Value::as_str)
        .unwrap_or(kind);
    format!(
        r#"        <section class="runtime-section" data-kind="{kind}">
          <div>
            <h2>{heading}</h2>
            <p>{body}</p>
          </div>
          <aside class="runtime-visual">{visual}</aside>
        </section>"#,
        kind = html_escape(kind),
        heading = html_escape(&heading),
        body = html_escape(body),
        visual = html_escape(visual),
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn js_string_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}
