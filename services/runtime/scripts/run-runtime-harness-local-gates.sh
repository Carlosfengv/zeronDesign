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
  cargo test --manifest-path services/runtime/Cargo.toml --test agent_loop -- --nocapture

run_step "runtime http api" \
  cargo test --manifest-path services/runtime/Cargo.toml --test http_api -- --nocapture

run_step "runtime template build agent" \
  cargo test --manifest-path services/runtime/Cargo.toml --test astro_build_agent -- --nocapture

run_step "shared package tests" \
  npm test --prefix packages/shared

run_step "shared package typecheck" \
  npm run typecheck --prefix packages/shared

run_step "script syntax" \
  bash -n services/runtime/scripts/run-real-provider-http-lifecycle-e2e.sh
run_step "provider gate syntax" \
  bash -n services/runtime/scripts/run-runtime-harness-provider-gates.sh
run_step "fumadocs smoke script syntax" \
  bash -n services/runtime/scripts/smoke-fumadocs-docs-build.sh
run_step "computed-style smoke script syntax" \
  bash -n services/runtime/scripts/smoke-computed-style-artifact.sh

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

run_step "provider gate no-key failure" \
  bash -c 'set +e; RUNTIME_E2E_RUN_LOCAL_GATES=0 DEEPSEEK_API_KEY= RUNTIME_E2E_ENV_FILE= DEEPSEEK_API_KEY_FILE= bash services/runtime/scripts/run-runtime-harness-provider-gates.sh >/tmp/runtime-provider-no-key.out 2>/tmp/runtime-provider-no-key.err; rc=$?; set -e; test "$rc" -eq 1; grep -q "DEEPSEEK_API_KEY is required" /tmp/runtime-provider-no-key.err'

run_step "provider gate default no-key fails before local gates" \
  bash -c 'set +e; DEEPSEEK_API_KEY= RUNTIME_E2E_ENV_FILE= DEEPSEEK_API_KEY_FILE= bash services/runtime/scripts/run-runtime-harness-provider-gates.sh >/tmp/runtime-provider-default-no-key.out 2>/tmp/runtime-provider-default-no-key.err; rc=$?; set -e; test "$rc" -eq 1; grep -q "DEEPSEEK_API_KEY is required" /tmp/runtime-provider-default-no-key.err; ! grep -q "== cargo fmt ==" /tmp/runtime-provider-default-no-key.out'

run_step "provider gate key-file dry run" \
  bash -c 'tmpkey="$(mktemp)"; tmpdir="$(mktemp -d)"; artifact_url="http://127.0.0.1:18082/artifacts/real-http-website/current"; trap "rm -f \"$tmpkey\"" EXIT; printf "dummy-key\n" > "$tmpkey"; RUNTIME_E2E_LOG_DIR="$tmpdir" RUNTIME_E2E_RUN_LOCAL_GATES=0 RUNTIME_E2E_DRY_RUN=1 RUNTIME_E2E_ARTIFACT_URL="$artifact_url" DEEPSEEK_API_KEY= DEEPSEEK_API_KEY_FILE="$tmpkey" bash services/runtime/scripts/run-runtime-harness-provider-gates.sh >/tmp/runtime-provider-key-file-dry-run.out; grep -q "DEEPSEEK_API_KEY_PRESENT=true" /tmp/runtime-provider-key-file-dry-run.out; grep -q "RUNTIME_E2E_ARTIFACT_URL=$artifact_url" /tmp/runtime-provider-key-file-dry-run.out; grep -q "RUNTIME_E2E_STYLE_PROJECT=real-http-website" /tmp/runtime-provider-key-file-dry-run.out; grep -q "RUNTIME_E2E_STYLE_STAGE=edit" /tmp/runtime-provider-key-file-dry-run.out; grep -q "RUNTIME_E2E_STYLE_SELECTOR=:root" /tmp/runtime-provider-key-file-dry-run.out; grep -q "RUNTIME_E2E_STYLE_PROPERTY=--runtime-primary" /tmp/runtime-provider-key-file-dry-run.out; grep -q "RUNTIME_E2E_STYLE_EXPECTED=#f97316" /tmp/runtime-provider-key-file-dry-run.out; grep -q "providerGateDryRun=1" "$tmpdir/run-metadata.env"; grep -q "deepseekApiKeyPresent=true" "$tmpdir/run-metadata.env"; grep -q "artifactUrl=$artifact_url" "$tmpdir/run-metadata.env"; grep -q "styleProperty=--runtime-primary" "$tmpdir/run-metadata.env"; ! grep -q "dummy-key" "$tmpdir/run-metadata.env"'

run_step "provider gate env-file dry run" \
  bash -c 'tmpenv="$(mktemp)"; trap "rm -f \"$tmpenv\"" EXIT; printf "DEEPSEEK_API_KEY=dummy-env-key\n" > "$tmpenv"; RUNTIME_E2E_RUN_LOCAL_GATES=0 RUNTIME_E2E_DRY_RUN=1 DEEPSEEK_API_KEY= RUNTIME_E2E_ENV_FILE="$tmpenv" bash services/runtime/scripts/run-runtime-harness-provider-gates.sh | grep -q "DEEPSEEK_API_KEY_PRESENT=true"'

run_step "provider gate dry run skips local gates by default" \
  bash -c 'tmpkey="$(mktemp)"; trap "rm -f \"$tmpkey\"" EXIT; printf "dummy-key\n" > "$tmpkey"; RUNTIME_E2E_DRY_RUN=1 RUNTIME_E2E_STYLE_PROJECT=real-http-docs RUNTIME_E2E_STYLE_STAGE=edit DEEPSEEK_API_KEY= DEEPSEEK_API_KEY_FILE="$tmpkey" bash services/runtime/scripts/run-runtime-harness-provider-gates.sh >/tmp/runtime-provider-dry-run-default.out; grep -q "PROVIDER_GATE_DRY_RUN=1" /tmp/runtime-provider-dry-run-default.out; grep -q "RUNTIME_E2E_STYLE_PROJECT=real-http-docs" /tmp/runtime-provider-dry-run-default.out; grep -q "RUNTIME_E2E_STYLE_STAGE=edit" /tmp/runtime-provider-dry-run-default.out; ! grep -q "== cargo fmt ==" /tmp/runtime-provider-dry-run-default.out'

run_step "fumadocs real build smoke" \
  bash services/runtime/scripts/smoke-fumadocs-docs-build.sh

run_step "computed-style local artifact smoke" \
  bash services/runtime/scripts/smoke-computed-style-artifact.sh

run_step "computed-style custom property smoke" \
  bash -c 'tmpdir=".runtime-evidence/style-only-custom-property-$(date +%Y%m%d-%H%M%S)"; RUNTIME_E2E_LOG_DIR="$tmpdir" RUNTIME_E2E_STYLE_SELECTOR=":root" RUNTIME_E2E_STYLE_PROPERTY="--runtime-primary" RUNTIME_E2E_STYLE_EXPECTED="#f97316" bash services/runtime/scripts/smoke-computed-style-artifact.sh >/tmp/runtime-style-custom-property.out; grep -q "\"property\": \"--runtime-primary\"" "$tmpdir/evidence-summary.json"; grep -q "\"actual\": \"#f97316\"" "$tmpdir/evidence-summary.json"; echo "STYLE_CUSTOM_PROPERTY_EVIDENCE_DIR=$tmpdir"'

run_step "computed-style metadata records target" \
  bash -c 'tmpdir="$(mktemp -d)"; fixture="$(mktemp -d)"; mkdir -p "$fixture/_astro"; printf "<link rel=\"stylesheet\" href=\"/_astro/app.css\"><h1 id=\"probe\">x</h1>" > "$fixture/index.html"; printf ":root{--runtime-primary:#f97316}#probe{color:var(--runtime-primary)}" > "$fixture/_astro/app.css"; RUNTIME_E2E_STYLE_ONLY=1 RUNTIME_E2E_LOG_DIR="$tmpdir" RUNTIME_E2E_ARTIFACT_URL="file://$fixture/index.html" RUNTIME_E2E_STYLE_PROJECT=real-http-docs RUNTIME_E2E_STYLE_STAGE=edit RUNTIME_E2E_STYLE_SELECTOR="#probe" RUNTIME_E2E_STYLE_PROPERTY=color RUNTIME_E2E_STYLE_EXPECTED="#f97316" bash services/runtime/scripts/run-real-provider-http-lifecycle-e2e.sh >/tmp/runtime-style-metadata.out; grep -q "styleProject=real-http-docs" "$tmpdir/run-metadata.env"; grep -q "styleStage=edit" "$tmpdir/run-metadata.env"'

run_step "git diff whitespace" \
  git diff --check
