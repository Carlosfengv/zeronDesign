#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
cluster_name="${GENERATION_REAL_CLUSTER:-zerondesign-e2e}"
context="k3d-${cluster_name}"
namespace="anydesign-runtime"
runtime_deployment="${GENERATION_REAL_RUNTIME_DEPLOYMENT:-anydesign-runtime}"
runtime_service="${GENERATION_REAL_RUNTIME_SERVICE:-${runtime_deployment}}"
runtime_role="${GENERATION_REAL_RUNTIME_ROLE:-primary}"
workspace_namespace="${GENERATION_REAL_WORKSPACE_NAMESPACE:?GENERATION_REAL_WORKSPACE_NAMESPACE must name a managed Workspace namespace}"
cases_file="${GENERATION_REAL_CASES_FILE:-${SCRIPT_DIR}/real-provider-cases.json}"
evidence_dir="${GENERATION_REAL_EVIDENCE_DIR:-${ROOT_DIR}/services/runtime/target/e2e-evidence/${cluster_name}/real-provider-runs}"
runtime_port="${GENERATION_REAL_RUNTIME_PORT:-}"
prepared_session_dir="${GENERATION_REAL_PREPARED_SESSION_DIR:-}"

cd "${ROOT_DIR}"

for command in "${KUBECTL}" jq node curl base64 grep; do
  command -v "${command}" >/dev/null || {
    printf 'generation_real.missing_command: %s\n' "${command}" >&2
    exit 2
  }
done

OPENSSL_BIN="${OPENSSL_BIN:-openssl}"
if ! command -v "${OPENSSL_BIN}" >/dev/null \
  || ! "${OPENSSL_BIN}" list -public-key-algorithms 2>/dev/null | grep -qi ed25519; then
  for candidate in /opt/homebrew/bin/openssl /usr/local/bin/openssl; do
    if [[ -x "${candidate}" ]] \
      && "${candidate}" list -public-key-algorithms 2>/dev/null | grep -qi ed25519; then
      OPENSSL_BIN="${candidate}"
      break
    fi
  done
fi
if ! command -v "${OPENSSL_BIN}" >/dev/null \
  || ! "${OPENSSL_BIN}" list -public-key-algorithms 2>/dev/null | grep -qi ed25519; then
  printf 'generation_real.openssl_ed25519_unavailable: %s\n' "${OPENSSL_BIN}" >&2
  exit 2
fi

[[ -f "${cases_file}" ]] || {
  printf 'real-provider case manifest does not exist: %s\n' "${cases_file}" >&2
  exit 2
}

node - "${cases_file}" <<'NODE'
const fs = require("node:fs");
const manifest = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
if (manifest.schemaVersion !== "generation-real-provider-suite@1") throw new Error("unsupported suite schema");
if (manifest.approval?.required !== false) throw new Error("human approval must be disabled");
if (!Number.isSafeInteger(manifest.budget?.totalTokens) || manifest.budget.totalTokens <= 0 || manifest.budget.totalTokens > 25_000_000) {
  throw new Error("suite budget must be a positive safety ceiling no greater than 25,000,000 tokens");
}
if (manifest.cases?.length !== 5) throw new Error("suite must contain exactly five cases");
if (manifest.budget.maxRunsPerCase !== 2) throw new Error("suite must reserve exactly Brief + Build per case");
const reserved = manifest.cases.length * manifest.budget.maxRunsPerCase *
  (manifest.budget.perRun.maxInputTokens + manifest.budget.perRun.maxOutputTokens);
const perRunSafetyCeiling =
  manifest.budget.perRun.maxInputTokens + manifest.budget.perRun.maxOutputTokens;
if (perRunSafetyCeiling > manifest.budget.totalTokens) {
  throw new Error("per-run safety ceiling exceeds suite budget");
}
process.stdout.write(
  `Validated five generated cases; theoretical maximum=${reserved}, per-run safety ceiling=${perRunSafetyCeiling}, suite actual-use ceiling=${manifest.budget.totalTokens}\n`,
);
NODE

read -r suite_max_turns suite_max_tools suite_max_input suite_max_output < <(
  node -e '
const manifest = JSON.parse(require("node:fs").readFileSync(process.argv[1], "utf8"));
const budget = manifest.budget.perRun;
process.stdout.write(`${budget.maxTurns} ${budget.maxToolCalls} ${budget.maxInputTokens} ${budget.maxOutputTokens}\n`);
' "${cases_file}"
)

"${KUBECTL}" --context "${context}" get deployment "${runtime_deployment}" \
  -n "${namespace}" >/dev/null
principal_secret="$("${KUBECTL}" --context "${context}" -n "${namespace}" \
  get deployment "${runtime_deployment}" \
  -o jsonpath='{.spec.template.spec.volumes[?(@.name=="public-principal")].secret.secretName}')"
[[ "${principal_secret}" =~ ^[a-z0-9]([-a-z0-9]*[a-z0-9])?$ ]] || {
  printf 'Runtime public Principal SecretRef is missing or invalid: deployment=%s\n' \
    "${runtime_deployment}" >&2
  exit 2
}
"${KUBECTL}" --context "${context}" rollout status deployment/"${runtime_deployment}" \
  -n "${namespace}" --timeout=180s >/dev/null
"${KUBECTL}" --context "${context}" rollout status deployment/provider-gateway \
  -n provider-system --timeout=180s >/dev/null
workspace_label="$("${KUBECTL}" --context "${context}" get namespace "${workspace_namespace}" \
  -o jsonpath='{.metadata.labels.zerondesign\.dev/workspace}')"
[[ "${workspace_label}" == "true" ]] || {
  printf 'GENERATION_REAL_WORKSPACE_NAMESPACE is not a managed Workspace: %s\n' \
    "${workspace_namespace}" >&2
  exit 2
}
export GENERATION_REAL_WORKSPACE_NAMESPACE="${workspace_namespace}"

mkdir -p "${evidence_dir}"
if [[ -n "${prepared_session_dir}" ]]; then
  [[ -s "${prepared_session_dir}/session-meta.json" ]] || {
    printf 'prepared cohort session metadata is missing: %s\n' "${prepared_session_dir}" >&2
    exit 2
  }
  node - "${prepared_session_dir}/session-meta.json" "${context}" "${workspace_namespace}" \
    "${runtime_deployment}" "$(${KUBECTL} --context "${context}" -n "${namespace}" get deployment "${runtime_deployment}" -o json)" <<'NODE'
const fs = require("node:fs");
const [metaFile, context, workspaceNamespace, deploymentName, deploymentRaw] = process.argv.slice(2);
const meta = JSON.parse(fs.readFileSync(metaFile, "utf8"));
const deployment = JSON.parse(deploymentRaw);
const frozen = meta.deployments?.find(item => item.deployment === deploymentName);
if (meta.context !== context || meta.workspaceNamespace !== workspaceNamespace || !frozen) {
  throw new Error("prepared cohort session target identity mismatch");
}
if (deployment.metadata?.uid !== frozen.uid || deployment.metadata?.generation !== frozen.generation) {
  throw new Error("prepared cohort Runtime deployment revision drift");
}
NODE
  export GENERATION_PROVIDER_CONFIG_DIGEST
  GENERATION_PROVIDER_CONFIG_DIGEST="$(node -e 'process.stdout.write(JSON.parse(require("node:fs").readFileSync(process.argv[1],"utf8")).providerConfigSha256)' "${prepared_session_dir}/session-meta.json")"
  export GENERATION_PROVIDER_CONFIG_REVISION
  GENERATION_PROVIDER_CONFIG_REVISION="$(node -e 'process.stdout.write(String(JSON.parse(require("node:fs").readFileSync(process.argv[1],"utf8")).providerResourceRevision))' "${prepared_session_dir}/session-meta.json")"
else
  RUNTIME_PROVIDER_GATEWAY_CONTEXT="${context}" \
    RUNTIME_PROVIDER_GATEWAY_DEPLOYMENT="${runtime_deployment}" \
    RUNTIME_PROVIDER_GATEWAY_POD_SELECTOR="app=anydesign-runtime,anydesign.io/runtime-role=${runtime_role}" \
    RUNTIME_PROVIDER_GATEWAY_EVIDENCE_FILE="${evidence_dir}/runtime-provider-gateway-mode.json" \
    bash infra/generation-reliability/configure-runtime-provider-gateway.sh
  provider_reconcile_evidence="${evidence_dir}/provider-resource-reconcile.json"
  PROVIDER_GATEWAY_CONTEXT="${context}" \
    PROVIDER_GATEWAY_RUN_READINESS_PROBE="${GENERATION_REAL_PROVIDER_READINESS_PROBE:-1}" \
    PROVIDER_GATEWAY_RECONCILE_EVIDENCE_FILE="${provider_reconcile_evidence}" \
    bash infra/provider-gateway/reconcile-k3d-model-resources.sh
  export GENERATION_PROVIDER_CONFIG_DIGEST
  GENERATION_PROVIDER_CONFIG_DIGEST="$(node -e 'process.stdout.write(JSON.parse(require("node:fs").readFileSync(process.argv[1],"utf8")).source.sha256)' "${provider_reconcile_evidence}")"
  export GENERATION_PROVIDER_CONFIG_REVISION
  GENERATION_PROVIDER_CONFIG_REVISION="$(node -e 'process.stdout.write(String(JSON.parse(require("node:fs").readFileSync(process.argv[1],"utf8")).currentResource.revision))' "${provider_reconcile_evidence}")"
fi
export GENERATION_SOURCE_COMMIT
GENERATION_SOURCE_COMMIT="$(git rev-parse HEAD 2>/dev/null || true)"
export GENERATION_SOURCE_DIRTY=false
if [[ -n "$(git status --porcelain 2>/dev/null || true)" ]]; then
  GENERATION_SOURCE_DIRTY=true
fi

provider_config="$("${KUBECTL}" --context "${context}" -n provider-system \
  get configmap provider-gateway-model-resources \
  -o jsonpath='{.data.model-resources\.json}')"
provider_resource_id="$(node -e '
const manifest = JSON.parse(require("node:fs").readFileSync(process.argv[1], "utf8"));
const id = manifest.provider?.modelResourceId;
if (!/^[a-z0-9][a-z0-9._-]{0,127}$/.test(id || "")) throw new Error("invalid Provider Resource id");
process.stdout.write(id);
' "${cases_file}")"
node - "${provider_config}" "${cases_file}" "${provider_resource_id}" <<'NODE'
const fs = require("node:fs");
const config = JSON.parse(process.argv[2]);
const manifest = JSON.parse(fs.readFileSync(process.argv[3], "utf8"));
const providerResourceId = process.argv[4];
const resource = config.resources?.find(item => item.id === providerResourceId);
if (!resource?.enabled || resource.kind !== "openai_compatible" || !resource.auth?.secretRef) {
  throw new Error(`${providerResourceId} is not an enabled credential-backed Provider resource`);
}
const policy = config.policies?.find(item =>
  item.candidates?.some(candidate => candidate.modelResourceId === providerResourceId));
if (!policy || policy.limits?.dailyInputTokens < manifest.budget.totalTokens) {
  throw new Error(`${providerResourceId} daily input-token policy is below the suite safety ceiling`);
}
process.stdout.write(
  `Validated governed ${providerResourceId} Provider resource; suite safety ceiling=${manifest.budget.totalTokens}, provider daily input ceiling=${policy.limits.dailyInputTokens}\n`,
);
NODE

runtime_provider="$("${KUBECTL}" --context "${context}" -n "${namespace}" \
  get deployment "${runtime_deployment}" \
  -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="MODEL_PROVIDER")].value}')"
[[ "${runtime_provider}" == "internal_gateway" ]] || {
  printf 'Runtime must use MODEL_PROVIDER=internal_gateway; actual=%s\n' "${runtime_provider}" >&2
  exit 2
}

expected_gateway_url="${GENERATION_REAL_PROVIDER_GATEWAY_URL:-http://provider-gateway.provider-system.svc.cluster.local:9000}"
runtime_gateway_url="$("${KUBECTL}" --context "${context}" -n "${namespace}" \
  get deployment "${runtime_deployment}" \
  -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="MODEL_GATEWAY_URL")].value}')"
[[ "${runtime_gateway_url%/}" == "${expected_gateway_url%/}" ]] || {
  printf 'Runtime must target the real Provider Gateway; expected=%s actual=%s\n' \
    "${expected_gateway_url}" "${runtime_gateway_url}" >&2
  exit 2
}

if [[ -z "${runtime_port}" ]]; then
  runtime_port="$(node -e '
const net = require("node:net");
const server = net.createServer();
server.unref();
server.listen(0, "127.0.0.1", () => {
  process.stdout.write(String(server.address().port));
  server.close();
});
')"
fi

work_dir="$(mktemp -d)"
principal_private_key="${prepared_session_dir:+${prepared_session_dir}/.credentials/principal-private.pem}"
principal_private_key="${principal_private_key:-${work_dir}/principal-private.pem}"
principal_public_key="${work_dir}/principal-public.der"
principal_secret_backup="${work_dir}/principal-secret.yaml"
admin_token_file="${prepared_session_dir:+${prepared_session_dir}/.credentials/runtime-admin-token}"
admin_token_file="${admin_token_file:-${work_dir}/runtime-admin-token}"
port_forward_log="${work_dir}/port-forward.log"
port_forward_pid=""
runtime_mutated=false

old_input_budget=""
old_output_budget=""
old_turn_budget=""
old_tool_budget=""
if [[ -z "${prepared_session_dir}" ]]; then
  old_input_budget="$("${KUBECTL}" --context "${context}" -n "${namespace}" get deployment "${runtime_deployment}" -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="RUNTIME_AGENT_MAX_INPUT_TOKENS")].value}')"
  old_output_budget="$("${KUBECTL}" --context "${context}" -n "${namespace}" get deployment "${runtime_deployment}" -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="RUNTIME_AGENT_MAX_OUTPUT_TOKENS")].value}')"
  old_turn_budget="$("${KUBECTL}" --context "${context}" -n "${namespace}" get deployment "${runtime_deployment}" -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="RUNTIME_AGENT_MAX_TURNS")].value}')"
  old_tool_budget="$("${KUBECTL}" --context "${context}" -n "${namespace}" get deployment "${runtime_deployment}" -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="RUNTIME_AGENT_MAX_TOOL_CALLS")].value}')"
fi

cleanup() {
  local status=$?
  if [[ -n "${port_forward_pid}" ]]; then
    kill "${port_forward_pid}" >/dev/null 2>&1 || true
    wait "${port_forward_pid}" >/dev/null 2>&1 || true
  fi
  if [[ "${runtime_mutated}" == "true" && -z "${prepared_session_dir}" ]]; then
    "${KUBECTL}" --context "${context}" apply -f "${principal_secret_backup}" >/dev/null 2>&1 || true
    "${KUBECTL}" --context "${context}" set env deployment/"${runtime_deployment}" \
      -n "${namespace}" \
      "RUNTIME_AGENT_MAX_TURNS=${old_turn_budget}" \
      "RUNTIME_AGENT_MAX_TOOL_CALLS=${old_tool_budget}" \
      "RUNTIME_AGENT_MAX_INPUT_TOKENS=${old_input_budget}" \
      "RUNTIME_AGENT_MAX_OUTPUT_TOKENS=${old_output_budget}" >/dev/null 2>&1 || true
    "${KUBECTL}" --context "${context}" rollout status deployment/"${runtime_deployment}" \
      -n "${namespace}" --timeout=180s >/dev/null 2>&1 || true
  fi
  rm -rf "${work_dir}"
  exit "${status}"
}
trap cleanup EXIT

if [[ -n "${prepared_session_dir}" ]]; then
  [[ -s "${principal_private_key}" && -s "${admin_token_file}" ]] || {
    printf 'prepared cohort session credentials are missing\n' >&2
    exit 2
  }
  "${OPENSSL_BIN}" pkey -in "${principal_private_key}" -pubout -outform DER -out "${principal_public_key}" 2>/dev/null
  cluster_public_key="$("${KUBECTL}" --context "${context}" get secret "${principal_secret}" -n "${namespace}" -o jsonpath='{.data.public\.der}')"
  local_public_key="$(base64 <"${principal_public_key}" | tr -d '\r\n')"
  [[ "${cluster_public_key}" == "${local_public_key}" ]] || {
    printf 'prepared cohort Principal key drift\n' >&2
    exit 2
  }
else
  "${KUBECTL}" --context "${context}" get secret "${principal_secret}" -n "${namespace}" -o yaml >"${principal_secret_backup}"
  "${KUBECTL}" --context "${context}" get secret anydesign-runtime-internal-admin -n "${namespace}" -o jsonpath='{.data.token}' | base64 --decode >"${admin_token_file}"
  chmod 600 "${admin_token_file}"
  "${OPENSSL_BIN}" genpkey -algorithm ED25519 -out "${principal_private_key}" 2>/dev/null
  "${OPENSSL_BIN}" pkey -in "${principal_private_key}" -pubout -outform DER -out "${principal_public_key}" 2>/dev/null
  [[ -s "${principal_private_key}" && -s "${principal_public_key}" ]] || {
    printf 'generation_real.principal_key_generation_failed\n' >&2
    exit 2
  }
  "${KUBECTL}" --context "${context}" create secret generic "${principal_secret}" -n "${namespace}" --from-file="public.der=${principal_public_key}" --dry-run=client -o yaml | "${KUBECTL}" --context "${context}" apply -f - >/dev/null
  "${KUBECTL}" --context "${context}" set env deployment/"${runtime_deployment}" -n "${namespace}" \
    "RUNTIME_AGENT_MAX_TURNS=${suite_max_turns}" \
    "RUNTIME_AGENT_MAX_TOOL_CALLS=${suite_max_tools}" \
    "RUNTIME_AGENT_MAX_INPUT_TOKENS=${suite_max_input}" \
    "RUNTIME_AGENT_MAX_OUTPUT_TOKENS=${suite_max_output}" >/dev/null
  runtime_mutated=true
  "${KUBECTL}" --context "${context}" rollout status deployment/"${runtime_deployment}" -n "${namespace}" --timeout=300s >/dev/null
fi

"${KUBECTL}" --context "${context}" port-forward -n "${namespace}" \
  service/"${runtime_service}" "${runtime_port}:8080" >"${port_forward_log}" 2>&1 &
port_forward_pid=$!
base_url="http://127.0.0.1:${runtime_port}"
for _ in $(seq 1 60); do
  if ! kill -0 "${port_forward_pid}" >/dev/null 2>&1; then
    sed -n '1,120p' "${port_forward_log}" >&2
    exit 3
  fi
  if curl --fail --silent "${base_url}/health" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
curl --fail --silent "${base_url}/health" >/dev/null

set +e
node infra/generation-reliability/run-real-provider-examples.mjs \
  "${cases_file}" \
  "${base_url}" \
  "${principal_private_key}" \
  "${admin_token_file}" \
  "${evidence_dir}"
suite_status=$?
set -e

summary_file="$(find "${evidence_dir}" -mindepth 2 -maxdepth 2 \
  -name real-provider-examples-summary.json -print | sort | tail -n 1)"
[[ -n "${summary_file}" && -s "${summary_file}" ]] || {
  printf 'real-provider summary was not produced\n' >&2
  exit 8
}
node infra/generation-reliability/audit-real-provider-stability.mjs \
  --evidence-root "${evidence_dir}" \
  --out "${evidence_dir}/real-provider-stability-audit.json" \
  --required-consecutive 3 \
  --allow-incomplete
suite_evidence_dir="$(dirname "${summary_file}")"
if rg -n -i '(bearer[[:space:]]+[a-z0-9._-]{20,}|sk-[a-z0-9_-]{12,}|api[_-]?key["[:space:]]*:[[:space:]]*[a-z0-9._-]{12,})' \
  "${suite_evidence_dir}"; then
  printf 'secret-like value found in real-provider evidence\n' >&2
  exit 7
fi
printf 'Five real Provider examples completed: %s\n' "${summary_file}"
exit "${suite_status}"
