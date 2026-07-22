#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
CONTEXT="${CONTENT_PLAN_APPROVAL_CONTEXT:-k3d-zerondesign-e2e}"
NAMESPACE="${CONTENT_PLAN_APPROVAL_NAMESPACE:-anydesign-runtime}"
DEPLOYMENT="${CONTENT_PLAN_APPROVAL_DEPLOYMENT:-anydesign-runtime}"
SERVICE="${CONTENT_PLAN_APPROVAL_SERVICE:-anydesign-runtime}"
WORKSPACE_NAMESPACE="${CONTENT_PLAN_APPROVAL_WORKSPACE_NAMESPACE:-ws-runtime-rc}"
ADMIN_SECRET="${CONTENT_PLAN_APPROVAL_ADMIN_SECRET:-anydesign-runtime-internal-admin}"
PRINCIPAL_ID="${CONTENT_PLAN_APPROVAL_PRINCIPAL_ID:-wave1-readiness-principal}"
PROJECT_ID="${CONTENT_PLAN_APPROVAL_PROJECT_ID:-wave1-approval-$(date -u +%Y%m%d%H%M%S)}"
PLAN_ID="${CONTENT_PLAN_APPROVAL_PLAN_ID:-wave1-readiness-plan}"
EVIDENCE_FILE="${CONTENT_PLAN_APPROVAL_EVIDENCE_FILE:-${ROOT_DIR}/services/runtime/target/e2e-evidence/${CONTEXT#k3d-}/content-plan-approval-readiness.json}"

for command in "${KUBECTL}" node curl base64; do
  command -v "${command}" >/dev/null || {
    printf 'content_plan_approval_readiness.missing_command: %s\n' "${command}" >&2
    exit 2
  }
done

kube() {
  "${KUBECTL}" --context "${CONTEXT}" "$@"
}

ready_runtime_pod_identity() {
  kube get pods -n "${NAMESPACE}" -l app=anydesign-runtime,anydesign.io/runtime-role=primary -o json \
    | node -e '
const fs = require("node:fs");
const pods = JSON.parse(fs.readFileSync(0, "utf8")).items
  .filter(pod => !pod.metadata.deletionTimestamp)
  .filter(pod => pod.status.conditions?.some(condition =>
    condition.type === "Ready" && condition.status === "True"))
  .sort((a, b) => String(b.metadata.creationTimestamp).localeCompare(String(a.metadata.creationTimestamp)));
if (!pods[0]) process.exit(2);
process.stdout.write(`${pods[0].metadata.name} ${pods[0].metadata.uid}`);
'
}

wait_for_new_runtime_pod() {
  local previous_uid="$1"
  local identity=""
  local current_uid=""
  for _ in $(seq 1 180); do
    identity="$(ready_runtime_pod_identity 2>/dev/null || true)"
    if [[ -n "${identity}" ]]; then
      current_uid="${identity##* }"
      if [[ -n "${current_uid}" && "${current_uid}" != "${previous_uid}" ]]; then
        printf '%s' "${identity}"
        return 0
      fi
    fi
    sleep 1
  done
  printf 'content_plan_approval_readiness.runtime_restart_not_observed\n' >&2
  return 4
}

wait_for_stable_runtime_pod() {
  local expected_uid="$1"
  local identity=""
  local current_uid=""
  for _ in $(seq 1 10); do
    sleep 1
    identity="$(ready_runtime_pod_identity 2>/dev/null || true)"
    current_uid="${identity##* }"
    if [[ -z "${identity}" || "${current_uid}" != "${expected_uid}" ]]; then
      printf 'content_plan_approval_readiness.runtime_ready_pod_changed_during_stabilization\n' >&2
      return 4
    fi
  done
}

restart_runtime_and_wait() {
  local before=""
  local previous_uid=""
  local after=""
  before="$(ready_runtime_pod_identity 2>/dev/null || true)"
  previous_uid="${before##* }"
  [[ -n "${previous_uid}" ]] || {
    printf 'content_plan_approval_readiness.runtime_ready_pod_missing\n' >&2
    return 4
  }
  kube rollout restart deployment/"${DEPLOYMENT}" -n "${NAMESPACE}" >/dev/null
  after="$(wait_for_new_runtime_pod "${previous_uid}")"
  kube rollout status deployment/"${DEPLOYMENT}" -n "${NAMESPACE}" --timeout=300s >/dev/null
  wait_for_stable_runtime_pod "${after##* }"
  printf '%s' "${after}"
}

work_dir="$(mktemp -d)"
port_forward_pid=""
port_forward_target=""
probe_stage="bootstrap"
diagnostic_emitted=false
diagnose_error() {
  local exit_code=$?
  if [[ "${diagnostic_emitted}" == "true" ]]; then
    return "${exit_code}"
  fi
  diagnostic_emitted=true
  printf 'content_plan_approval_readiness.failed: stage=%s exit=%s target=%s\n' \
    "${probe_stage}" "${exit_code}" "${port_forward_target:-none}" >&2
  if [[ -s "${port_forward_log:-}" ]]; then
    sed -n '1,120p' "${port_forward_log}" >&2
  fi
  if [[ "${port_forward_target}" == pod/* ]]; then
    kube logs -n "${NAMESPACE}" "${port_forward_target}" --tail=120 >&2 || true
  fi
  return "${exit_code}"
}
cleanup() {
  if [[ -n "${port_forward_pid}" ]]; then
    kill "${port_forward_pid}" >/dev/null 2>&1 || true
    wait "${port_forward_pid}" >/dev/null 2>&1 || true
  fi
  rm -rf "${work_dir}"
}
trap cleanup EXIT
trap diagnose_error ERR

admin_token_file="${work_dir}/admin-token"
admin_curl_config="${work_dir}/admin-curl.conf"
port_forward_log="${work_dir}/port-forward.log"
stale_response_file="${work_dir}/stale-response.json"
umask 077

kube get namespace "${NAMESPACE}" >/dev/null
kube get namespace "${WORKSPACE_NAMESPACE}" >/dev/null
kube get deployment "${DEPLOYMENT}" -n "${NAMESPACE}" >/dev/null
kube get secret "${ADMIN_SECRET}" -n "${NAMESPACE}" >/dev/null

kube get secret "${ADMIN_SECRET}" -n "${NAMESPACE}" -o jsonpath='{.data.token}' \
  | base64 --decode >"${admin_token_file}"
[[ -s "${admin_token_file}" ]] || {
  printf 'content_plan_approval_readiness.admin_token_empty\n' >&2
  exit 3
}

node - "${admin_token_file}" "${admin_curl_config}" <<'NODE'
const fs = require("node:fs");
const [tokenFile, configFile] = process.argv.slice(2);
const token = fs.readFileSync(tokenFile, "utf8").trim();
fs.writeFileSync(configFile, [
  "silent",
  "show-error",
  "fail-with-body",
  "retry = 4",
  "retry-delay = 1",
  "retry-all-errors",
  'header = "x-anydesign-internal: true"',
  `header = "x-runtime-admin-token: ${token}"`,
  "",
].join("\n"), { mode: 0o600 });
NODE

start_port_forward() {
  local target="${1:-service/${SERVICE}}"
  port_forward_target="${target}"
  local_port="$(node -e '
const net = require("node:net");
const server = net.createServer();
server.unref();
server.listen(0, "127.0.0.1", () => {
  process.stdout.write(String(server.address().port));
  server.close();
});
')"
  kube port-forward -n "${NAMESPACE}" "${target}" "${local_port}:8080" \
    >"${port_forward_log}" 2>&1 &
  port_forward_pid=$!
  base_url="http://127.0.0.1:${local_port}"
  for _ in $(seq 1 60); do
    if ! kill -0 "${port_forward_pid}" >/dev/null 2>&1; then
      sed -n '1,120p' "${port_forward_log}" >&2
      exit 3
    fi
    if curl --silent --fail "${base_url}/health" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  printf 'content_plan_approval_readiness.runtime_health_timeout\n' >&2
  exit 3
}

stop_port_forward() {
  if [[ -n "${port_forward_pid}" ]]; then
    kill "${port_forward_pid}" >/dev/null 2>&1 || true
    wait "${port_forward_pid}" >/dev/null 2>&1 || true
    port_forward_pid=""
  fi
}

start_port_forward
runtime_image="$(kube get deployment "${DEPLOYMENT}" -n "${NAMESPACE}" \
  -o jsonpath='{.spec.template.spec.containers[0].image}')"
read -r pod_before pod_uid_before <<<"$(ready_runtime_pod_identity)"

access_body="$(node -e 'process.stdout.write(JSON.stringify({ownerPrincipalId:process.argv[1],workspaceNamespace:process.argv[2]}))' \
  "${PRINCIPAL_ID}" "${WORKSPACE_NAMESPACE}")"
curl --config "${admin_curl_config}" -X PUT -H 'content-type: application/json' \
  -d "${access_body}" "${base_url}/internal/projects/${PROJECT_ID}/access" >/dev/null

hash_a="$(node -e 'process.stdout.write("a".repeat(64))')"
hash_b="$(node -e 'process.stdout.write("b".repeat(64))')"

producer_before="$(curl --config "${admin_curl_config}" \
  "${base_url}/projects/${PROJECT_ID}/content-plan-approval-producer")"
sequence_before="$(node -e '
const value=JSON.parse(process.argv[1]);
if(value.ready!==true||value.schemaVersion!=="content-plan-approval-producer@1"||value.transactionSchemaVersion!=="content-plan-approval-transaction@1")process.exit(2);
process.stdout.write(String(value.lastSequence));
' "${producer_before}")"

change_one_body="$(node -e 'process.stdout.write(JSON.stringify({planId:process.argv[1],revision:1,contentHash:process.argv[2],changeEventId:process.argv[3]}))' \
  "${PLAN_ID}" "${hash_a}" "change-${PROJECT_ID}-1")"
change_one="$(curl --config "${admin_curl_config}" -X POST -H 'content-type: application/json' \
  -d "${change_one_body}" "${base_url}/projects/${PROJECT_ID}/content-plan-changes")"

approval_one_body="$(node -e 'process.stdout.write(JSON.stringify({planId:process.argv[1],revision:1,contentHash:process.argv[2],confirmationEventId:process.argv[3]}))' \
  "${PLAN_ID}" "${hash_a}" "confirmation-${PROJECT_ID}-1")"
approval_one="$(curl --config "${admin_curl_config}" -X POST -H 'content-type: application/json' \
  -d "${approval_one_body}" "${base_url}/projects/${PROJECT_ID}/content-plan-approvals")"
approval_one_id="$(node -e 'const v=JSON.parse(process.argv[1]);if(v.schemaVersion!=="content-plan-approval@1"||v.decision!=="approved")process.exit(2);process.stdout.write(v.approvalId)' "${approval_one}")"

verify_one_path="/projects/${PROJECT_ID}/content-plan-approvals/verify?planId=${PLAN_ID}&revision=1&contentHash=${hash_a}"
verify_one_uri="${base_url}${verify_one_path}"
verification_one="$(curl --config "${admin_curl_config}" "${verify_one_uri}")"
node -e 'const v=JSON.parse(process.argv[1]);if(v.state!=="verified")process.exit(2)' "${verification_one}"

change_two_body="$(node -e 'process.stdout.write(JSON.stringify({planId:process.argv[1],revision:2,contentHash:process.argv[2],changeEventId:process.argv[3]}))' \
  "${PLAN_ID}" "${hash_b}" "change-${PROJECT_ID}-2")"
change_two="$(curl --config "${admin_curl_config}" -X POST -H 'content-type: application/json' \
  -d "${change_two_body}" "${base_url}/projects/${PROJECT_ID}/content-plan-changes")"
node -e 'const v=JSON.parse(process.argv[1]);if(!v.invalidatedApprovalIds?.includes(process.argv[2]))process.exit(2)' \
  "${change_two}" "${approval_one_id}"

verification_invalidated="$(curl --config "${admin_curl_config}" "${verify_one_uri}")"
node -e 'const v=JSON.parse(process.argv[1]);if(v.state!=="invalidated"||v.reason!=="plan_changed")process.exit(2)' \
  "${verification_invalidated}"

stale_status="$(curl --silent --show-error --config "${admin_curl_config}" \
  --no-fail-with-body \
  -o "${stale_response_file}" -w '%{http_code}' -X POST -H 'content-type: application/json' \
  -d "${approval_one_body}" "${base_url}/projects/${PROJECT_ID}/content-plan-approvals")"
[[ "${stale_status}" == "409" ]] || {
  printf 'content_plan_approval_readiness.stale_confirmation_status: %s\n' "${stale_status}" >&2
  exit 4
}
node -e 'const v=JSON.parse(require("node:fs").readFileSync(process.argv[1],"utf8"));if(v.errorCode!=="content_plan.confirmation_event_stale")process.exit(2)' \
  "${stale_response_file}"

approval_two_body="$(node -e 'process.stdout.write(JSON.stringify({planId:process.argv[1],revision:2,contentHash:process.argv[2],confirmationEventId:process.argv[3]}))' \
  "${PLAN_ID}" "${hash_b}" "confirmation-${PROJECT_ID}-2")"
approval_two="$(curl --config "${admin_curl_config}" -X POST -H 'content-type: application/json' \
  -d "${approval_two_body}" "${base_url}/projects/${PROJECT_ID}/content-plan-approvals")"
approval_two_id="$(node -e 'const v=JSON.parse(process.argv[1]);if(v.decision!=="approved")process.exit(2);process.stdout.write(v.approvalId)' "${approval_two}")"

verify_two_path="/projects/${PROJECT_ID}/content-plan-approvals/verify?planId=${PLAN_ID}&revision=2&contentHash=${hash_b}"
verify_two_uri="${base_url}${verify_two_path}"
verification_two="$(curl --config "${admin_curl_config}" "${verify_two_uri}")"
node -e 'const v=JSON.parse(process.argv[1]);if(v.state!=="verified"||v.approval?.approvalId!==process.argv[2])process.exit(2)' \
  "${verification_two}" "${approval_two_id}"

producer_after="$(curl --config "${admin_curl_config}" \
  "${base_url}/projects/${PROJECT_ID}/content-plan-approval-producer")"
sequence_after="$(node -e 'const v=JSON.parse(process.argv[1]);process.stdout.write(String(v.lastSequence))' "${producer_after}")"
[[ "$((sequence_before + 4))" == "${sequence_after}" ]] || {
  printf 'content_plan_approval_readiness.sequence_mismatch: before=%s after=%s\n' \
    "${sequence_before}" "${sequence_after}" >&2
  exit 4
}

stop_port_forward
probe_stage="runtime_restart"
read -r pod_after pod_uid_after <<<"$(restart_runtime_and_wait)"

probe_stage="verification_after_restart"
post_restart_state="$(kube exec -i -n "${NAMESPACE}" "pod/${pod_after}" -- node - \
  "${verify_two_path}" "/projects/${PROJECT_ID}/content-plan-approval-producer" <<'NODE'
const [verificationPath, producerPath] = process.argv.slice(2);
const token = process.env.RUNTIME_INTERNAL_ADMIN_TOKEN;
if (!token) throw new Error("Runtime internal admin token is unavailable");
const headers = {
  "x-anydesign-internal": "true",
  "x-runtime-admin-token": token,
};
const request = async path => {
  const response = await fetch(`http://127.0.0.1:8080${path}`, { headers });
  const body = await response.text();
  if (!response.ok) throw new Error(`${path} returned ${response.status}: ${body}`);
  return JSON.parse(body);
};
Promise.all([request(verificationPath), request(producerPath)])
  .then(([verification, producer]) => process.stdout.write(JSON.stringify({ verification, producer })))
  .catch(error => { console.error(error); process.exit(2); });
NODE
)"
node - "${post_restart_state}" "${approval_two_id}" "${sequence_after}" <<'NODE'
const [stateRaw, approvalId, sequence] = process.argv.slice(2);
const { verification, producer } = JSON.parse(stateRaw);
if (verification.state !== "verified" || verification.approval?.approvalId !== approvalId) process.exit(2);
if (producer.ready !== true || producer.lastSequence !== Number(sequence)) process.exit(2);
NODE

mkdir -p "$(dirname "${EVIDENCE_FILE}")"
node - "${EVIDENCE_FILE}" "${CONTEXT}" "${NAMESPACE}" "${DEPLOYMENT}" "${runtime_image}" \
  "${PROJECT_ID}" "${PLAN_ID}" "${WORKSPACE_NAMESPACE}" "${PRINCIPAL_ID}" \
  "${sequence_before}" "${sequence_after}" "${approval_one_id}" "${approval_two_id}" \
  "${pod_before}" "${pod_uid_before}" "${pod_after}" "${pod_uid_after}" <<'NODE'
const fs = require("node:fs");
const [
  out, context, namespace, deployment, runtimeImage, projectId, planId, workspaceNamespace,
  principalId, sequenceBefore, sequenceAfter, firstApprovalId, secondApprovalId,
  podBefore, podUidBefore, podAfter, podUidAfter,
] = process.argv.slice(2);
const evidence = {
  schemaVersion: "content-plan-approval-readiness-evidence@1",
  recordedAt: new Date().toISOString(),
  target: { context, namespace, deployment, runtimeImage },
  project: { projectId, planId, workspaceNamespace, principalId },
  producer: {
    schemaVersion: "content-plan-approval-producer@1",
    transactionSchemaVersion: "content-plan-approval-transaction@1",
    ready: true,
    sequenceBefore: Number(sequenceBefore),
    sequenceAfter: Number(sequenceAfter),
  },
  lifecycle: {
    initialApproval: { approvalId: firstApprovalId, state: "verified" },
    planChange: { revision: 2, invalidatedApprovalId: firstApprovalId },
    staleConfirmation: { httpStatus: 409, errorCode: "content_plan.confirmation_event_stale" },
    reapproval: { approvalId: secondApprovalId, state: "verified" },
    restartRecovery: {
      podBefore: { name: podBefore, uid: podUidBefore },
      podAfter: { name: podAfter, uid: podUidAfter },
      recoveredState: "verified",
    },
  },
  credentialHandling: {
    internalProducerAuthorization: true,
    publicPrincipalSecretMutated: false,
    secretMaterialRecorded: false,
  },
  passed: true,
};
fs.writeFileSync(out, `${JSON.stringify(evidence, null, 2)}\n`, { mode: 0o600 });
NODE

stop_port_forward
printf 'Content Plan Approval readiness verified across Runtime restart: project=%s evidence=%s\n' \
  "${PROJECT_ID}" "${EVIDENCE_FILE}"
