# Generic Artifact Manifest G3 Evidence

## Identity

| Field | Value |
|---|---|
| Base commit | `74fe6e5` |
| Branch | `codex/runtime-architecture-g3` |
| Goal | G3 generic host-root artifact delivery |
| Contract | `services/runtime/contracts/artifact-manifest-v1.schema.json` |
| Evidence date | 2026-07-12 |

## Structural Result

| Requirement | Result |
|---|---|
| Typed schema | Rust types and JSON Schema freeze `artifact-manifest@1` |
| Canonical identity | sorted typed manifest is canonically serialized and SHA-256 hashed |
| Actual bytes | every file records and verifies actual size and SHA-256 |
| Content type | extension-derived allowlist is frozen in code and schema |
| Reserved paths | `.anydesign/*` and `.well-known/anydesign/*` fail closed |
| Delivery | `TemplateSpec` declares framework-neutral host-root mounts |
| Resolver | manifest identity, stored manifest hash, path, size, and file hash are verified |
| Built-in templates | Astro and Fumadocs use the same host-root delivery capability |
| Third template | synthetic template publishes and resolves without core dispatch changes |
| Legacy compatibility | historical framework rewrites are isolated in `artifact_legacy.rs` |
| New framework routing | CI rejects framework names and new framework paths in generic layers |

Fumadocs no longer writes the authoring `/artifacts/{project}/current` prefix into
its production export. New artifacts are therefore host-root portable for a dedicated
Service and Ingress. Existing pre-manifest artifacts retain read-only HTTP compatibility.

## Verification

| Gate | Result |
|---|---|
| `cargo fmt --check` | passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | passed |
| `cargo test --all-targets` | passed; only explicitly environment-gated tests ignored |
| `cargo test --test http_api` | `84 passed; 0 failed; 1 environment-gated ignored` |
| `cargo test --test template_registry` | `11 passed; 0 failed` |
| manifest integrity/resolver tests | `3 passed; 0 failed` |
| Generic artifact architecture | passed |
| HTTP architecture boundary | passed |
| Strict Sandbox architecture | passed |
| Remote workspace FS boundary | passed |
| Real Fumadocs production build | passed |

## Preserved Invariants

- Existing HTTP artifact paths and route manifest remain unchanged.
- Historical `_next`, `_astro`, and docs HTML rewrites are read-only compatibility code.
- New manifest-backed HTML bytes are returned unchanged and are not framework-rewritten.
- Artifact lookup remains project/version scoped and now additionally checks manifest identity.
- New template integration changes TemplateSpec and template code only, not resolver dispatch.
- The user-owned untracked architecture/product document was not staged.
