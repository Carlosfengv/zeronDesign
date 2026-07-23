#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$ROOT_DIR"

run_step() {
  local label="$1"
  shift
  printf '\n== %s ==\n' "$label"
  "$@"
}

run_step "cargo fmt" \
  cargo fmt --manifest-path services/runtime/Cargo.toml -- --check

run_step "remote workspace filesystem boundary" \
  services/runtime/scripts/check-remote-workspace-fs-boundary.sh

run_step "sandbox architecture boundary" \
  services/runtime/scripts/check-sandbox-architecture.sh

run_step "sandbox tool contract baseline" \
  cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_contract_baseline -- --nocapture

run_step "runtime agent hooks" \
  cargo test --manifest-path services/runtime/Cargo.toml agent_hooks -- --nocapture

run_step "runtime sandbox tools" \
  cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools -- --nocapture

run_step "runtime preview promotion" \
  cargo test --manifest-path services/runtime/Cargo.toml --test preview_promotion -- --nocapture

run_step "runtime permission engine" \
  cargo test --manifest-path services/runtime/Cargo.toml --test permission_engine -- --nocapture

run_step "runtime tool permissions" \
  cargo test --manifest-path services/runtime/Cargo.toml --test tool_permissions_integration -- --nocapture

run_step "runtime agent loop" \
  env RUST_MIN_STACK=8388608 \
  cargo test --manifest-path services/runtime/Cargo.toml --test agent_loop -- --nocapture

run_step "runtime http api" \
  cargo test --manifest-path services/runtime/Cargo.toml --test http_api -- --nocapture

run_step "runtime template build agent" \
  cargo test --manifest-path services/runtime/Cargo.toml --test template_build_agent -- --nocapture

run_step "shared package tests" \
  npm test --prefix packages/shared

run_step "shared package typecheck" \
  npm run typecheck --prefix packages/shared

run_step "script syntax" \
  bash -n services/runtime/scripts/run-real-provider-http-lifecycle-e2e.sh
run_step "runtime RC preflight registry policy syntax" \
  node --check infra/agent-sandbox/test-preflight-registry-policy.mjs
run_step "runtime RC preflight registry policy tests" \
  node infra/agent-sandbox/test-preflight-registry-policy.mjs
run_step "provider gate syntax" \
  bash -n services/runtime/scripts/run-runtime-harness-provider-gates.sh
run_step "fumadocs smoke script syntax" \
  bash -n services/runtime/scripts/smoke-fumadocs-docs-build.sh
run_step "computed-style smoke script syntax" \
  bash -n services/runtime/scripts/smoke-computed-style-artifact.sh
run_step "fumadocs dark-theme smoke script syntax" \
  bash -n services/runtime/scripts/smoke-fumadocs-dark-theme.sh

run_step "node script syntax" \
  node --check services/runtime/scripts/verify-computed-style.mjs
run_step "artifact URL extractor syntax" \
  node --check services/runtime/scripts/extract-provider-artifact-url.mjs
run_step "evidence summary syntax" \
  node --check services/runtime/scripts/summarize-real-provider-evidence.mjs
run_step "evidence summary tests syntax" \
  node --check services/runtime/scripts/test-real-provider-evidence-summary.mjs
run_step "evidence summary tests" \
  node services/runtime/scripts/test-real-provider-evidence-summary.mjs
run_step "design-context canary evidence validator syntax" \
  node --check services/runtime/scripts/validate-design-context-canary-evidence.mjs
run_step "design-context canary evidence validator tests" \
  node services/runtime/scripts/test-design-context-canary-evidence-validator.mjs
run_step "release evidence validator syntax" \
  node --check services/runtime/scripts/validate-release-evidence.mjs
run_step "release evidence validator tests" \
  node services/runtime/scripts/test-release-evidence-validator.mjs
run_step "release evidence aggregator syntax" \
  node --check services/runtime/scripts/aggregate-release-evidence.mjs
run_step "release evidence aggregator tests" \
  node services/runtime/scripts/test-aggregate-release-evidence.mjs
run_step "generation-context operations artifacts" \
  node services/runtime/scripts/test-generation-context-ops-artifacts.mjs
run_step "generation-context monitoring proof" \
  node infra/generation-reliability/verify-generation-context-monitoring.test.mjs
run_step "generation-context rollout evaluator" \
  node services/runtime/scripts/test-generation-context-rollout.mjs
run_step "generation-context paired sample creator" \
  node services/runtime/scripts/test-create-generation-context-paired-sample.mjs
run_step "generation-context real-provider paired sample collector" \
  node services/runtime/scripts/test-collect-generation-context-paired-sample.mjs
run_step "generation-context paired-cohort ledger" \
  node services/runtime/scripts/test-generation-context-paired-cohort-ledger.mjs
run_step "generation-context paired-pair runner" \
  node services/runtime/scripts/test-run-generation-context-paired-pair.mjs
run_step "generation-context baseline calculator" \
  node services/runtime/scripts/test-generation-context-baseline.mjs
run_step "design-context canary ledger syntax" \
  node --check services/runtime/scripts/design-context-canary-ledger.mjs
run_step "design-context canary ledger tests syntax" \
  node --check services/runtime/scripts/test-design-context-canary-ledger.mjs
run_step "design-context canary ledger tests" \
  node services/runtime/scripts/test-design-context-canary-ledger.mjs
run_step "design-context canary metrics collector syntax" \
  node --check services/runtime/scripts/collect-design-context-canary-metrics.mjs
run_step "design-context canary metrics collector tests syntax" \
  node --check services/runtime/scripts/test-design-context-canary-metrics-collector.mjs
run_step "design-context canary metrics collector tests" \
  node services/runtime/scripts/test-design-context-canary-metrics-collector.mjs
run_step "design-context canary report syntax" \
  node --check services/runtime/scripts/render-design-context-canary-report.mjs
run_step "design-context canary report tests syntax" \
  node --check services/runtime/scripts/test-design-context-canary-report.mjs
run_step "design-context canary report tests" \
  node services/runtime/scripts/test-design-context-canary-report.mjs
run_step "design-context canary alert dispatcher syntax" \
  node --check services/runtime/scripts/dispatch-design-context-canary-alerts.mjs
run_step "design-context canary alert dispatcher tests syntax" \
  node --check services/runtime/scripts/test-design-context-canary-alert-dispatcher.mjs
run_step "design-context canary alert dispatcher tests" \
  node services/runtime/scripts/test-design-context-canary-alert-dispatcher.mjs
run_step "design-context canary rollback syntax" \
  node --check services/runtime/scripts/run-design-context-canary-rollback.mjs
run_step "design-context canary rollback tests syntax" \
  node --check services/runtime/scripts/test-design-context-canary-rollback.mjs
run_step "design-context canary rollback tests" \
  node services/runtime/scripts/test-design-context-canary-rollback.mjs

run_step "provider gate no-gateway failure" \
  bash -c 'set +e; RUNTIME_E2E_RUN_LOCAL_GATES=0 MODEL_GATEWAY_URL= RUNTIME_E2E_ENV_FILE= MODEL_GATEWAY_AUTH_TOKEN_FILE= bash services/runtime/scripts/run-runtime-harness-provider-gates.sh >/tmp/runtime-provider-no-gateway.out 2>/tmp/runtime-provider-no-gateway.err; rc=$?; set -e; test "$rc" -eq 1; grep -q "MODEL_GATEWAY_URL is required" /tmp/runtime-provider-no-gateway.err'

run_step "provider gate default no-gateway fails before local gates" \
  bash -c 'set +e; MODEL_GATEWAY_URL= RUNTIME_E2E_ENV_FILE= MODEL_GATEWAY_AUTH_TOKEN_FILE= bash services/runtime/scripts/run-runtime-harness-provider-gates.sh >/tmp/runtime-provider-default-no-gateway.out 2>/tmp/runtime-provider-default-no-gateway.err; rc=$?; set -e; test "$rc" -eq 1; grep -q "MODEL_GATEWAY_URL is required" /tmp/runtime-provider-default-no-gateway.err; ! grep -q "== cargo fmt ==" /tmp/runtime-provider-default-no-gateway.out'

run_step "provider gate workload-token-file dry run" \
  bash -c 'tmptoken="$(mktemp)"; tmpdir="$(mktemp -d)"; artifact_url="http://127.0.0.1:18082/artifacts/real-http-website/current"; trap "rm -f \"$tmptoken\"" EXIT; printf "dummy-workload-token\n" > "$tmptoken"; RUNTIME_E2E_LOG_DIR="$tmpdir" RUNTIME_E2E_RUN_LOCAL_GATES=0 RUNTIME_E2E_DRY_RUN=1 RUNTIME_E2E_ARTIFACT_URL="$artifact_url" MODEL_GATEWAY_URL="http://127.0.0.1:19000" MODEL_GATEWAY_AUTH_TOKEN= MODEL_GATEWAY_AUTH_TOKEN_FILE="$tmptoken" bash services/runtime/scripts/run-runtime-harness-provider-gates.sh >/tmp/runtime-provider-token-file-dry-run.out; grep -q "MODEL_GATEWAY_URL=http://127.0.0.1:19000" /tmp/runtime-provider-token-file-dry-run.out; grep -q "MODEL_RESOURCE_ID=deepseek-v4-pro" /tmp/runtime-provider-token-file-dry-run.out; grep -q "MODEL_GATEWAY_AUTH_TOKEN_PRESENT=true" /tmp/runtime-provider-token-file-dry-run.out; grep -q "RUNTIME_E2E_ARTIFACT_URL=$artifact_url" /tmp/runtime-provider-token-file-dry-run.out; grep -q "RUNTIME_E2E_STYLE_PROJECT=real-http-website" /tmp/runtime-provider-token-file-dry-run.out; grep -q "RUNTIME_E2E_STYLE_STAGE=edit" /tmp/runtime-provider-token-file-dry-run.out; grep -q "RUNTIME_E2E_STYLE_SELECTOR=:root" /tmp/runtime-provider-token-file-dry-run.out; grep -q "RUNTIME_E2E_STYLE_PROPERTY=--runtime-primary" /tmp/runtime-provider-token-file-dry-run.out; grep -q "RUNTIME_E2E_STYLE_EXPECTED=#f97316" /tmp/runtime-provider-token-file-dry-run.out; grep -q "providerGateDryRun=1" "$tmpdir/run-metadata.env"; grep -q "modelGatewayUrl=http://127.0.0.1:19000" "$tmpdir/run-metadata.env"; grep -q "modelResourceId=deepseek-v4-pro" "$tmpdir/run-metadata.env"; grep -q "modelGatewayAuthTokenPresent=true" "$tmpdir/run-metadata.env"; grep -q "artifactUrl=$artifact_url" "$tmpdir/run-metadata.env"; grep -q "styleProperty=--runtime-primary" "$tmpdir/run-metadata.env"; ! grep -q "dummy-workload-token" "$tmpdir/run-metadata.env"'

run_step "provider gate env-file dry run" \
  bash -c 'tmpenv="$(mktemp)"; trap "rm -f \"$tmpenv\"" EXIT; printf "MODEL_GATEWAY_URL=http://127.0.0.1:19000\nMODEL_GATEWAY_AUTH_TOKEN=dummy-workload-token\nDEEPSEEK_E2E_MODEL=deepseek-v4-pro\n" > "$tmpenv"; RUNTIME_E2E_RUN_LOCAL_GATES=0 RUNTIME_E2E_DRY_RUN=1 MODEL_GATEWAY_URL= RUNTIME_E2E_ENV_FILE="$tmpenv" bash services/runtime/scripts/run-runtime-harness-provider-gates.sh | grep -q "MODEL_GATEWAY_AUTH_TOKEN_PRESENT=true"'

run_step "provider gate dry run skips local gates by default" \
  bash -c 'RUNTIME_E2E_DRY_RUN=1 RUNTIME_E2E_STYLE_PROJECT=real-http-docs RUNTIME_E2E_STYLE_STAGE=edit MODEL_GATEWAY_URL="http://127.0.0.1:19000" bash services/runtime/scripts/run-runtime-harness-provider-gates.sh >/tmp/runtime-provider-dry-run-default.out; grep -q "PROVIDER_GATE_DRY_RUN=1" /tmp/runtime-provider-dry-run-default.out; grep -q "RUNTIME_E2E_STYLE_PROJECT=real-http-docs" /tmp/runtime-provider-dry-run-default.out; grep -q "RUNTIME_E2E_STYLE_STAGE=edit" /tmp/runtime-provider-dry-run-default.out; ! grep -q "== cargo fmt ==" /tmp/runtime-provider-dry-run-default.out'

run_step "fumadocs real build smoke" \
  bash services/runtime/scripts/smoke-fumadocs-docs-build.sh

run_step "computed-style local artifact smoke" \
  bash services/runtime/scripts/smoke-computed-style-artifact.sh

run_step "fumadocs dark-theme computed-style smoke" \
  bash services/runtime/scripts/smoke-fumadocs-dark-theme.sh

run_step "computed-style custom property smoke" \
  bash -c 'tmpdir=".runtime-evidence/style-only-custom-property-$(date +%Y%m%d-%H%M%S)"; RUNTIME_E2E_LOG_DIR="$tmpdir" RUNTIME_E2E_STYLE_SELECTOR=":root" RUNTIME_E2E_STYLE_PROPERTY="--runtime-primary" RUNTIME_E2E_STYLE_EXPECTED="#f97316" bash services/runtime/scripts/smoke-computed-style-artifact.sh >/tmp/runtime-style-custom-property.out; grep -q "\"property\": \"--runtime-primary\"" "$tmpdir/evidence-summary.json"; grep -q "\"actual\": \"#f97316\"" "$tmpdir/evidence-summary.json"; echo "STYLE_CUSTOM_PROPERTY_EVIDENCE_DIR=$tmpdir"'

run_step "computed-style metadata records target" \
  bash -c 'tmpdir="$(mktemp -d)"; fixture="$(mktemp -d)"; mkdir -p "$fixture/_next"; printf "<link rel=\"stylesheet\" href=\"/_next/app.css\"><h1 id=\"probe\">x</h1>" > "$fixture/index.html"; printf ":root{--runtime-primary:#f97316}#probe{color:var(--runtime-primary)}" > "$fixture/_next/app.css"; RUNTIME_E2E_STYLE_ONLY=1 RUNTIME_E2E_LOG_DIR="$tmpdir" RUNTIME_E2E_ARTIFACT_URL="file://$fixture/index.html" RUNTIME_E2E_STYLE_PROJECT=real-http-docs RUNTIME_E2E_STYLE_STAGE=edit RUNTIME_E2E_STYLE_SELECTOR="#probe" RUNTIME_E2E_STYLE_PROPERTY=color RUNTIME_E2E_STYLE_EXPECTED="#f97316" bash services/runtime/scripts/run-real-provider-http-lifecycle-e2e.sh >/tmp/runtime-style-metadata.out; grep -q "styleProject=real-http-docs" "$tmpdir/run-metadata.env"; grep -q "styleStage=edit" "$tmpdir/run-metadata.env"'

run_step "git diff whitespace" \
  git diff --check
