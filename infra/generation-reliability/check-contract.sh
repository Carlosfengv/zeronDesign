#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

bash -n infra/generation-reliability/run-k3d-matrix.sh
bash -n infra/generation-reliability/run-real-provider-examples.sh
bash -n infra/generation-reliability/configure-runtime-provider-gateway.sh
bash -n infra/generation-reliability/configure-runtime-provider-gateway.test.sh
bash -n infra/generation-reliability/test-fixtures/fake-kubectl-runtime-provider-gateway.sh
bash -n infra/provider-gateway/reconcile-k3d-model-resources.sh
bash -n infra/provider-gateway/apply-k3d-persistent-sqlite.sh
bash -n infra/agent-sandbox/run-runtime-rc-gate.sh
bash -n infra/agent-sandbox/run-runtime-recovery-gate.sh
node --check infra/generation-reliability/summarize-matrix-evidence.mjs
node --check infra/generation-reliability/run-real-provider-examples.mjs
node --check infra/generation-reliability/run-real-provider-examples.test.mjs
node --check infra/generation-reliability/audit-real-provider-stability.mjs
node --check infra/generation-reliability/audit-real-provider-stability.test.mjs
node --check infra/agent-sandbox/verify-runtime-version.mjs
node --check infra/agent-sandbox/verify-runtime-version.test.mjs
node --check services/runtime/scripts/aggregate-release-evidence.mjs
node --check services/runtime/scripts/check-browser-fonts.mjs
node infra/agent-sandbox/runtime/fixture-model-gateway.test.cjs
node infra/agent-sandbox/verify-runtime-version.test.mjs
node infra/generation-reliability/run-real-provider-examples.test.mjs
node infra/generation-reliability/audit-real-provider-stability.test.mjs
bash infra/generation-reliability/configure-runtime-provider-gateway.test.sh
DEEPSEEK_API_KEY=fixture \
  RUNTIME_RC_PROVIDER_MODE=deepseek \
  RUNTIME_RC_TOKEN_BUDGET_SELF_TEST=1 \
  RUNTIME_RC_REAL_TOTAL_TOKEN_CEILING=240000 \
  bash infra/agent-sandbox/run-runtime-rc-gate.sh
cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_security \
  deployment_manifests_parse_without_cluster -- --exact

node - <<'NODE'
const fs = require("node:fs");
const config = JSON.parse(fs.readFileSync(
  "infra/provider-gateway/model-resources.deepseek-v4-pro.json",
  "utf8",
));
const resource = config.resources?.find(item => item.id === "deepseek-v4-pro");
const policy = config.policies?.find(item => item.id === "local-deepseek-v4-pro-default");
if (!resource || resource.revision !== 4 || resource.defaults?.maxAttempts !== 3) {
  console.error("deepseek-v4-pro authority must remain at reviewed revision 4 with maxAttempts=3");
  process.exit(2);
}
if (resource.auth?.secretRef !== "file:/var/run/secrets/deepseek/api-key") process.exit(2);
if (!policy?.directSelection?.allowedModelResourceIds?.includes(resource.id)) process.exit(2);
if (JSON.stringify(config).match(/sk-[A-Za-z0-9_-]{12,}/)) process.exit(2);
NODE

rg -q 'PROVIDER_GATEWAY_ADMIN_TOKEN_FILE' infra/provider-gateway/k3d-persistent-sqlite-deployment.yaml
rg -q 'Runtime must target the real Provider Gateway' infra/generation-reliability/run-real-provider-examples.sh
rg -q 'configure-runtime-provider-gateway.sh' infra/generation-reliability/run-real-provider-examples.sh
rg -q 'runtime-provider-gateway-mode@1' infra/generation-reliability/configure-runtime-provider-gateway.sh
rg -q 'generation-real-provider-stability-audit@1' infra/generation-reliability/audit-real-provider-stability.mjs
rg -q 'audit-real-provider-stability.mjs' infra/generation-reliability/run-real-provider-examples.sh
if rg -q 'fixture-model-gateway' infra/agent-sandbox/runtime/deployment.yaml; then
  printf 'base Runtime deployment must not hard-code the Fixture Gateway\n' >&2
  exit 3
fi
rg -q '^[[:space:]]+type: Recreate$' infra/agent-sandbox/runtime/deployment.yaml || {
  printf 'single-replica Runtime deployment with shared storage must use Recreate strategy\n' >&2
  exit 3
}
rg -q 'fixture-model-gateway.anydesign-runtime.svc.cluster.local:9000' \
  infra/agent-sandbox/runtime/fixture-gateway-env-patch.yaml
rg -q 'fixture-gateway-env-patch.yaml' infra/agent-sandbox/run-runtime-rc-gate.sh
rg -q 'every executed run must include model.execution evidence' infra/generation-reliability/run-real-provider-examples.mjs
rg -q 'generation-real-provider-suite-evidence@2' infra/generation-reliability/run-real-provider-examples.mjs
rg -q 'response.body.getReader' infra/generation-reliability/run-real-provider-examples.mjs
rg -q 'assertRunReservation' infra/generation-reliability/run-real-provider-examples.mjs
rg -q 'ENABLE_SCHEDULED_REAL_PROVIDER' .github/workflows/generation-reliability.yml
rg -q 'RUNTIME_RC_REAL_TOTAL_TOKEN_CEILING' .github/workflows/generation-reliability.yml
rg -q 'production_retires_manual_candidate_reporting_before_it_can_promote' .github/workflows/generation-reliability.yml
rg -q 'preview_publish_rejects_unchanged_source_after_failed_generation_validation' .github/workflows/generation-reliability.yml
rg -q 'preview_publish_rejects_candidate_that_fails_frozen_brief_acceptance' .github/workflows/generation-reliability.yml
rg -q 'persisted_runs_are_hydrated_once_per_store_instance' .github/workflows/generation-reliability.yml
rg -q 'truncated_run_wal_tail_is_repaired_before_restart_continues_writing' .github/workflows/generation-reliability.yml
rg -q 'preview_publish_rejects_unchanged_source_after_failed_generation_validation' infra/agent-sandbox/run-runtime-recovery-gate.sh
rg -q 'persisted_runs_are_hydrated_once_per_store_instance' infra/agent-sandbox/run-runtime-recovery-gate.sh
rg -q 'truncated_run_wal_tail_is_repaired_before_restart_continues_writing' infra/agent-sandbox/run-runtime-recovery-gate.sh
rg -q 'reserve_real_provider_run' infra/agent-sandbox/run-runtime-rc-gate.sh
rg -q 'call preview.publish to build and validate the candidate' infra/agent-sandbox/run-runtime-rc-gate.sh
if rg -q 'report the candidate|promote the candidate' infra/agent-sandbox/run-runtime-rc-gate.sh; then
  printf 'Runtime RC prompt still references a retired manual Candidate workflow\n' >&2
  exit 3
fi

GENERATION_MATRIX_DRY_RUN=1 \
  GENERATION_MATRIX_MODE=fixture \
  GENERATION_MATRIX_BOOTSTRAP=auto \
  bash infra/generation-reliability/run-k3d-matrix.sh \
  | node -e '
let input="";
process.stdin.on("data", chunk => input += chunk).on("end", () => {
  const plan=JSON.parse(input);
  if(plan.schemaVersion!=="generation-matrix-plan@1"
    || plan.mode!=="fixture"
    || plan.bootstrap!=="auto"
    || plan.runtimePort!=="auto") process.exit(2);
});
'

rg -q 'runtime-rc-preflight-skipped@1' infra/agent-sandbox/run-runtime-rc-gate.sh
rg -q 'evidence.auditPassed = auditPassed' services/runtime/scripts/aggregate-release-evidence.mjs
rg -q 'unexpectedClaimNames' infra/agent-sandbox/run-runtime-recovery-gate.sh
rg -q '/usr/local/bin/node /usr/local/lib/anydesign/check-browser-fonts.mjs' services/runtime/Dockerfile

for required in \
  RUNTIME_AGENT_MAX_TURNS \
  RUNTIME_AGENT_MAX_TOOL_CALLS \
  RUNTIME_AGENT_MAX_INPUT_TOKENS \
  RUNTIME_AGENT_MAX_OUTPUT_TOKENS \
  RUNTIME_AGENT_MAX_CONSECUTIVE_PROTOCOL_ERRORS \
  RUNTIME_AGENT_TOTAL_TIMEOUT_SECONDS \
  RUNTIME_AGENT_IDLE_TIMEOUT_SECONDS \
  RUNTIME_AGENT_MAX_NO_PROGRESS_TURNS \
  RUNTIME_AGENT_MAX_READ_TOOL_CALLS \
  RUNTIME_AGENT_MAX_SEARCH_TOOL_CALLS \
  RUNTIME_AGENT_MAX_REPAIR_READ_TOOL_CALLS \
  RUNTIME_AGENT_MAX_REPAIR_SEARCH_TOOL_CALLS \
  RUNTIME_TOOL_CALL_DEADLINE_MS \
  RUNTIME_BUILD_TOOL_CALL_DEADLINE_MS \
  RUNTIME_MAX_ACCEPTANCE_REPAIR_CYCLES; do
  rg -q "name: ${required}" infra/agent-sandbox/runtime/deployment.yaml || {
    printf 'missing Runtime budget from deployment: %s\n' "${required}" >&2
    exit 3
  }
done

printf 'Generation reliability gate contract passed\n'
