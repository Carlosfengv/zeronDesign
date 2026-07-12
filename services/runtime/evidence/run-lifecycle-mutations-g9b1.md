# G9-B1 RunLifecycle Mutation Extraction Evidence

Date: 2026-07-12

Branch: `codex/runtime-architecture-g9b-run-mutations`

## Boundary delivered

- Cancel, permission decision, and continue orchestration moved from Axum handlers into the `run_lifecycle` application boundary.
- The three handlers are now 17, 29, and 22 lines and only perform transport validation, application invocation, and HTTP response/error mapping.
- Application outcomes and errors contain no Axum types. `http_api/error.rs` is the sole mapper from `RunLifecycleError` to the frozen 404/409/500 contracts.
- Run status, conversation, event, audit, cancellation cleanup, and session-launch ordering remain unchanged.
- Session launch is represented by the injected `RunSessionLauncher` command port. The concrete Runtime adapter owns Model, Supervisor, control-plane executor construction, and local workspace bootstrap.
- The service is assembled once in `router_with_state` and injected as an Axum extension; mutation handlers do not construct stores, backends, or launchers during a request.

## Enforced architecture gates

`check-http-api-architecture.sh` now fails when the RunLifecycle application boundary:

- imports Axum; or
- directly calls `std::fs` or `tokio::fs`.

Current size evidence:

```text
http_api/mod.rs                                      292
http_api/routes/runs/cancel.rs                       17
http_api/routes/runs/permission.rs                   29
http_api/routes/runs/continue_run.rs                 22
run_lifecycle/cancel.rs                              81
run_lifecycle/permission.rs                         165
run_lifecycle/continue_run.rs                       314
runtime/session_launcher.rs                          78
```

All production modules remain below the 800-line limit and the HTTP façade remains below 300 lines.

## Behavioral verification

Focused tests passed unchanged:

```text
run_lifecycle::tests                                 1 passed
http_api run_mutations                              11 passed
http_api profile_run_integration                     7 passed
http_api runs_bindings                               5 passed
http_api runs_start                                  5 passed
```

The injected-launcher unit test proves the application use case can be executed without Axum and delegates session ownership through the command port.

Full verification:

```text
cargo test --all-targets                            passed
cargo clippy --all-targets -- -D warnings           passed
scripts/check-http-api-architecture.sh              passed
git diff --check                                    passed
```

The existing real-provider/network tests remain ignored by their established opt-in contracts. Publication routes and Publication behavior were not changed in this increment.

## Remaining G9 work

- G9-B2: StartRun, sandbox provision/bind, and Edit restore application extraction.
- G9-C: DesignProfile parser/validator/service extraction.
- G9-D: Preview, Artifact, Internal, and authorization boundary extraction.
- Final G9 Published Runtime regression and consolidated sign-off.
