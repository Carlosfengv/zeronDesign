# Runtime Bootstrap and Supervisor G2 Evidence

## Identity

| Field | Value |
|---|---|
| Base commit | `eb5834e` |
| Branch | `codex/runtime-architecture-g2` |
| Goal | G2 explicit bootstrap and background-task ownership |
| Evidence date | 2026-07-12 |

## Structural Result

| Requirement | Result |
|---|---|
| Production bootstrap | `main.rs` delegates only to `RuntimeBootstrap::run` |
| Recovery ownership | startup recovery moved from HTTP into `runtime/bootstrap.rs` |
| Listen-after-recovery | public and capture listeners bind only after recovery completes |
| Task ownership | session, public server, and capture server register with `RuntimeSupervisor` |
| Duplicate ownership | duplicate task names fail closed |
| Readiness | recovered, shutdown, and fatal-task state drive `/health` |
| Graceful shutdown | bounded deadline with completed and aborted task evidence |
| Test startup semantics | `TestRuntimeBuilder` exposes explicit fresh and recovered modes |
| Detached HTTP tasks | architecture gate rejects direct `tokio::spawn` under `http_api` |

The Supervisor keeps its shutdown channel alive for the complete lifetime of each
owned task. This prevents an ephemeral caller clone from being interpreted as a
shutdown event and is covered by a dedicated regression test.

## Verification

| Gate | Result |
|---|---|
| `cargo fmt --check` | passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | passed |
| `cargo test --test http_api` | `84 passed; 0 failed; 1 environment-gated ignored` |
| `cargo test --all-targets` | passed; only explicitly environment-gated tests ignored |
| session-resume race regression | passed 10 consecutive runs |
| Runtime bootstrap architecture | passed |
| HTTP architecture boundary | passed |
| Strict Sandbox architecture | passed |
| Remote workspace FS boundary | passed |
| Real Fumadocs production build | passed |
| shared schemas | `24 passed`; TypeScript typecheck passed |

## Preserved Invariants

- Existing startup recovery ordering and persisted-run semantics remain in the recovered path.
- HTTP route declarations, request/response payloads, auth rules, and SSE behavior are unchanged.
- Health retains the existing ready response and adds an explicit `503 not_ready` fatal state.
- Production startup cannot bind either listener before recovery succeeds.
- Controller and reconcile tasks can be added through the same unique-name Supervisor registry.
- The user-owned untracked architecture/product document was not staged.
