#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
CONTEXT="${PROVIDER_GATEWAY_CONTEXT:-}"
NAMESPACE="${PROVIDER_GATEWAY_NAMESPACE:-provider-system}"
DEPLOYMENT="${PROVIDER_GATEWAY_DEPLOYMENT:-provider-gateway}"
SERVICE="${PROVIDER_GATEWAY_SERVICE:-provider-gateway}"
RESOURCE_ID="${PROVIDER_GATEWAY_RESOURCE_ID:-deepseek-v4-pro}"
POLICY_ID="${PROVIDER_GATEWAY_POLICY_ID:-local-deepseek-v4-pro-default}"
CONFIG_FILE="${PROVIDER_GATEWAY_RESOURCE_CONFIG:-${ROOT_DIR}/infra/provider-gateway/model-resources.deepseek-v4-pro.json}"
ADMIN_SECRET="${PROVIDER_GATEWAY_ADMIN_SECRET:-provider-gateway-admin-auth}"
RUN_READINESS_PROBE="${PROVIDER_GATEWAY_RUN_READINESS_PROBE:-0}"
EVIDENCE_FILE="${PROVIDER_GATEWAY_RECONCILE_EVIDENCE_FILE:-${ROOT_DIR}/services/runtime/target/e2e-evidence/provider-resource-reconcile.json}"
OPERATOR_ID="${PROVIDER_GATEWAY_OPERATOR_ID:-generation-reliability-gitops}"
CHANGE_REASON="${PROVIDER_GATEWAY_CHANGE_REASON:-reconcile reviewed model resource configuration}"
CHANGE_REFERENCE="${PROVIDER_GATEWAY_CHANGE_REFERENCE:-generation-reliability-m8}"

for command in "${KUBECTL}" node curl base64; do
  command -v "${command}" >/dev/null || {
    printf 'provider_reconcile.missing_command: %s\n' "${command}" >&2
    exit 2
  }
done
[[ -f "${CONFIG_FILE}" ]] || {
  printf 'provider_reconcile.config_missing: %s\n' "${CONFIG_FILE}" >&2
  exit 2
}
case "${RUN_READINESS_PROBE}" in
  0 | 1) ;;
  *)
    printf 'PROVIDER_GATEWAY_RUN_READINESS_PROBE must be 0 or 1\n' >&2
    exit 2
    ;;
esac

kubectl_args=()
if [[ -n "${CONTEXT}" ]]; then
  kubectl_args+=(--context "${CONTEXT}")
fi
kube() {
  "${KUBECTL}" "${kubectl_args[@]}" "$@"
}

read -r desired_revision desired_policy_revision desired_max_attempts desired_daily_tokens < <(
  node - "${CONFIG_FILE}" "${RESOURCE_ID}" "${POLICY_ID}" <<'NODE'
const fs = require("node:fs");
const [file, resourceId, policyId] = process.argv.slice(2);
const config = JSON.parse(fs.readFileSync(file, "utf8"));
const resource = config.resources?.find(item => item.id === resourceId);
const policy = config.policies?.find(item => item.id === policyId);
if (!resource || !policy) throw new Error("authoritative resource or policy is missing");
if (resource.schemaVersion !== "model-resource@1" || resource.kind !== "openai_compatible") {
  throw new Error("authoritative model resource contract is invalid");
}
if (!resource.enabled || resource.physicalModel !== resourceId) {
  throw new Error("authoritative model resource must be enabled and target its declared model");
}
if (!String(resource.auth?.secretRef || "").startsWith("file:/var/run/secrets/")) {
  throw new Error("authoritative configuration must contain only a mounted file secret reference");
}
if (!policy.candidates?.some(candidate => candidate.modelResourceId === resourceId)) {
  throw new Error("authoritative policy does not route to the model resource");
}
if (!policy.directSelection?.allowedModelResourceIds?.includes(resourceId)) {
  throw new Error("authoritative policy does not allow direct model selection");
}
for (const value of [resource.revision, policy.revision, resource.defaults?.maxAttempts, policy.limits?.dailyInputTokens]) {
  if (!Number.isSafeInteger(value) || value < 1) throw new Error("authoritative revisions and limits must be positive integers");
}
process.stdout.write(`${resource.revision} ${policy.revision} ${resource.defaults.maxAttempts} ${policy.limits.dailyInputTokens}\n`);
NODE
)
config_digest="$(node -e 'const fs=require("node:fs"),c=require("node:crypto");process.stdout.write(c.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"))' "${CONFIG_FILE}")"
operation_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"

kube get namespace "${NAMESPACE}" >/dev/null
kube get deployment "${DEPLOYMENT}" -n "${NAMESPACE}" >/dev/null
kube get secret "${ADMIN_SECRET}" -n "${NAMESPACE}" >/dev/null

admin_token_path="$(kube get deployment "${DEPLOYMENT}" -n "${NAMESPACE}" -o json | node -e '
const fs=require("node:fs");
const deployment=JSON.parse(fs.readFileSync(0,"utf8"));
const env=deployment.spec.template.spec.containers[0].env||[];
process.stdout.write(env.find(item=>item.name==="PROVIDER_GATEWAY_ADMIN_TOKEN_FILE")?.value||"");
')"
[[ -n "${admin_token_path}" ]] || {
  printf 'provider_reconcile.admin_token_not_mounted: deployment=%s\n' "${DEPLOYMENT}" >&2
  exit 3
}

kube create configmap provider-gateway-model-resources -n "${NAMESPACE}" \
  --from-file="model-resources.json=${CONFIG_FILE}" \
  --dry-run=client -o yaml | kube apply -f - >/dev/null
kube patch deployment "${DEPLOYMENT}" -n "${NAMESPACE}" --type=merge \
  -p "{\"spec\":{\"template\":{\"metadata\":{\"annotations\":{\"anydesign.io/provider-config-digest\":\"${config_digest}\"}}}}}" \
  >/dev/null
kube rollout status deployment/"${DEPLOYMENT}" -n "${NAMESPACE}" --timeout=300s >/dev/null

work_dir="$(mktemp -d)"
port_forward_pid=""
cleanup() {
  if [[ -n "${port_forward_pid}" ]]; then
    kill "${port_forward_pid}" >/dev/null 2>&1 || true
    wait "${port_forward_pid}" >/dev/null 2>&1 || true
  fi
  rm -rf "${work_dir}"
}
trap cleanup EXIT

admin_token_file="${work_dir}/admin-token"
curl_config="${work_dir}/curl.conf"
port_forward_log="${work_dir}/port-forward.log"
umask 077
kube get secret "${ADMIN_SECRET}" -n "${NAMESPACE}" -o jsonpath='{.data.token}' \
  | base64 --decode >"${admin_token_file}"
[[ -s "${admin_token_file}" ]] || {
  printf 'provider_reconcile.admin_token_empty\n' >&2
  exit 3
}
admin_token="$(tr -d '\r\n' <"${admin_token_file}")"
cat >"${curl_config}" <<EOF
silent
show-error
fail-with-body
header = "Authorization: Bearer ${admin_token}"
header = "x-operator-id: ${OPERATOR_ID}"
header = "x-change-reason: ${CHANGE_REASON}"
header = "x-change-reference: ${CHANGE_REFERENCE}"
EOF
unset admin_token

local_port="$(node -e '
const net=require("node:net");
const server=net.createServer();
server.unref();
server.listen(0,"127.0.0.1",()=>{process.stdout.write(String(server.address().port));server.close();});
')"
kube port-forward -n "${NAMESPACE}" service/"${SERVICE}" "${local_port}:9000" \
  >"${port_forward_log}" 2>&1 &
port_forward_pid=$!
base_url="http://127.0.0.1:${local_port}/internal/provider-gateway/admin/v1"
for _ in $(seq 1 60); do
  if ! kill -0 "${port_forward_pid}" >/dev/null 2>&1; then
    sed -n '1,120p' "${port_forward_log}" >&2
    exit 3
  fi
  if curl --silent --fail "http://127.0.0.1:${local_port}/health/ready" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
curl --silent --fail "http://127.0.0.1:${local_port}/health/ready" >/dev/null

reconcile_json="$(curl --config "${curl_config}" -X POST \
  -H "Idempotency-Key: reconcile-${config_digest:0:20}-${operation_id}" \
  "${base_url}/configuration/reconcile")"
resource_json="$(curl --config "${curl_config}" "${base_url}/model-resources/${RESOURCE_ID}")"
policy_json="$(curl --config "${curl_config}" "${base_url}/model-selection-policies/${POLICY_ID}")"

node - "${resource_json}" "${policy_json}" "${RESOURCE_ID}" "${POLICY_ID}" \
  "${desired_revision}" "${desired_policy_revision}" "${desired_max_attempts}" \
  "${desired_daily_tokens}" <<'NODE'
const [resourceRaw, policyRaw, resourceId, policyId, revision, policyRevision, maxAttempts, dailyTokens] = process.argv.slice(2);
const resource = JSON.parse(resourceRaw);
const policy = JSON.parse(policyRaw);
if (resource.id !== resourceId || resource.revision !== Number(revision) || !resource.enabled) {
  throw new Error(`current model resource does not match desired revision ${resourceId}@${revision}`);
}
if (resource.physicalModel !== resourceId || resource.defaults?.maxAttempts !== Number(maxAttempts)) {
  throw new Error("current model resource behavior differs from the authoritative declaration");
}
if (resource.auth?.secretConfigured !== true) throw new Error("current model resource has no configured secret");
if (policy.id !== policyId || policy.revision !== Number(policyRevision)) {
  throw new Error(`current model selection policy does not match desired revision ${policyId}@${policyRevision}`);
}
if (policy.limits?.dailyInputTokens !== Number(dailyTokens)) {
  throw new Error("current model selection policy token limit differs from the authoritative declaration");
}
NODE

readiness_json="null"
if [[ "${RUN_READINESS_PROBE}" == "1" ]]; then
  readiness_json="$(curl --config "${curl_config}" -X POST \
    -H 'content-type: application/json' \
    -H "Idempotency-Key: readiness-${RESOURCE_ID}-${desired_revision}-${operation_id}" \
    -d "{\"expectedRevision\":${desired_revision}}" \
    "${base_url}/model-resources/${RESOURCE_ID}/readiness")"
  node -e 'const v=JSON.parse(process.argv[1]);if(v.ready!==true||v.revision!==Number(process.argv[2]))process.exit(2)' \
    "${readiness_json}" "${desired_revision}"
fi

mkdir -p "$(dirname "${EVIDENCE_FILE}")"
git_commit="$(git -C "${ROOT_DIR}" rev-parse HEAD 2>/dev/null || true)"
git_dirty=false
if [[ -n "$(git -C "${ROOT_DIR}" status --porcelain 2>/dev/null || true)" ]]; then
  git_dirty=true
fi
node - "${EVIDENCE_FILE}" "${config_digest}" "${CONFIG_FILE}" "${reconcile_json}" \
  "${resource_json}" "${policy_json}" "${readiness_json}" "${git_commit}" "${git_dirty}" <<'NODE'
const fs = require("node:fs");
const [out, digest, configFile, reconcileRaw, resourceRaw, policyRaw, readinessRaw, commit, dirty] = process.argv.slice(2);
const readiness = JSON.parse(readinessRaw);
if (readiness) delete readiness.providerRequestId;
const evidence = {
  schemaVersion: "provider-resource-reconcile-evidence@1",
  recordedAt: new Date().toISOString(),
  source: { path: configFile, sha256: digest, gitCommit: commit || null, gitDirty: dirty === "true" },
  reconcile: JSON.parse(reconcileRaw),
  currentResource: JSON.parse(resourceRaw),
  currentPolicy: JSON.parse(policyRaw),
  readiness,
  passed: true,
};
fs.writeFileSync(out, `${JSON.stringify(evidence, null, 2)}\n`, { mode: 0o600 });
NODE

printf 'Provider resource reconciled and verified: %s@%s digest=%s evidence=%s\n' \
  "${RESOURCE_ID}" "${desired_revision}" "${config_digest}" "${EVIDENCE_FILE}"
