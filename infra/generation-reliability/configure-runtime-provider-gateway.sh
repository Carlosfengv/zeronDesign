#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"

context="${RUNTIME_PROVIDER_GATEWAY_CONTEXT:-k3d-${GENERATION_REAL_CLUSTER:-zerondesign-e2e}}"
namespace="${RUNTIME_PROVIDER_GATEWAY_NAMESPACE:-anydesign-runtime}"
deployment="${RUNTIME_PROVIDER_GATEWAY_DEPLOYMENT:-anydesign-runtime}"
expected_url="${RUNTIME_PROVIDER_GATEWAY_URL:-http://provider-gateway.provider-system.svc.cluster.local:9000}"
patch_file="${RUNTIME_PROVIDER_GATEWAY_PATCH_FILE:-${ROOT_DIR}/infra/agent-sandbox/runtime/provider-gateway-env-patch.yaml}"
evidence_file="${RUNTIME_PROVIDER_GATEWAY_EVIDENCE_FILE:-${ROOT_DIR}/services/runtime/target/e2e-evidence/${GENERATION_REAL_CLUSTER:-zerondesign-e2e}/real-provider-runs/runtime-provider-gateway-mode.json}"

command -v "${KUBECTL}" >/dev/null || {
  printf 'runtime_provider_gateway.missing_command: %s\n' "${KUBECTL}" >&2
  exit 2
}
[[ -s "${patch_file}" ]] || {
  printf 'Runtime Provider Gateway patch does not exist: %s\n' "${patch_file}" >&2
  exit 2
}

mkdir -p "$(dirname "${evidence_file}")"
previous_deployment="$(${KUBECTL} --context "${context}" -n "${namespace}" \
  get deployment "${deployment}" -o json)"

"${KUBECTL}" --context "${context}" -n provider-system \
  get service provider-gateway >/dev/null
"${KUBECTL}" --context "${context}" -n provider-system \
  rollout status deployment/provider-gateway --timeout=180s >/dev/null
"${KUBECTL}" --context "${context}" -n "${namespace}" \
  get secret provider-gateway-runtime-auth >/dev/null

"${KUBECTL}" --context "${context}" -n "${namespace}" \
  patch deployment "${deployment}" --type=strategic \
  --patch-file "${patch_file}" >/dev/null
"${KUBECTL}" --context "${context}" -n "${namespace}" \
  rollout status deployment/"${deployment}" --timeout=300s >/dev/null

current_deployment="$(${KUBECTL} --context "${context}" -n "${namespace}" \
  get deployment "${deployment}" -o json)"
ready_pods="$(${KUBECTL} --context "${context}" -n "${namespace}" \
  get pods -l app=anydesign-runtime -o json)"

node - "${previous_deployment}" "${current_deployment}" "${ready_pods}" \
  "${expected_url}" "${context}" "${namespace}" "${deployment}" \
  "${evidence_file}" <<'NODE'
const fs = require("node:fs");
const [
  previousRaw,
  currentRaw,
  podsRaw,
  expectedUrl,
  context,
  namespace,
  deploymentName,
  evidenceFile,
] = process.argv.slice(2);

const previous = JSON.parse(previousRaw);
const current = JSON.parse(currentRaw);
const pods = JSON.parse(podsRaw).items.filter(
  (pod) =>
    !pod.metadata?.deletionTimestamp &&
    pod.status?.phase === "Running" &&
    pod.status?.conditions?.some(
      (condition) => condition.type === "Ready" && condition.status === "True",
    ),
);
if (pods.length !== 1) {
  throw new Error(`Runtime Provider Gateway switch requires exactly one Ready Pod; actual=${pods.length}`);
}

function runtimeEnv(resource) {
  const container = resource.spec?.template?.spec?.containers?.find(
    (item) => item.name === "runtime",
  );
  if (!container) throw new Error("Runtime deployment is missing the runtime container");
  return new Map((container.env || []).map((entry) => [entry.name, entry]));
}

const previousEnv = runtimeEnv(previous);
const currentEnv = runtimeEnv(current);
const provider = currentEnv.get("MODEL_PROVIDER")?.value || null;
const gatewayUrl = currentEnv.get("MODEL_GATEWAY_URL")?.value || null;
const authRef = currentEnv.get("MODEL_GATEWAY_AUTH_TOKEN")?.valueFrom?.secretKeyRef || null;
if (provider !== "internal_gateway") {
  throw new Error(`Runtime Provider Gateway switch has wrong MODEL_PROVIDER: ${provider}`);
}
if ((gatewayUrl || "").replace(/\/$/, "") !== expectedUrl.replace(/\/$/, "")) {
  throw new Error(
    `Runtime Provider Gateway switch has wrong MODEL_GATEWAY_URL: expected=${expectedUrl} actual=${gatewayUrl}`,
  );
}
if (
  authRef?.name !== "provider-gateway-runtime-auth" ||
  authRef?.key !== "MODEL_GATEWAY_AUTH_TOKEN"
) {
  throw new Error("Runtime Provider Gateway switch is missing the governed auth Secret reference");
}

const pod = pods[0];
const evidence = {
  schemaVersion: "runtime-provider-gateway-mode@1",
  recordedAt: new Date().toISOString(),
  context,
  namespace,
  deployment: deploymentName,
  previous: {
    modelProvider: previousEnv.get("MODEL_PROVIDER")?.value || null,
    gatewayUrl: previousEnv.get("MODEL_GATEWAY_URL")?.value || null,
    generation: previous.metadata?.generation || null,
  },
  expected: {
    modelProvider: "internal_gateway",
    gatewayUrl: expectedUrl,
    authSecretName: "provider-gateway-runtime-auth",
  },
  actual: {
    modelProvider: provider,
    gatewayUrl,
    authSecretName: authRef.name,
    deploymentGeneration: current.metadata?.generation || null,
    observedGeneration: current.status?.observedGeneration || null,
    readyPodName: pod.metadata?.name || null,
    readyPodUid: pod.metadata?.uid || null,
  },
  verified: true,
};
fs.writeFileSync(evidenceFile, `${JSON.stringify(evidence, null, 2)}\n`);
NODE

printf 'Runtime Provider Gateway mode verified: %s evidence=%s\n' \
  "${expected_url}" "${evidence_file}"
