# G9-C1 DesignProfile Pure Domain Evidence

Date: 2026-07-12

Branch: `codex/runtime-architecture-g9c-design-profile`

## Boundary delivered

- DesignProfile source parsing moved from `http_api/profile_support.rs` into `design_profile/source.rs`.
- Candidate validation and legacy `intent` to canonical component `role` normalization moved into `design_profile/validation.rs`.
- Signature-rule surface matching and template capability/token checks moved into `design_profile/capability.rs`.
- Scope normalization moved into the DesignProfile domain façade.
- The remaining HTTP support file is a 54-line compatibility adapter for request DTO extraction and transport error mapping.

The pure modules do not import Axum, HTTP headers/status types, or `RuntimeStore`. Source parsing therefore cannot mutate runtime state or interpret operational instructions as design semantics.

## Enforced gates

`check-http-api-architecture.sh` now rejects:

- Axum, `HeaderMap`, `StatusCode`, Router, or `RuntimeStore` dependencies in `src/design_profile`;
- any DesignProfile domain module above 800 lines.

Current size evidence:

```text
http_api/profile_support.rs                           54
design_profile/source.rs                            175
design_profile/validation.rs                         90
design_profile/capability.rs                         40
design_profile/mod.rs                                29
```

## Verification

```text
DesignProfile pure-domain unit tests                 3 passed
HTTP design_sources focused tests                    2 passed
HTTP design_profiles focused tests                   3 passed
cargo test --all-targets                            passed
cargo clippy --all-targets -- -D warnings           passed
scripts/check-http-api-architecture.sh              passed
git diff --check                                    passed
```

The new tests freeze operational-instruction quarantine, source token extraction, component-role conflict handling, and imported-candidate blocking issues.

## Remaining G9-C work

- Extract CRUD, version, archive, activation, project binding, diff, and fidelity-report orchestration into DesignProfile application services.
- Move StartRun DesignProfile resolution/preflight behind that service boundary.
- Keep HTTP routes limited to authorization, DTO conversion, service invocation, and response mapping.
