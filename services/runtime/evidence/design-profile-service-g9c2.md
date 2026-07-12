# G9-C2 DesignProfile Application Service Evidence

Date: 2026-07-12

Branch: `codex/runtime-architecture-g9c-profile-service`

## Boundary delivered

- DesignProfile create, list, get, version history, diff, update, archive, activation, project binding, conversion report, and fidelity report orchestration moved into `DesignProfileService`.
- StartRun resolves Profile precedence, effective execution target, template conflict, source integrity, and prebuild blocking state through the same service.
- `design_profiles.rs` is now an HTTP/auth/DTO adapter with no direct `RuntimeStore` orchestration.
- Activation-specific conflict metadata remains a typed application error and is mapped back to the frozen `currentVersion` plus `validationIssues` JSON contract.
- Design Capsule rendering moved from `agent_loop.rs` to the pure DesignProfile domain, so the Profile service no longer depends on AgentLoop implementation details.
- Profile source artifact validation and template availability checks are owned by the application service and preserve existing 400/404/409/500 mappings.

## Enforced architecture gates

`check-http-api-architecture.sh` now rejects:

- HTTP or direct filesystem APIs in `design_profile_service`;
- a DesignProfile service module above 800 lines;
- direct Store orchestration in `http_api/routes/design_profiles.rs`.

Current size evidence:

```text
http_api/routes/design_profiles.rs                  260
http_api/profile_support.rs                          17
run_lifecycle/start.rs                              617
design_profile/capsule.rs                           185
design_profile_service/lifecycle.rs                 578
design_profile_service/run_context.rs               175
design_profile_service/reports.rs                   132
design_profile_service/diff.rs                       79
```

## Verification

```text
DesignProfile domain/service unit tests              passed
HTTP design_profiles focused suite                   passed
HTTP profile_run_integration suite                   passed
cargo test --all-targets                            passed
cargo clippy --all-targets -- -D warnings           passed
scripts/check-http-api-architecture.sh              passed
git diff --check                                    passed
```

The full 86-test HTTP crate remained at 85 passed and one existing external-provider opt-in test ignored. Publication routes and Published Runtime behavior were not modified.

## G9 status after this increment

G9-C is complete. Remaining work is G9-D Preview, Artifact, Internal, and authorization isolation, followed by the consolidated Published Runtime and architecture completion audit.
