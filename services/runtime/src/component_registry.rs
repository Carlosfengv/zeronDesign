use crate::types::sha256_hex;
use serde::Serialize;

pub const COMPONENT_REGISTRY_VERSION: &str = "next-app-shadcn-base-ui@1";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentRegistryFile {
    pub path: &'static str,
    pub content: &'static str,
    pub kind: &'static str,
    pub target: &'static str,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentRegistryItem {
    #[serde(rename = "$schema")]
    pub schema: &'static str,
    pub name: &'static str,
    #[serde(rename = "type")]
    pub item_type: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub registry_version: &'static str,
    pub source: &'static str,
    pub license: &'static str,
    pub dependencies: Vec<&'static str>,
    pub registry_dependencies: Vec<&'static str>,
    pub compatible_templates: Vec<&'static str>,
    pub security_scan: &'static str,
    pub content_hash: String,
    pub files: Vec<ComponentRegistryFile>,
}

pub fn search(query: &str) -> Vec<ComponentRegistryItem> {
    let query = query.trim().to_ascii_lowercase();
    items()
        .into_iter()
        .filter(|item| {
            query.is_empty()
                || item.name.contains(&query)
                || item.title.to_ascii_lowercase().contains(&query)
                || item.description.to_ascii_lowercase().contains(&query)
        })
        .collect()
}

pub fn get(name: &str) -> Option<ComponentRegistryItem> {
    items().into_iter().find(|item| item.name == name)
}

fn items() -> Vec<ComponentRegistryItem> {
    vec![
        item(
            "badge",
            "Badge",
            "Compact status and category label with semantic variants.",
            "components/ui/badge.tsx",
            BADGE_SOURCE,
        ),
        item(
            "avatar",
            "Avatar",
            "Accessible image avatar with deterministic fallback content.",
            "components/ui/avatar.tsx",
            AVATAR_SOURCE,
        ),
        item(
            "progress",
            "Progress",
            "Accessible determinate progress indicator using project tokens.",
            "components/ui/progress.tsx",
            PROGRESS_SOURCE,
        ),
    ]
}

fn item(
    name: &'static str,
    title: &'static str,
    description: &'static str,
    path: &'static str,
    content: &'static str,
) -> ComponentRegistryItem {
    let file_hash = sha256_hex(content.as_bytes());
    let content_hash = sha256_hex(
        serde_json::to_vec(&serde_json::json!({
            "name": name,
            "path": path,
            "sha256": file_hash,
            "registryVersion": COMPONENT_REGISTRY_VERSION,
        }))
        .expect("registry identity serializes")
        .as_slice(),
    );
    ComponentRegistryItem {
        schema: "https://ui.shadcn.com/schema/registry-item.json",
        name,
        item_type: "registry:ui",
        title,
        description,
        registry_version: COMPONENT_REGISTRY_VERSION,
        source: "runtime://component-registry/next-app/base-ui",
        license: "MIT",
        dependencies: Vec::new(),
        registry_dependencies: Vec::new(),
        compatible_templates: vec!["next-app@1", "next-app@2"],
        security_scan: "runtime-static-source-reviewed",
        content_hash,
        files: vec![ComponentRegistryFile {
            path,
            content,
            kind: "registry:file",
            target: path,
            sha256: file_hash,
        }],
    }
}

const BADGE_SOURCE: &str = r#"import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"

import { cn } from "@/lib/utils"

const badgeVariants = cva(
  "inline-flex items-center rounded-full border px-2.5 py-0.5 text-xs font-semibold transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
  {
    variants: {
      variant: {
        default: "border-transparent bg-primary text-primary-foreground",
        secondary: "border-transparent bg-secondary text-secondary-foreground",
        outline: "text-foreground",
      },
    },
    defaultVariants: { variant: "default" },
  },
)

function Badge({ className, variant, ...props }: React.ComponentProps<"div"> & VariantProps<typeof badgeVariants>) {
  return <div data-slot="badge" className={cn(badgeVariants({ variant }), className)} {...props} />
}

export { Badge, badgeVariants }
"#;

const AVATAR_SOURCE: &str = r#"import * as React from "react"

import { cn } from "@/lib/utils"

function Avatar({ className, ...props }: React.ComponentProps<"span">) {
  return <span data-slot="avatar" className={cn("relative flex size-10 shrink-0 overflow-hidden rounded-full bg-muted", className)} {...props} />
}

function AvatarImage({ className, alt, ...props }: React.ComponentProps<"img">) {
  return <img data-slot="avatar-image" alt={alt} className={cn("aspect-square size-full object-cover", className)} {...props} />
}

function AvatarFallback({ className, ...props }: React.ComponentProps<"span">) {
  return <span data-slot="avatar-fallback" className={cn("flex size-full items-center justify-center rounded-full bg-muted text-sm text-muted-foreground", className)} {...props} />
}

export { Avatar, AvatarImage, AvatarFallback }
"#;

const PROGRESS_SOURCE: &str = r#"import * as React from "react"

import { cn } from "@/lib/utils"

function Progress({ value = 0, className, ...props }: Omit<React.ComponentProps<"div">, "children"> & { value?: number }) {
  const bounded = Math.min(100, Math.max(0, value))
  return (
    <div data-slot="progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={bounded} className={cn("relative h-2 w-full overflow-hidden rounded-full bg-secondary", className)} {...props}>
      <div className="h-full bg-primary transition-transform" style={{ transform: `translateX(-${100 - bounded}%)` }} />
    </div>
  )
}

export { Progress }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_items_are_hash_frozen_shadcn_records() {
        let badge = get("badge").unwrap();
        assert_eq!(badge.item_type, "registry:ui");
        assert_eq!(badge.compatible_templates, ["next-app@1", "next-app@2"]);
        assert_eq!(badge.content_hash.len(), 64);
        assert_eq!(badge.files[0].sha256, sha256_hex(BADGE_SOURCE.as_bytes()));
        assert!(search("status").iter().any(|item| item.name == "badge"));
    }
}
