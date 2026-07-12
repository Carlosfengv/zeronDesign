# G9-D2 Preview Access and Authorization Boundary

Date: 2026-07-12

Branch: `codex/runtime-architecture-g9d-preview-auth`

Base main commit: `16479a3`

## Boundary delivered

- `AuthenticatedPrincipal` is now the credential-free application identity passed beyond HTTP authentication adapters.
- `ApplicationAuthorizationPolicy` owns project-scope and owner-principal authorization for both Candidate Preview and Publication.
- `PreviewAccessService` owns path validation, active lease lookup, lease/run/project identity, project-owner authorization, Sandbox binding identity, Pod UID validation, audit decisions, and channel endpoint resolution.
- Candidate Preview HTTP auth only loads/verifies bearer credentials and maps token errors. It no longer queries project access or decides ownership.
- Candidate Preview upstream request, manifest-evidence verification, safe response headers, private cache policy, and HTML prefix rewriting are isolated in a proxy adapter.
- Public Preview and Internal Capture share the same lease, binding, Pod identity, endpoint, and manifest checks; Internal Capture does not accidentally require a public bearer credential.
- Publication authentication now delegates the same project-owner rule to the application policy, removing a second ownership implementation.
- HTTP service construction moved into `http_api/composition.rs`, keeping the façade below its frozen limit.

## Fail-closed matrix

- missing bearer principal in required mode: `401`;
- invalid operation scope: `403`;
- cross-project principal: `403`;
- non-owner principal: `403`;
- missing or stopped/expired lease: `404`;
- lease/run project drift: `409`;
- Sandbox name or Pod UID drift: `409`;
- missing upstream manifest evidence: `404`;
- candidate manifest mismatch: `409`;
- invalid preview path or public prefix: `404` / `400`;
- valid owner and valid Internal Capture: `200`.

The Candidate Preview URL remains non-bearer: possession of the lease ID alone does not authorize public access when principal authentication is required.

## Enforced architecture gates

`check-http-api-architecture.sh` now rejects:

- HTTP, raw credentials, or filesystem APIs in the application authorization policy and PreviewAccess service;
- lease, Sandbox binding, ChannelManager, or lease-status orchestration in Preview routes;
- project access, owner, or principal-project decisions in HTTP auth adapters.

Current size evidence:

```text
http_api/mod.rs                                    279
http_api/routes/previews.rs                        145
http_api/candidate_preview_proxy.rs                 86
http_api/auth/candidate_preview.rs                  84
http_api/auth/publication.rs                        63
preview_access.rs                                  161
authorization.rs                                   108
http_api/composition.rs                             48
```

## Verification

```text
Authorization policy unit tests                      passed
HTTP Candidate Preview security matrix               passed
HTTP Candidate manifest mismatch                     passed with 409
Publication API authorization suite                   passed
cargo test --all-targets                              passed
  core library                                      106 passed
  HTTP integration                                   85 passed, 1 external-provider test ignored
  Astro/Fumadocs integration build                     5 passed
cargo fmt --check                                     passed
cargo clippy --all-targets --all-features -D warnings passed
strict Sandbox architecture gate                      passed
remote workspace filesystem boundary gate             passed
Artifact manifest architecture gate                   passed
Runtime bootstrap architecture gate                   passed
Release packaging architecture gate                   passed
Publication control-plane architecture gate           passed
HTTP API architecture gate                            passed
git diff --check                                      passed
```

## G9 status after this increment

G9-D2 is complete. G9 remains in progress: ReleaseEvidence service extraction, Internal route-family split and shared admin layer, final consolidated Runtime/Published gates, and completion audit remain.
