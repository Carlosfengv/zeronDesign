#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
OPENSSL_BIN="${OPENSSL_BIN:-openssl}"
CONTEXT="${GENERATION_COHORT_CONTEXT:-k3d-zerondesign-e2e}"
NAMESPACE="${GENERATION_COHORT_NAMESPACE:-anydesign-runtime}"
WORKSPACE_NAMESPACE="${GENERATION_COHORT_WORKSPACE_NAMESPACE:-ws-runtime-rc}"
CONTROL_DEPLOYMENT="anydesign-runtime-generation-control"
CANDIDATE_DEPLOYMENT="anydesign-runtime-generation-candidate"
CASES_FILE="${GENERATION_COHORT_CASES_FILE:-${ROOT_DIR}/infra/generation-reliability/real-provider-cases.json}"
PROVIDER_RESOURCE_ID="${GENERATION_COHORT_PROVIDER_RESOURCE_ID:-deepseek-v4-pro}"
PROVIDER_POLICY_ID="${GENERATION_COHORT_PROVIDER_POLICY_ID:-local-deepseek-v4-pro-default}"
PROVIDER_CONFIG_FILE="${GENERATION_COHORT_PROVIDER_CONFIG:-${ROOT_DIR}/infra/provider-gateway/model-resources.deepseek-v4-pro.json}"
SESSION_ID="${GENERATION_COHORT_SESSION_ID:-${PROVIDER_RESOURCE_ID}-$(date -u +%Y%m%dT%H%M%SZ)}"
SESSION_DIR="${GENERATION_COHORT_SESSION_DIR:-${ROOT_DIR}/services/runtime/target/e2e-evidence/generation-context-cohort/${SESSION_ID}}"

for command in "${KUBECTL}" "${OPENSSL_BIN}" node base64; do
  command -v "${command}" >/dev/null || {
    printf 'generation_cohort_session.missing_command: %s\n' "${command}" >&2
    exit 2
  }
done
if ! "${OPENSSL_BIN}" list -public-key-algorithms 2>/dev/null | grep -qi ed25519; then
  for candidate in /opt/homebrew/bin/openssl /usr/local/bin/openssl; do
    if [[ -x "${candidate}" ]] && "${candidate}" list -public-key-algorithms 2>/dev/null | grep -qi ed25519; then
      OPENSSL_BIN="${candidate}"
      break
    fi
  done
fi
if ! "${OPENSSL_BIN}" list -public-key-algorithms 2>/dev/null | grep -qi ed25519; then
  printf 'generation_cohort_session.openssl_ed25519_unavailable: %s\n' "${OPENSSL_BIN}" >&2
  exit 2
fi
[[ -s "${CASES_FILE}" ]] || {
  printf 'generation_cohort_session.cases_missing: %s\n' "${CASES_FILE}" >&2
  exit 2
}
[[ -s "${PROVIDER_CONFIG_FILE}" ]] || {
  printf 'generation_cohort_session.provider_config_missing: %s\n' "${PROVIDER_CONFIG_FILE}" >&2
  exit 2
}
[[ "${PROVIDER_RESOURCE_ID}" =~ ^[a-z0-9][a-z0-9._-]{0,127}$ ]] || {
  printf 'generation_cohort_session.invalid_provider_resource_id: %s\n' "${PROVIDER_RESOURCE_ID}" >&2
  exit 2
}
[[ ! -e "${SESSION_DIR}" ]] || {
  printf 'generation_cohort_session.already_exists: %s\n' "${SESSION_DIR}" >&2
  exit 2
}

work_dir="$(mktemp -d)"
cleanup() {
  find "${work_dir}" -type f -delete
  rmdir "${work_dir}"
}
trap cleanup EXIT
mkdir -p "${SESSION_DIR}/.credentials"
chmod 700 "${SESSION_DIR}" "${SESSION_DIR}/.credentials"

provider_evidence="${SESSION_DIR}/provider-resource-reconcile.json"
PROVIDER_GATEWAY_CONTEXT="${CONTEXT}" \
  PROVIDER_GATEWAY_RESOURCE_ID="${PROVIDER_RESOURCE_ID}" \
  PROVIDER_GATEWAY_POLICY_ID="${PROVIDER_POLICY_ID}" \
  PROVIDER_GATEWAY_RESOURCE_CONFIG="${PROVIDER_CONFIG_FILE}" \
  PROVIDER_GATEWAY_RUN_READINESS_PROBE=1 \
  PROVIDER_GATEWAY_RECONCILE_EVIDENCE_FILE="${provider_evidence}" \
  bash "${ROOT_DIR}/infra/provider-gateway/reconcile-k3d-model-resources.sh"

for target in \
  "${CONTROL_DEPLOYMENT}:generation-control" \
  "${CANDIDATE_DEPLOYMENT}:generation-candidate"; do
  deployment="${target%%:*}"
  role="${target##*:}"
  RUNTIME_PROVIDER_GATEWAY_CONTEXT="${CONTEXT}" \
    RUNTIME_PROVIDER_GATEWAY_DEPLOYMENT="${deployment}" \
    RUNTIME_PROVIDER_GATEWAY_POD_SELECTOR="app=anydesign-runtime,anydesign.io/runtime-role=${role}" \
    RUNTIME_PROVIDER_GATEWAY_EVIDENCE_FILE="${SESSION_DIR}/runtime-provider-gateway-${role}.json" \
    bash "${ROOT_DIR}/infra/generation-reliability/configure-runtime-provider-gateway.sh"
done

principal_private_key="${SESSION_DIR}/.credentials/principal-private.pem"
principal_public_key="${work_dir}/principal-public.der"
admin_token_file="${SESSION_DIR}/.credentials/runtime-admin-token"
"${OPENSSL_BIN}" genpkey -algorithm ED25519 -out "${principal_private_key}" 2>/dev/null
"${OPENSSL_BIN}" pkey -in "${principal_private_key}" -pubout -outform DER -out "${principal_public_key}" 2>/dev/null
"${KUBECTL}" --context "${CONTEXT}" -n "${NAMESPACE}" get secret anydesign-runtime-internal-admin \
  -o jsonpath='{.data.token}' | base64 --decode >"${admin_token_file}"
chmod 600 "${principal_private_key}" "${admin_token_file}"

for principal_secret in \
  anydesign-runtime-public-principal-generation-control \
  anydesign-runtime-public-principal-generation-candidate; do
  "${KUBECTL}" --context "${CONTEXT}" create secret generic "${principal_secret}" \
    -n "${NAMESPACE}" --from-file="public.der=${principal_public_key}" --dry-run=client -o yaml \
    | "${KUBECTL}" --context "${CONTEXT}" apply -f - >/dev/null
done

read -r max_turns max_tools max_input max_output < <(
  node - "${CASES_FILE}" <<'NODE'
const fs = require("node:fs");
const manifest = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const budget = manifest.budget?.perRun;
for (const field of ["maxTurns", "maxToolCalls", "maxInputTokens", "maxOutputTokens"]) {
  if (!Number.isSafeInteger(budget?.[field]) || budget[field] <= 0) throw new Error(`invalid ${field}`);
}
process.stdout.write(`${budget.maxTurns} ${budget.maxToolCalls} ${budget.maxInputTokens} ${budget.maxOutputTokens}\n`);
NODE
)

for deployment in "${CONTROL_DEPLOYMENT}" "${CANDIDATE_DEPLOYMENT}"; do
  "${KUBECTL}" --context "${CONTEXT}" set env deployment/"${deployment}" -n "${NAMESPACE}" \
    "RUNTIME_AGENT_MAX_TURNS=${max_turns}" \
    "RUNTIME_AGENT_MAX_TOOL_CALLS=${max_tools}" \
    "RUNTIME_AGENT_MAX_INPUT_TOKENS=${max_input}" \
    "RUNTIME_AGENT_MAX_OUTPUT_TOKENS=${max_output}" >/dev/null
  "${KUBECTL}" --context "${CONTEXT}" rollout restart deployment/"${deployment}" \
    -n "${NAMESPACE}" >/dev/null
done
"${KUBECTL}" --context "${CONTEXT}" rollout status deployment/"${CONTROL_DEPLOYMENT}" -n "${NAMESPACE}" --timeout=300s >/dev/null
"${KUBECTL}" --context "${CONTEXT}" rollout status deployment/"${CANDIDATE_DEPLOYMENT}" -n "${NAMESPACE}" --timeout=300s >/dev/null

"${KUBECTL}" --context "${CONTEXT}" -n "${NAMESPACE}" get deployment "${CONTROL_DEPLOYMENT}" -o json >"${work_dir}/control.json"
"${KUBECTL}" --context "${CONTEXT}" -n "${NAMESPACE}" get deployment "${CANDIDATE_DEPLOYMENT}" -o json >"${work_dir}/candidate.json"

node - "${SESSION_DIR}" "${SESSION_ID}" "${CONTEXT}" "${WORKSPACE_NAMESPACE}" \
  "${CASES_FILE}" "${provider_evidence}" "${work_dir}/control.json" "${work_dir}/candidate.json" \
  "${PROVIDER_RESOURCE_ID}" <<'NODE'
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const [sessionDir, sessionId, context, workspaceNamespace, casesFile, providerFile, controlFile, candidateFile, expectedProviderResourceId] = process.argv.slice(2);
const sha256 = value => crypto.createHash("sha256").update(value).digest("hex");
const canonical = value => {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;
  return JSON.stringify(value);
};
const providerEvidence = JSON.parse(fs.readFileSync(providerFile, "utf8"));
const resource = providerEvidence.currentResource;
if (
  resource?.id !== expectedProviderResourceId ||
  !Number.isSafeInteger(resource?.revision) ||
  resource.revision < 1 ||
  typeof resource?.physicalModel !== "string" ||
  !resource.physicalModel.trim()
) {
  throw new Error("prepared session Provider Resource identity is invalid or drifted");
}
const cases = JSON.parse(fs.readFileSync(casesFile, "utf8"));
if (cases.provider?.modelResourceId !== resource.id) {
  throw new Error("case manifest Provider Resource does not match the prepared session");
}
const deployments = [
  ["control", "off", JSON.parse(fs.readFileSync(controlFile, "utf8"))],
  ["candidate", "enabled", JSON.parse(fs.readFileSync(candidateFile, "utf8"))],
].map(([side, expectedMode, deployment]) => {
  const container = deployment.spec?.template?.spec?.containers?.find(item => item.name === "runtime");
  const env = Object.fromEntries((container?.env || []).map(item => [item.name, item]));
  if (env.RUNTIME_GENERATION_CONTEXT_MODE?.value !== expectedMode) throw new Error(`${side} mode drift`);
  if (env.MODEL_PROVIDER?.value !== "internal_gateway") throw new Error(`${side} Gateway mode drift`);
  return {
    side,
    deployment: deployment.metadata.name,
    uid: deployment.metadata.uid,
    generation: deployment.metadata.generation,
    image: container.image,
    generationContextMode: expectedMode,
    deploymentRevision: sha256(canonical({
      uid: deployment.metadata.uid,
      generation: deployment.metadata.generation,
      podTemplate: deployment.spec.template,
    })),
  };
});
if (deployments[0].image !== deployments[1].image) throw new Error("control/candidate Runtime image mismatch");
const providerParametersHash = sha256(canonical(resource.defaults || {}));
const session = {
  schemaVersion: "generation-context-paired-cohort-session@1",
  sessionId,
  createdAt: new Date().toISOString(),
  calculatorVersion: "generation-context-rollout-calculator@1",
  bootstrap: { iterations: 2_000, seed: 20260720 },
  sourcePolicy: "hashes_only",
  fixtureManifestSha256: sha256(fs.readFileSync(casesFile)),
  providers: [{
    gatewayMode: "internal_gateway",
    modelResourceId: resource.id,
    resourceRevision: resource.revision,
    modelVersion: resource.physicalModel,
    providerParametersHash,
    visionCapable: resource.capabilities?.vision === true || resource.capabilities?.visionInput === true,
    supportedImageMediaTypes: resource.capabilities?.supportedImageMediaTypes || [],
    maxImageCount: resource.capabilities?.maxImageCount || 0,
  }],
  runtimes: Object.fromEntries(deployments.map(item => [item.side, {
    generationContextMode: item.generationContextMode,
    deploymentRevision: item.deploymentRevision,
    allowedModelResourceIds: [resource.id],
  }])),
};
const meta = {
  schemaVersion: "generation-context-cohort-session-meta@1",
  sessionId,
  context,
  workspaceNamespace,
  providerConfigSha256: providerEvidence.source.sha256,
  providerModelResourceId: resource.id,
  providerResourceRevision: resource.revision,
  deployments,
};
fs.writeFileSync(path.join(sessionDir, "session.json"), `${JSON.stringify(session, null, 2)}\n`, { flag: "wx", mode: 0o600 });
fs.writeFileSync(path.join(sessionDir, "session-meta.json"), `${JSON.stringify(meta, null, 2)}\n`, { flag: "wx", mode: 0o600 });
NODE

node "${ROOT_DIR}/services/runtime/scripts/generation-context-paired-cohort-ledger.mjs" \
  init "${SESSION_DIR}/cohort.ndjson" "${SESSION_DIR}/session.json"

printf 'Prepared fixed Generation Context cohort session: %s\n' "${SESSION_DIR}"
printf 'Use GENERATION_REAL_PREPARED_SESSION_DIR=%s for both control and candidate collection.\n' "${SESSION_DIR}"
