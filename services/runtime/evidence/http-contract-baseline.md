# Runtime HTTP Contract Baseline

## Identity

| Field | Value |
|---|---|
| Baseline commit | `96f9f92c4abdee469f7bd943aa317e8091f482f8` |
| Baseline date | `2026-07-12` |
| Runtime crate | `services/runtime` |
| Route source | `src/http_api.rs` |
| Executable manifest | `contracts/http-routes.json` |

## Worktree Boundary

The following untracked file existed before G0 implementation and is user-owned. It must not be
staged by a G0 implementation commit:

```text
2026-07-11-harness-runtime-product-optimization-spec.md
```

The three `2026-07-12` architecture plans are planning artifacts for the current architecture
initiative. They remain separate from production-code changes unless explicitly included in a
documentation commit.

## Test Discovery Baseline

Before adding the executable manifest harness, Cargo reported exactly 70 tests:

```bash
cd services/runtime
cargo test --test http_api -- --list
```

```text
70 tests, 0 benchmarks
```

After adding `tests/http_api/contract_manifest.rs`, the expected floor is 75 discovered tests. The
five additions freeze:

- complete Axum route/method/surface inventory;
- manifest metadata and fail-closed internal-route annotations;
- JSON error status, content type, and body shape;
- SSE content type and cache policy;
- the 393,216-byte design-source request limit.

## G0 Verification Record

| Gate | Result | Evidence |
|---|---|---|
| Manifest characterization tests | passed | `5 passed; 0 failed` |
| HTTP test discovery | passed | `75 tests, 0 benchmarks` |
| Full HTTP integration suite | passed | `74 passed; 0 failed; 1 real-provider test ignored` |
| `cargo fmt --check` | passed | No formatting diff |
| `cargo clippy --all-targets --all-features -- -D warnings` | passed | Existing warnings were mechanically remediated or replaced with narrow, documented boundary exceptions; the global gate was not weakened |
| `cargo test --all-targets` | passed | All executed targets passed; only explicitly environment-gated tests were ignored |
| Strict sandbox architecture gate | passed | `sandbox architecture check passed (strict=1)` |
| Remote workspace FS boundary gate | passed | `remote workspace filesystem boundary check passed` |
| Real Fumadocs production build | passed | `fumadocs_docs_real_next_build_smoke ... ok` |

All G0 technical gates are green. The execution tracker remains `in_progress` until the G0 commits
are reviewed and merged according to PR-01.
