# G9-A HTTP Integration Test Split Evidence

Date: 2026-07-12

Branch: `codex/runtime-architecture-g9a-http-tests`

## Mechanical boundary

- `services/runtime/tests/http_api.rs` is now a 15-line Cargo integration-test crate root with no test bodies.
- Shared imports, environment locks, fixtures, and reusable HTTP/runtime helpers live in `tests/http_api/suite/mod.rs`.
- Test bodies are grouped into 16 bounded-context modules under `tests/http_api/cases/`.
- No route, DTO, status code, mutation order, production implementation, or test assertion changed in this PR.
- Every HTTP test source file is at most 800 lines; the largest case module is below 700 lines after formatting.

## Cargo discovery

Command:

```text
cargo test --manifest-path services/runtime/Cargo.toml --test http_api -- --list
```

Result:

```text
86 tests, 0 benchmarks
```

This equals the frozen pre-migration inventory. Test names gained bounded-context module prefixes, while function names and assertions remain unchanged.

## Execution

Command:

```text
cargo test --manifest-path services/runtime/Cargo.toml --test http_api
```

Result:

```text
test result: ok. 85 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
```

The ignored case remains the existing real-provider test requiring an API key and external registries.

## Enforced regression gates

`check-http-api-architecture.sh` now rejects:

- an HTTP integration crate root above 100 lines;
- test bodies in the crate root;
- any HTTP test module above 800 lines;
- a source inventory below the frozen 86-test baseline.

G9-A is intentionally a test-only mechanical split. RunLifecycle, DesignProfile, Preview/Artifact/Internal/Auth service extraction remains in G9-B through G9-D and is not claimed by this evidence.
