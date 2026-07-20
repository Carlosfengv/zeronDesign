use super::{
    BuildSpec, ComponentRegistryRef, DependencyPolicyRef, DevelopmentServerSpec, FrameworkId,
    ManifestHash, MutationPolicySpec, PreviewSpec, RenderPageRequest, RenderedFile,
    SandboxExecutionProfileId, SandboxExecutionProfileRef, SandboxExecutionProfileVersion,
    SourceContractReport, SourceContractSpec, SourceSnapshot, StyleContractSpec, StyleTokenSpec,
    TemplateCapabilities, TemplateFile, TemplateFileRole, TemplateId, TemplateOperationError,
    TemplateOperations, TemplateSpec, TemplateVersion, TemplateWriteMode, ValidationContractSpec,
};
use crate::artifact_manifest::ArtifactDeliverySpec;
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
    asset!("next.config.mjs", TemplateFileRole::FrameworkConfig),
    asset!("tsconfig.json", TemplateFileRole::FrameworkConfig),
    asset!("next-env.d.ts", TemplateFileRole::FrameworkConfig),
    asset!("postcss.config.mjs", TemplateFileRole::FrameworkConfig),
    asset!("components.json", TemplateFileRole::FrameworkConfig),
    asset!("app/layout.tsx", TemplateFileRole::Source),
    asset!("app/page.tsx", TemplateFileRole::Source),
    asset!("app/globals.css", TemplateFileRole::Style),
    asset!("app/tokens.css", TemplateFileRole::Style),
    asset!("components/ui/button.tsx", TemplateFileRole::Source),
    asset!("components/ui/card.tsx", TemplateFileRole::Source),
    asset!("components/ui/input.tsx", TemplateFileRole::Source),
    asset!("components/ui/label.tsx", TemplateFileRole::Source),
    asset!("components/ui/textarea.tsx", TemplateFileRole::Source),
    asset!("components/ui/tabs.tsx", TemplateFileRole::Source),
    asset!("components/ui/dialog.tsx", TemplateFileRole::Source),
    asset!("components/ui/dropdown-menu.tsx", TemplateFileRole::Source),
    asset!("components/ui/select.tsx", TemplateFileRole::Source),
    asset!("components/ui/tooltip.tsx", TemplateFileRole::Source),
    asset!("components/ui/skeleton.tsx", TemplateFileRole::Source),
    asset!("components/ui/separator.tsx", TemplateFileRole::Source),
    asset!("lib/utils.ts", TemplateFileRole::Source),
    asset!("public/assets/.gitkeep", TemplateFileRole::Content),
];

pub static STYLE_TOKENS: &[StyleTokenSpec] = &[
    StyleTokenSpec {
        name: "color.background",
        css_variable: "--background",
    },
    StyleTokenSpec {
        name: "color.surface",
        css_variable: "--card",
    },
    StyleTokenSpec {
        name: "color.surfaceStrong",
        css_variable: "--secondary",
    },
    StyleTokenSpec {
        name: "color.text",
        css_variable: "--foreground",
    },
    StyleTokenSpec {
        name: "color.muted",
        css_variable: "--muted-foreground",
    },
    StyleTokenSpec {
        name: "color.primary",
        css_variable: "--primary",
    },
    StyleTokenSpec {
        name: "color.primaryContrast",
        css_variable: "--primary-foreground",
    },
    StyleTokenSpec {
        name: "color.action",
        css_variable: "--primary",
    },
    StyleTokenSpec {
        name: "color.actionContrast",
        css_variable: "--primary-foreground",
    },
    StyleTokenSpec {
        name: "color.authSubmit",
        css_variable: "--primary",
    },
    StyleTokenSpec {
        name: "color.border",
        css_variable: "--border",
    },
    StyleTokenSpec {
        name: "radius.card",
        css_variable: "--radius",
    },
    StyleTokenSpec {
        name: "radius.control",
        css_variable: "--radius",
    },
    StyleTokenSpec {
        name: "radius.input",
        css_variable: "--radius",
    },
    StyleTokenSpec {
        name: "radius.badge",
        css_variable: "--radius",
    },
    StyleTokenSpec {
        name: "radius.largeCard",
        css_variable: "--radius",
    },
    StyleTokenSpec {
        name: "font.sans",
        css_variable: "--font-sans",
    },
    StyleTokenSpec {
        name: "font.display",
        css_variable: "--font-display",
    },
    StyleTokenSpec {
        name: "font.mono",
        css_variable: "--font-mono",
    },
    StyleTokenSpec {
        name: "shadow.soft",
        css_variable: "--shadow-soft",
    },
    StyleTokenSpec {
        name: "spacing.pageGutter",
        css_variable: "--spacing-page-gutter",
    },
    StyleTokenSpec {
        name: "spacing.section",
        css_variable: "--spacing-section",
    },
];

const SOURCE_PATHS: &[&str] = &[
    "package.json",
    "package-lock.json",
    "next.config.mjs",
    "components.json",
    "postcss.config.mjs",
    "tsconfig.json",
    "app/layout.tsx",
    "app/page.tsx",
    "app/globals.css",
    "app/tokens.css",
];
const SOURCE_ROOTS: &[&str] = &["pages", "src/pages", "app/api"];

struct NextAppOperations;
static OPERATIONS: NextAppOperations = NextAppOperations;

impl TemplateOperations for NextAppOperations {
    fn name(&self) -> &'static str {
        "next-app"
    }

    fn supports_render_page(&self) -> bool {
        true
    }

    fn render_page(
        &self,
        request: &RenderPageRequest,
    ) -> Result<Vec<RenderedFile>, TemplateOperationError> {
        let page_path = page_relative_path(&request.route)?;
        let title =
            serde_json::to_string(&request.title).map_err(|error| TemplateOperationError {
                error_kind: "project.route_invalid",
                message: error.to_string(),
            })?;
        Ok(vec![RenderedFile {
            path: page_path.to_string_lossy().replace('\\', "/"),
            content: format!(
                "export default function Page() {{\n  return (\n    <main className=\"mx-auto min-h-svh max-w-6xl px-6 py-20\">\n      <h1 className=\"text-5xl font-semibold tracking-tight\">{{{title}}}</h1>\n    </main>\n  )\n}}\n"
            ),
        }])
    }

    fn source_contract_paths(&self) -> &'static [&'static str] {
        SOURCE_PATHS
    }

    fn source_contract_roots(&self) -> &'static [&'static str] {
        SOURCE_ROOTS
    }

    fn validate_source(&self, snapshot: &SourceSnapshot) -> SourceContractReport {
        let mut violations = Vec::new();
        require_contains(
            snapshot,
            "next.config.mjs",
            &["output: \"export\"", "unoptimized: true"],
            "next.config.mjs must preserve static export and unoptimized images",
            &mut violations,
        );
        require_contains(
            snapshot,
            "package.json",
            &[
                "\"next\"",
                "\"react\"",
                "\"@base-ui/react\"",
                "\"build\": \"next build\"",
            ],
            "package.json must preserve the pinned Next/React/Base UI build contract",
            &mut violations,
        );
        require_contains(
            snapshot,
            "components.json",
            &["\"style\": \"base-nova\"", "\"css\": \"app/globals.css\""],
            "components.json must remain on the frozen shadcn Base UI contract",
            &mut violations,
        );
        require_contains(
            snapshot,
            "app/globals.css",
            &[
                "@import \"tailwindcss\"",
                "@theme inline",
                "@import \"./tokens.css\"",
            ],
            "app/globals.css must preserve Tailwind v4 and token imports",
            &mut violations,
        );
        for root in SOURCE_ROOTS {
            if snapshot.has_root(root) {
                violations.push(format!("forbidden next-app source root present: {root}"));
            }
        }
        SourceContractReport {
            violations,
            error_kind: "template.static_export_forbidden",
            summary: "next-app static export contract invalid",
            guidance: "Keep routes under app/**/page.tsx, preserve output: export, and remove server-only routes or configuration.",
        }
    }
}

pub fn spec() -> TemplateSpec {
    TemplateSpec {
        id: TemplateId::parse("next-app").unwrap(),
        version: TemplateVersion::parse("next-app@1").unwrap(),
        manifest_sha256: ManifestHash::parse(
            "919771231a9745aee050a3280518189d4b8d9f106d6ba334a896f41eac253067",
        )
        .unwrap(),
        framework: FrameworkId::parse("nextjs").unwrap(),
        surface: "website",
        default_title: "AnyDesign React Project",
        sandbox_execution_profile: SandboxExecutionProfileRef {
            id: SandboxExecutionProfileId::parse("next-app").unwrap(),
            version: SandboxExecutionProfileVersion::parse("0.1.0").unwrap(),
        },
        files: FILES,
        inspection_files: SOURCE_PATHS,
        build: BuildSpec {
            argv: vec!["npm".to_string(), "run".to_string(), "build".to_string()],
            timeout_ms: 180_000,
            success_marker: "next-app production build completed",
        },
        development_server: Some(DevelopmentServerSpec {
            argv: vec!["npm".to_string(), "run".to_string(), "dev".to_string()],
            port: 3000,
            readiness_path: "/",
            hmr: true,
        }),
        preview: PreviewSpec {
            output_directories: vec!["out".to_string()],
            port: 3000,
            command: "serve out --listen 3000",
            screenshot_id: "shot-next-app-home",
        },
        artifact_delivery: ArtifactDeliverySpec::HOST_ROOT,
        source_contract: SourceContractSpec {
            version: "next-app-source-contract@1",
            protected_paths: &["package.json", "package-lock.json", "next.config.mjs", "components.json"],
            forbidden_roots: &["pages", "src/pages", "app/api"],
            forbidden_source_patterns: &["\"use server\"", "'use server'"],
            forbidden_import_prefixes: &["server-only", "next/headers", "next/server"],
        },
        capabilities: TemplateCapabilities {
            structured_page_write: true,
            mdx_document_write: false,
            static_export: true,
            supported_component_roles: &["navigation", "action", "content", "input", "overlay", "feedback"],
            supported_craft_packs: &["accessibility-baseline", "responsive-layout", "form-states", "anti-generic-ui"],
        },
        mutation_policy: MutationPolicySpec {
            forbidden_write_roots: &["pages", "src/pages", "app/api"],
            protected_write_paths: &["package.json", "package-lock.json", "next.config.mjs", "components.json"],
            error_kind: "template.protected_contract_mutation",
            guidance: "Edit React source under app/ or components/. Use project.ensure_dependencies for cataloged dependencies; static export and shadcn contract files are Runtime-owned.",
        },
        dependency_policy: DependencyPolicyRef {
            version: "runtime-dependency-policy@1",
            catalogs: &["template-core", "visual-catalog"],
        },
        component_registry: Some(ComponentRegistryRef {
            version: "internal-shadcn-registry@1",
            protocol: "shadcn-registry@v1",
        }),
        style: StyleContractSpec {
            version: "runtime-style-contract@p3",
            token_file: "app/tokens.css",
            global_css_file: "app/globals.css",
            component_root: "components/ui",
            tailwind_version: "4",
            tailwind_entry_import: "@import \"tailwindcss\"",
            tokens: STYLE_TOKENS,
        },
        validation_contract: ValidationContractSpec {
            version: "next-app-validation@1",
            static_export_required: true,
        },
        operations: &OPERATIONS,
    }
}

fn page_relative_path(route: &str) -> Result<PathBuf, TemplateOperationError> {
    let route = route.trim();
    if !route.starts_with('/') || route.contains('?') || route.contains('#') {
        return Err(route_error(
            "route must start with '/' without a query or fragment",
        ));
    }
    let mut path = PathBuf::from("app");
    if route != "/" {
        let relative = Path::new(route.trim_matches('/'));
        for component in relative.components() {
            match component {
                Component::Normal(segment) => {
                    let segment = segment.to_string_lossy();
                    if segment.is_empty()
                        || segment.starts_with('.')
                        || segment == "api"
                        || segment.contains('[')
                        || segment.contains(']')
                    {
                        return Err(route_error(
                            "route contains an unsafe or unsupported segment",
                        ));
                    }
                    path.push(segment.as_ref());
                }
                _ => return Err(route_error("route contains an unsafe path component")),
            }
        }
    }
    path.push("page.tsx");
    Ok(path)
}

fn route_error(message: &str) -> TemplateOperationError {
    TemplateOperationError {
        error_kind: "project.route_invalid",
        message: message.to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_operation_maps_safe_routes_to_app_router_pages() {
        assert_eq!(
            page_relative_path("/").unwrap(),
            PathBuf::from("app/page.tsx")
        );
        assert_eq!(
            page_relative_path("/about/team").unwrap(),
            PathBuf::from("app/about/team/page.tsx")
        );
        assert!(page_relative_path("/api/users").is_err());
        assert!(page_relative_path("/../secret").is_err());
    }
}
