---
date: 2026-07-08
status: passed
type: acceptance-report
topic: phase-a-runtime-freeze-readiness
---

# Phase A Runtime Acceptance Report

## Summary

Phase A runtime acceptance is green on the local checkout at:

```text
d60e2f9 fix(runtime): stabilize preview artifacts and runtime index
```

The runtime has passed the repo-native freeze gate, the real DeepSeek website
generation regression, and the k3d agent-sandbox workspace channel smoke test.
No `apps/web` directory exists in this checkout, so the Phase A boundary remains
runtime-only.

## Verification Commands

### Phase A Freeze Gate

```bash
bash infra/phase-a/verify.sh
```

Result:

```text
passed
```

Coverage included:

- `cargo fmt --manifest-path services/runtime/Cargo.toml -- --check`
- `cargo test --manifest-path services/runtime/Cargo.toml`
- `npm test --prefix packages/shared`
- `npm run typecheck --prefix packages/shared`
- `bash infra/agent-sandbox/run-k8s-e2e.sh`

### Real DeepSeek Website Generation Regression

```bash
DEEPSEEK_API_KEY=... \
DEEPSEEK_E2E_MODEL=deepseek-v4-pro \
cargo test --manifest-path services/runtime/Cargo.toml \
  --test agent_loop real_deepseek_design_md_website_generation_e2e \
  -- --ignored --nocapture
```

Result:

```text
test real_deepseek_design_md_website_generation_e2e ... ok
1 passed; 0 failed; finished in 47.47s
```

### K8s Agent Sandbox E2E

```bash
bash infra/agent-sandbox/run-k8s-e2e.sh
```

Result:

```text
test k8s_sandbox_claim_workspace_channel_smoke ... ok
1 passed; 0 failed
```

## Acceptance Mapping

| Area | Evidence |
|---|---|
| Brief agent loop | `services/runtime/tests/brief_agent.rs` and `services/runtime/tests/http_api.rs` passed |
| Astro build and promotion | `services/runtime/tests/astro_build_agent.rs`, `preview_promotion.rs`, and real DeepSeek regression passed |
| Edit via ContinueRun | `services/runtime/tests/edit_agent.rs` and mock BFF contract tests passed |
| Review/Repair child graph | `services/runtime/tests/review_repair.rs` passed |
| Checkpoint and restart recovery | `services/runtime/tests/checkpoint.rs` passed |
| Permission and sandbox security | `permission_engine.rs`, `tool_permissions_integration.rs`, `sandbox_security.rs`, and k8s E2E passed |
| Runtime HTTP + SSE API | `http_api.rs` passed |
| Phase B contract shape | `mock_bff_contract.rs` and `packages/shared/src/mock-bff-contract-types.test.ts` passed |
| Shared package type safety | `npm test` and `npm run typecheck` in `packages/shared` passed |
| Runtime-only Phase A boundary | `infra/phase-a/verify.sh` checked that `apps/web` does not exist |

## Notes

- The default full Rust test suite still leaves real-provider checks ignored
  unless explicit API credentials are supplied. The DeepSeek website regression
  was run separately with `deepseek-v4-pro` and passed.
- Phase A.5 can now start from the existing Fumadocs runtime loop. The freeze
  decision should avoid breaking Runtime HTTP/SSE API shapes that Phase B BFF
  tests already consume.

## Recommended Next Step

Freeze the Phase A Runtime HTTP/SSE API contract, then start Phase A.5 Docs
Template Loop work from the accepted runtime foundation.
