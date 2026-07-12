# G9-D3 Internal Routes and Release Evidence Boundary

Date: 2026-07-12

Branch: `codex/runtime-architecture-g9d-internal-services`

Base main commit: `cc828cb`

## Boundary delivered

- The 410-line Internal route module is now a 16-line composition façade.
- Template Build, Preview Promotion, Project Access, Release Evidence, and Sandbox Release each have an independently reviewable route module.
- Existing feature-flag ordering is preserved: disabled Template Build and Preview Promotion endpoints still return not-found before service authorization is evaluated.
- `ReleaseEvidenceService` owns current/base version lookup, Artifact publish lookup, Preview lease and Sandbox binding lookup, event ordering, failure counting, and screenshot evidence reads.
- Release Evidence HTTP routing now performs only Internal Admin authorization, service invocation, and typed error mapping.
- Screenshot JSON remains behind `RuntimeEvidenceStore`; the application service and route use no direct filesystem API.
- The executable route-manifest verifier now recursively discovers nested route source modules. The frozen route set did not change.

## Enforced architecture gates

`check-http-api-architecture.sh` now rejects:

- handlers or Store orchestration in the Internal composition façade;
- Store or RuntimeEvidenceStore orchestration in the Release Evidence route;
- any missing Internal use-case route module;
- Axum, HeaderMap, status, router, or direct filesystem dependencies in `ReleaseEvidenceService`.

Current size evidence:

```text
http_api/mod.rs                                      279
http_api/routes/internal.rs                           16
http_api/routes/internal/template_build.rs           124
http_api/routes/internal/preview_promotion.rs         70
http_api/routes/internal/project_access.rs            61
http_api/routes/internal/release_evidence.rs           34
http_api/routes/internal/sandbox_release.rs            46
release_evidence.rs                                  180
```

## Verification

```text
ReleaseEvidence fail-closed unit test                  passed
Internal HTTP focused suite                            9 passed
recursive executable route-manifest verification       passed
cargo test --all-targets                               passed
  core library                                       107 passed
  HTTP integration                                    85 passed, 1 external-provider test ignored
  Astro/Fumadocs integration build                      5 passed
cargo fmt --check                                      passed
cargo clippy --all-targets --all-features -D warnings  passed
strict Sandbox architecture gate                       passed
remote workspace filesystem boundary gate              passed
Artifact manifest architecture gate                    passed
Runtime bootstrap architecture gate                    passed
Release packaging architecture gate                    passed
Publication control-plane architecture gate            passed
HTTP API architecture gate                             passed
git diff --check                                       passed
```

## G9 status after this increment

G9-D implementation is complete. G9 remains in progress until the consolidated Runtime/Published gate audit is recorded and the master plan completion state is updated from merged-main evidence.
