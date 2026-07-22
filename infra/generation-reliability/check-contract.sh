#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

bash -n infra/generation-reliability/run-k3d-matrix.sh
bash -n infra/generation-reliability/run-real-provider-examples.sh
rg -Fq '.spec.template.spec.volumes[?(@.name=="public-principal")].secret.secretName' \
  infra/generation-reliability/run-real-provider-examples.sh
if rg -q 'get secret anydesign-runtime-public-principal' \
  infra/generation-reliability/run-real-provider-examples.sh; then
  printf 'real-provider runner must resolve the target Deployment Principal SecretRef\n' >&2
  exit 3
fi
bash -n infra/generation-reliability/configure-runtime-provider-gateway.sh
bash -n infra/generation-reliability/configure-runtime-provider-gateway.test.sh
bash -n infra/generation-reliability/deploy-generation-context-cohort-runtimes.sh
bash -n infra/generation-reliability/prepare-generation-context-cohort-session.sh
bash -n infra/generation-reliability/run-generation-context-paired-pair.sh
bash -n infra/generation-reliability/run-generation-context-cold-dev-pair.sh
bash -n infra/generation-reliability/run-generation-context-repair-pair.sh
bash -n infra/generation-reliability/audit-content-plan-approval-migration.sh
bash -n infra/generation-reliability/verify-content-plan-approval-readiness.sh
bash -n infra/generation-reliability/test-fixtures/fake-kubectl-runtime-provider-gateway.sh
bash -n infra/provider-gateway/reconcile-k3d-model-resources.sh
bash -n infra/provider-gateway/apply-k3d-persistent-sqlite.sh
bash -n infra/agent-sandbox/run-runtime-rc-gate.sh
bash -n infra/agent-sandbox/run-runtime-recovery-gate.sh
node --check infra/generation-reliability/summarize-matrix-evidence.mjs
node --check infra/generation-reliability/run-real-provider-examples.mjs
node --check infra/generation-reliability/run-real-provider-edit.mjs
node --check infra/agent-sandbox/base/workspace-channel-server.js
node --check infra/generation-reliability/run-real-provider-examples.test.mjs
node --check infra/generation-reliability/audit-real-provider-stability.mjs
node --check infra/generation-reliability/audit-real-provider-stability.test.mjs
node --check infra/generation-reliability/verify-generation-context-monitoring.mjs
node --check infra/generation-reliability/verify-generation-context-monitoring.test.mjs
node --check infra/agent-sandbox/verify-runtime-version.mjs
node --check infra/agent-sandbox/verify-runtime-version.test.mjs
node --check services/runtime/scripts/aggregate-release-evidence.mjs
node --check services/runtime/scripts/create-generation-context-paired-sample.mjs
node --check services/runtime/scripts/collect-generation-context-paired-sample.mjs
node --check services/runtime/scripts/generation-context-paired-cohort-ledger.mjs
node --check services/runtime/scripts/audit-content-plan-approval-migration.mjs
node --check services/runtime/scripts/test-audit-content-plan-approval-migration.mjs
node --check services/runtime/scripts/test-run-generation-context-paired-pair.mjs
node --check services/runtime/scripts/generation-context-runtime-restart-evidence.mjs
node --check services/runtime/scripts/test-generation-context-runtime-restart-evidence.mjs
node --check infra/generation-reliability/probe-generation-context-runtime-restart.mjs
bash -n infra/generation-reliability/verify-generation-context-runtime-restart.sh
bash -n infra/generation-reliability/run-generation-context-runtime-restart-pair.sh
node --check services/runtime/scripts/check-browser-fonts.mjs
node infra/agent-sandbox/runtime/fixture-model-gateway.test.cjs
node infra/agent-sandbox/verify-runtime-version.test.mjs
node infra/generation-reliability/run-real-provider-examples.test.mjs
node infra/generation-reliability/audit-real-provider-stability.test.mjs
node infra/generation-reliability/verify-generation-context-monitoring.test.mjs
node services/runtime/scripts/test-create-generation-context-paired-sample.mjs
node services/runtime/scripts/test-collect-generation-context-paired-sample.mjs
node services/runtime/scripts/test-generation-context-paired-cohort-ledger.mjs
node services/runtime/scripts/test-audit-content-plan-approval-migration.mjs
node services/runtime/scripts/test-run-generation-context-paired-pair.mjs
node services/runtime/scripts/test-generation-context-runtime-restart-evidence.mjs
node services/runtime/scripts/test-generation-context-rollout.mjs
kubectl kustomize --load-restrictor=LoadRestrictionsNone \
  infra/generation-reliability/cohort/control >/tmp/generation-context-control.yaml
kubectl kustomize --load-restrictor=LoadRestrictionsNone \
  infra/generation-reliability/cohort/candidate >/tmp/generation-context-candidate.yaml
for rendered in /tmp/generation-context-control.yaml /tmp/generation-context-candidate.yaml; do
  rg -q 'MODEL_GATEWAY_URL' "${rendered}"
  rg -q 'provider-gateway-runtime-auth' "${rendered}"
done
rg -q 'name: anydesign-runtime-generation-control' /tmp/generation-context-control.yaml
rg -q 'value: "off"' /tmp/generation-context-control.yaml
rg -q 'name: anydesign-runtime-postgres-generation-control' /tmp/generation-context-control.yaml
rg -q 'value: s3://anydesign-runtime/generation-control' /tmp/generation-context-control.yaml
rg -q 'secretName: anydesign-runtime-public-principal-generation-control' /tmp/generation-context-control.yaml
rg -q 'name: anydesign-runtime-generation-candidate' /tmp/generation-context-candidate.yaml
rg -q 'value: enabled' /tmp/generation-context-candidate.yaml
rg -q 'name: anydesign-runtime-postgres-generation-candidate' /tmp/generation-context-candidate.yaml
rg -q 'value: s3://anydesign-runtime/generation-candidate' /tmp/generation-context-candidate.yaml
rg -q 'secretName: anydesign-runtime-public-principal-generation-candidate' /tmp/generation-context-candidate.yaml
[[ "$(rg -c 'value: shadow' /tmp/generation-context-candidate.yaml)" -ge 1 ]]
bash infra/generation-reliability/configure-runtime-provider-gateway.test.sh
DEEPSEEK_API_KEY=fixture \
  RUNTIME_RC_PROVIDER_MODE=deepseek \
  RUNTIME_RC_TOKEN_BUDGET_SELF_TEST=1 \
  RUNTIME_RC_REAL_TOTAL_TOKEN_CEILING=240000 \
  bash infra/agent-sandbox/run-runtime-rc-gate.sh
if DEEPSEEK_API_KEY=fixture RUNTIME_RC_MODE=release RUNTIME_RC_PROVIDER_MODE=deepseek \
  bash infra/agent-sandbox/run-runtime-rc-gate.sh \
  >/tmp/runtime-rc-direct-release.out 2>/tmp/runtime-rc-direct-release.err; then
  printf 'direct Provider Runtime RC release mode must fail closed\n' >&2
  exit 3
fi
rg -q 'governed Provider Gateway' /tmp/runtime-rc-direct-release.err
if GENERATION_MATRIX_RC_MODE=release GENERATION_MATRIX_MODE=real \
  bash infra/generation-reliability/run-k3d-matrix.sh \
  >/tmp/generation-matrix-direct-release.out 2>/tmp/generation-matrix-direct-release.err; then
  printf 'direct Provider generation matrix release mode must fail closed\n' >&2
  exit 3
fi
rg -q 'governed Provider Gateway' /tmp/generation-matrix-direct-release.err
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
rg -q 'run-real-provider-examples.sh' .github/workflows/generation-reliability.yml
rg -q 'apply-k3d-persistent-sqlite.sh' .github/workflows/generation-reliability.yml
if rg -q 'GENERATION_MATRIX_MODE:[[:space:]]*real' .github/workflows/generation-reliability.yml; then
  printf 'formal CI must not use the legacy direct-Provider real matrix\n' >&2
  exit 3
fi
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
