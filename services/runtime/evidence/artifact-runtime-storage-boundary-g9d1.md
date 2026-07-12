# G9-D1 Artifact and Runtime Evidence Storage Boundary

Date: 2026-07-12

Branch: `codex/runtime-architecture-g9d-access-artifacts`

Base main commit: `2431e63`

## Boundary delivered

- `ArtifactAccessService` now owns current-version and publish-manifest lookup without depending on Axum or filesystem APIs.
- `ArtifactStore` and `FileArtifactStore` own immutable version-root selection, verified manifest resolution, legacy fallback, content metadata, and path confinement.
- `RuntimeEvidenceStore` and `FileRuntimeEvidenceStore` own Runtime screenshot evidence reads and sanitize every identity segment before building a path.
- Artifact HTTP routes now delegate to `ArtifactAccessService`; they no longer orchestrate `RuntimeStore`, `ArtifactResolver`, `FileArtifactPublisher`, or filesystem reads.
- Artifact content headers and historical HTML URL rewriting moved into a dedicated HTTP presenter.
- Internal release-evidence routing no longer calls `std::fs`; screenshot JSON is read through the injected Runtime evidence port.
- The existing global `/_next/*` compatibility route remains unchanged. No new framework-specific global asset route was added.

## Fail-closed behavior retained

- Manifest identity, hash, file hash, byte length, and public mount validation still use the existing verified `ArtifactResolver`.
- A promoted version with an expected manifest hash but no manifest returns conflict and cannot fall back to legacy bytes.
- Legacy artifact paths remain confined to the immutable project/version root.
- Missing current versions, immutable output roots, and artifact files retain not-found behavior.
- Runtime evidence identities cannot escape the configured screenshot root.

## Enforced architecture gates

The architecture checks now reject:

- direct filesystem APIs in HTTP route modules;
- filesystem or HTTP dependencies in `ArtifactAccessService`;
- Store, resolver, publisher, or filesystem orchestration in the Artifact route;
- a verified manifest resolver outside the file-adapter boundary;
- historical framework URL rewriting outside the Artifact HTTP presenter.

Current size evidence:

```text
runtime_storage/artifact.rs                         238
runtime_storage/evidence.rs                          74
artifact_access.rs                                   45
http_api/artifact_presenter.rs                       58
http_api/routes/artifacts.rs                         66
```

## Verification

```text
Runtime storage unit tests                           3 passed
HTTP Artifact focused suite                          2 passed
HTTP Internal focused suite                          9 passed
cargo test --all-targets                             passed
  core library                                     105 passed
  HTTP integration                                  85 passed, 1 external-provider test ignored
  Astro/Fumadocs integration build                    5 passed
cargo fmt --check                                    passed
cargo clippy --all-targets --all-features -D warnings passed
strict Sandbox architecture gate                     passed
remote workspace filesystem boundary gate            passed
Artifact manifest architecture gate                  passed
Runtime bootstrap architecture gate                  passed
Release packaging architecture gate                  passed
Publication control-plane architecture gate          passed
HTTP API architecture gate                           passed
git diff --check                                     passed
```

## G9 status after this increment

G9-D1 is complete. G9 remains in progress: Preview access and application authorization policy, Internal route-family extraction, consolidated Runtime/Published gates, and the final completion audit are still required.
