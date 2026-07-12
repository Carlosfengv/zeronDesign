# G9-B2 StartRun Application Extraction Evidence

Date: 2026-07-12

Branch: `codex/runtime-architecture-g9b-start-run`

## Boundary delivered

- `POST /runs` is now a 19-line transport adapter that converts the HTTP DTO, invokes `RunLifecycleService::start`, and maps the typed outcome/error.
- StartRun validation, project/lifecycle checks, Run creation, Profile preflight/attachment, fidelity configuration, Sandbox binding, Edit restore, state transitions, audit, event ordering, and session registration are owned by the application boundary.
- `BuildSandboxProvisioner`, `EditWorkspaceRestorer`, and `RunSessionLauncher` are explicit application ports.
- Kubernetes/Phase-A Sandbox claim/wait/release, workspace snapshot I/O, and supervised QuerySession construction live in Runtime adapters.
- Startup recovery now propagates duplicate/failed resumed-session registration instead of silently reporting readiness.

## Durable failure matrix

The implementation now records these post-create failure paths explicitly:

| Failure stage | Durable result |
|---|---|
| Profile attach / fidelity configuration | audit + `cancelled` Run + terminal event |
| Explicit Sandbox bind | audit + `cancelled` Run + terminal event |
| Sandbox provision / exclusive acquire | binding compensation where applicable + audit + `cancelled` Run + terminal event |
| Edit workspace restore | binding cleanup through terminal transition + audit + `cancelled` Run + terminal event |
| Session registration | Run remains durable `queued`; audit + `queued:session_registration_failed` event; bootstrap may retry the same run ID |

Application failure-injection tests prove Sandbox provision failure, Edit restore failure, and Session registration failure behavior through injected ports.

## Enforced architecture gates

`check-http-api-architecture.sh` now rejects:

- Axum or direct filesystem access in RunLifecycle application modules;
- any RunLifecycle module above 800 lines;
- any Start/Continue/Cancel/Permission route adapter above 50 lines.

Current size evidence:

```text
http_api/mod.rs                                      294
http_api/routes/runs/start.rs                         19
run_lifecycle/start.rs                               788
run_lifecycle/start_failure.rs                        86
run_lifecycle/start_validation.rs                     85
runtime/edit_workspace_restorer.rs                   156
runtime/run_sandbox_provisioner.rs                    39
runtime/session_launcher.rs                           79
```

## Verification

```text
cargo test --all-targets                            passed
cargo test --test http_api                          85 passed, 1 existing opt-in test ignored
cargo clippy --all-targets -- -D warnings           passed
scripts/check-http-api-architecture.sh              passed
git diff --check                                    passed
```

The complete StartRun HTTP suites for validation, bindings, repair, Edit lifecycle, Profile integration, website lifecycle, and docs lifecycle passed without assertion changes. Publication routes and Published Runtime behavior were not modified.

## Remaining G9 work

- G9-C: extract DesignProfile parser, validation, lifecycle, capability, and conflict services from HTTP/Run modules.
- G9-D: isolate Preview, Artifact, Internal, and authorization boundaries.
- Final G9 Published Runtime regression, architectural completion audit, and consolidated sign-off.
