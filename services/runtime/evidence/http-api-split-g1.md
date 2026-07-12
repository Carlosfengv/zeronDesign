# Runtime HTTP API G1 Split Evidence

## Identity

| Field | Value |
|---|---|
| Base commit | `8f1527f66d6709e454d99da81e4a782c4674bb0a` |
| Branch | `codex/runtime-architecture-g1` |
| Goal | G1 HTTP mechanical boundary split |
| External contract | `services/runtime/contracts/http-routes.json` |

## Structural Result

| Requirement | Result |
|---|---|
| Legacy `src/http_api.rs` removed | passed |
| `http_api/mod.rs` façade | 271 lines, limit 300 |
| Largest production HTTP module | 770 lines, limit 800 |
| Route families | system, runs, run events, design sources, design profiles, projects, previews, artifacts, internal, capture |
| Run route split | start, continue, cancel, permission |
| Auth split | internal admin and candidate preview |
| Independent route-family test modules | 10 |
| Cargo-discovered HTTP tests | 85, up from G0 floor of 75 |

The split preserves `pub mod http_api`, `AppState`, `router`, `router_with_state`,
`recovered_router`, and `capture_router_with_state` compatibility entry points.

## Verification

| Gate | Result |
|---|---|
| `cargo fmt --check` | passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | passed |
| `cargo test --test http_api` | `84 passed; 0 failed; 1 environment-gated ignored` |
| `cargo test --all-targets` | passed; only explicitly environment-gated tests ignored |
| HTTP route manifest drift | passed |
| HTTP architecture boundary | passed |
| Strict Sandbox architecture | passed |
| Remote workspace FS boundary | passed |
| Real Fumadocs production build | passed |

## Preserved Invariants

- All 41 Axum route declarations and 44 method/path contracts remain in the executable manifest.
- The design-source 393,216-byte body limit remains route-local.
- JSON error status/body and SSE content/cache behavior remain characterized.
- Internal routes remain internal-service authorized and feature flags are unchanged.
- Capture routes remain on the isolated capture router.
- No Store state transition or public payload field was intentionally changed.
- The user-owned untracked architecture/product document was not staged.

PR-02 passed the remote `architecture-contracts` check and was merged to `main` as
`eb5834e`.
