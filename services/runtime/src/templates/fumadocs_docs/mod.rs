mod build_overlay;

use super::{
    BuildOverlayRequest, BuildSpec, FrameworkId, ManifestHash, MutationPolicySpec, PreviewSpec,
    RenderDocumentRequest, RenderedFile, SandboxExecutionProfileId, SandboxExecutionProfileRef,
    SandboxExecutionProfileVersion, SourceContractReport, SourceSnapshot, StyleContractSpec,
    StyleTokenSpec, TemplateCapabilities, TemplateFile, TemplateFileRole, TemplateId,
    TemplateOperationError, TemplateOperations, TemplateSpec, TemplateVersion, TemplateWriteMode,
};
use crate::artifact_manifest::ArtifactDeliverySpec;
use serde_json::Value;

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
    asset!("postcss.config.mjs", TemplateFileRole::FrameworkConfig),
    asset!("next.config.mjs", TemplateFileRole::FrameworkConfig),
    asset!("source.config.ts", TemplateFileRole::FrameworkConfig),
    asset!("tsconfig.json", TemplateFileRole::FrameworkConfig),
    asset!("next-env.d.ts", TemplateFileRole::FrameworkConfig),
    asset!("lib/source.js", TemplateFileRole::Source),
    asset!("lib/layout.shared.jsx", TemplateFileRole::Source),
    asset!("components/mdx.jsx", TemplateFileRole::Source),
    asset!("components/ui/button.jsx", TemplateFileRole::Source),
    asset!("mdx-components.jsx", TemplateFileRole::Source),
    asset!("app/tokens.css", TemplateFileRole::Style),
    asset!("app/global.css", TemplateFileRole::Style),
    asset!("app/layout.jsx", TemplateFileRole::Source),
    asset!("app/page.jsx", TemplateFileRole::Source),
    asset!("app/docs/layout.jsx", TemplateFileRole::Source),
    asset!("app/docs/[[...slug]]/page.jsx", TemplateFileRole::Source),
    asset!("content/docs/index.mdx", TemplateFileRole::Content),
    asset!("content/docs/runtime-flow.mdx", TemplateFileRole::Content),
    TemplateFile {
        path: "content/docs/meta.json",
        content: include_str!("files/content/docs/meta.json"),
        trim_final_newline: true,
        role: TemplateFileRole::Content,
        write_mode: TemplateWriteMode::ReplaceOnInit,
    },
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
        name: "radius.input",
        css_variable: "--runtime-radius-input",
    },
    StyleTokenSpec {
        name: "radius.badge",
        css_variable: "--runtime-radius-badge",
    },
    StyleTokenSpec {
        name: "gradient.display",
        css_variable: "--runtime-gradient-display",
    },
];

struct FumadocsDocsOperations;

static OPERATIONS: FumadocsDocsOperations = FumadocsDocsOperations;

const SOURCE_PATHS: &[&str] = &[
    "source.config.ts",
    "lib/source.js",
    "lib/source.ts",
    "app/docs/layout.jsx",
    "app/docs/[[...slug]]/page.jsx",
    "app/page.jsx",
    "content/docs/index.mdx",
    "content/docs/meta.json",
];

const SOURCE_ROOTS: &[&str] = &["pages", "src/pages"];

impl TemplateOperations for FumadocsDocsOperations {
    fn name(&self) -> &'static str {
        "fumadocs-docs"
    }

    fn supports_render_document(&self) -> bool {
        true
    }

    fn render_document(
        &self,
        request: &RenderDocumentRequest,
    ) -> Result<Vec<RenderedFile>, TemplateOperationError> {
        let slug = request.slug.trim_matches('/');
        if slug.is_empty()
            || slug.split('/').any(|segment| {
                segment.is_empty()
                    || !segment.chars().all(|character| {
                        character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
                    })
            })
        {
            return Err(TemplateOperationError {
                error_kind: "project.document_slug_invalid",
                message: "document slug must contain safe path segments".to_string(),
            });
        }
        Ok(vec![RenderedFile {
            path: format!("content/docs/{slug}.mdx"),
            content: format!(
                "---\ntitle: {}\ndescription: {}\n---\n\n{}\n",
                yaml_scalar(&request.title),
                yaml_scalar(&request.description),
                request.body
            ),
        }])
    }

    fn render_build_overlay(
        &self,
        request: &BuildOverlayRequest,
    ) -> Result<Vec<RenderedFile>, TemplateOperationError> {
        Ok(build_overlay::render(request))
    }

    fn validate_build_overlay(&self, request: &BuildOverlayRequest) -> Vec<&'static str> {
        build_overlay::validate(request)
    }

    fn source_contract_paths(&self) -> &'static [&'static str] {
        SOURCE_PATHS
    }

    fn source_contract_roots(&self) -> &'static [&'static str] {
        SOURCE_ROOTS
    }

    fn validate_source(&self, snapshot: &SourceSnapshot) -> SourceContractReport {
        let mut violations = Vec::new();
        if snapshot.has_root("pages") || snapshot.has_root("src/pages") {
            violations.push(
                "project/pages and project/src/pages are forbidden for fumadocs-docs; keep routes under app/"
                    .to_string(),
            );
        }
        require_contains(
            snapshot,
            "source.config.ts",
            &["defineDocs", "content/docs"],
            "source.config.ts must define docs dir content/docs",
            &mut violations,
        );
        let source_loader = snapshot
            .file("lib/source.js")
            .or_else(|| snapshot.file("lib/source.ts"));
        match source_loader {
            Some(text)
                if text.contains("baseUrl: '/docs'") && text.contains("toFumadocsSource()") => {}
            Some(_) => {
                violations.push("lib/source.js must load Fumadocs source at /docs".to_string())
            }
            None => violations.push("missing lib/source.js or lib/source.ts".to_string()),
        }
        require_contains(
            snapshot,
            "app/docs/layout.jsx",
            &["DocsLayout", "source.pageTree"],
            "app/docs/layout.jsx must render DocsLayout with source.pageTree",
            &mut violations,
        );
        require_contains(
            snapshot,
            "app/docs/[[...slug]]/page.jsx",
            &["generateStaticParams", "source.getPage"],
            "app/docs/[[...slug]]/page.jsx must map slugs through source.getPage",
            &mut violations,
        );
        require_contains(
            snapshot,
            "app/page.jsx",
            &["href=\"/docs\""],
            "app/page.jsx must link the home route to /docs",
            &mut violations,
        );
        match snapshot.file("content/docs/index.mdx") {
            Some(text) if text.trim_start().starts_with("---") && text.contains("\ntitle:") => {}
            Some(_) => {
                violations.push("content/docs/index.mdx must include frontmatter title".to_string())
            }
            None => violations.push("missing content/docs/index.mdx".to_string()),
        }
        match snapshot.file("content/docs/meta.json") {
            Some(text) => match serde_json::from_str::<Value>(text) {
                Ok(value)
                    if value
                        .get("pages")
                        .and_then(Value::as_array)
                        .is_some_and(|pages| {
                            !pages.is_empty()
                                && pages.iter().any(|page| page.as_str() == Some("index"))
                        }) => {}
                Ok(_) => {
                    violations.push("content/docs/meta.json must list index in pages".to_string())
                }
                Err(_) => violations.push("content/docs/meta.json must be valid JSON".to_string()),
            },
            None => violations.push("missing content/docs/meta.json".to_string()),
        }
        SourceContractReport {
            error_kind: if violations.iter().any(|violation| violation.contains("forbidden")) {
                "docs.routing_root_forbidden"
            } else {
                "docs.source_contract_invalid"
            },
            summary: "Docs source contract invalid",
            violations,
            guidance: "Repair the fumadocs-docs app-router scaffold: keep routes under app/, docs content under content/docs, and do not create project/pages or project/src/pages.",
        }
    }
}

pub fn spec() -> TemplateSpec {
    TemplateSpec {
        id: TemplateId::parse("fumadocs-docs").unwrap(),
        version: TemplateVersion::parse("fumadocs-docs@runtime-p3").unwrap(),
        manifest_sha256: ManifestHash::parse(
            "753ce62ea481258e9620bafe2d5e53e31da2db7c037945f6266490cc0d1336e4",
        )
        .unwrap(),
        framework: FrameworkId::parse("fumadocs").unwrap(),
        surface: "docs",
        default_title: "AnyDesign Runtime Docs",
        sandbox_execution_profile: SandboxExecutionProfileRef {
            id: SandboxExecutionProfileId::parse("fumadocs-docs").unwrap(),
            version: SandboxExecutionProfileVersion::parse("0.1.0").unwrap(),
        },
        files: FILES,
        inspection_files: &[
            "package.json",
            "next.config.mjs",
            "source.config.ts",
            "app/global.css",
            "app/tokens.css",
            "app/docs/layout.jsx",
            "app/docs/[[...slug]]/page.jsx",
            "content/docs/index.mdx",
            "content/docs/meta.json",
        ],
        build: BuildSpec {
            argv: vec!["npm".to_string(), "run".to_string(), "build".to_string()],
            timeout_ms: 180_000,
            success_marker: "fumadocs docs build completed",
        },
        preview: PreviewSpec {
            output_directories: vec!["out".to_string()],
            port: 3000,
            command: "serve out --listen 3000",
            screenshot_id: "shot-fumadocs-home",
        },
        artifact_delivery: ArtifactDeliverySpec::HOST_ROOT,
        capabilities: TemplateCapabilities {
            structured_page_write: false,
            mdx_document_write: true,
            static_export: true,
            supported_component_roles: &["navigation", "content", "action"],
            supported_craft_packs: &["accessibility-baseline", "responsive-layout"],
        },
        mutation_policy: MutationPolicySpec {
            forbidden_write_roots: &["pages", "src/pages"],
            error_kind: "docs.routing_root_forbidden",
            guidance: "Keep fumadocs-docs projects on the Next app router. Write docs routes under app/docs/[[...slug]] and MDX content under content/docs; do not create pages or src/pages.",
        },
        style: StyleContractSpec {
            version: "runtime-style-contract@p3",
            token_file: "app/tokens.css",
            global_css_file: "app/global.css",
            component_root: "components/ui",
            tailwind_version: "4",
            tailwind_entry_import: "@import \"tailwindcss\"",
            tokens: STYLE_TOKENS,
        },
        operations: &OPERATIONS,
    }
}

fn require_contains(
    snapshot: &SourceSnapshot,
    path: &str,
    needles: &[&str],
    violation: &str,
    violations: &mut Vec<String>,
) {
    match snapshot.file(path) {
        Some(text) if needles.iter().all(|needle| text.contains(needle)) => {}
        Some(_) => violations.push(violation.to_string()),
        None => violations.push(format!("missing {path}")),
    }
}

fn yaml_scalar(value: &str) -> String {
    format!("{:?}", value)
}
