#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SESSION_DIR="${1:-}"
SIDE="${2:-}"
DEPLOYMENT="${3:-}"
CASE_EVIDENCE="${4:-}"
OUTPUT_FILE="${5:-}"
KUBECTL="${KUBECTL:-kubectl}"

if [[ -z "${SESSION_DIR}" || ! "${SIDE}" =~ ^(control|candidate)$ || -z "${DEPLOYMENT}" \
  || ! -s "${CASE_EVIDENCE}" || -z "${OUTPUT_FILE}" ]]; then
  printf 'usage: %s <prepared-session-dir> <control|candidate> <deployment> <case-evidence.json> <output.json>\n' "$0" >&2
  exit 2
fi
[[ ! -e "${OUTPUT_FILE}" ]] || {
  printf 'generation_context_restart.output_exists: %s\n' "${OUTPUT_FILE}" >&2
  exit 2
}

for command in "${KUBECTL}" node curl; do
  command -v "${command}" >/dev/null || {
    printf 'generation_context_restart.missing_command: %s\n' "${command}" >&2
    exit 2
  }
done

SESSION_FILE="${SESSION_DIR}/session.json"
SESSION_META_FILE="${SESSION_DIR}/session-meta.json"
PRIVATE_KEY_FILE="${SESSION_DIR}/.credentials/principal-private.pem"
ADMIN_TOKEN_FILE="${SESSION_DIR}/.credentials/runtime-admin-token"
for file in "${SESSION_FILE}" "${SESSION_META_FILE}" "${PRIVATE_KEY_FILE}" "${ADMIN_TOKEN_FILE}"; do
  [[ -s "${file}" ]] || {
    printf 'generation_context_restart.required_file_missing: %s\n' "${file}" >&2
    exit 2
  }
done

read -r context namespace frozen_deployment_revision < <(
  node - "${SESSION_FILE}" "${SESSION_META_FILE}" "${SIDE}" "${DEPLOYMENT}" <<'NODE'
const fs = require("node:fs");
const [sessionFile, metaFile, side, deployment] = process.argv.slice(2);
const session = JSON.parse(fs.readFileSync(sessionFile, "utf8"));
const meta = JSON.parse(fs.readFileSync(metaFile, "utf8"));
const frozen = meta.deployments?.find(item => item.side === side);
if (meta.sessionId !== session.sessionId || frozen?.deployment !== deployment) {
  throw new Error("Runtime Restart target does not match the frozen cohort session");
}
const revision = session.runtimes?.[side]?.deploymentRevision;
if (!meta.context || !meta.workspaceNamespace || !revision) {
  throw new Error("Runtime Restart session identity is incomplete");
}
process.stdout.write(`${meta.context} ${meta.workspaceNamespace} ${revision}\n`);
NODE
)

read -r project_id run_id route artifact_mode snapshot_id preview_lease_id < <(
  node - "${CASE_EVIDENCE}" <<'NODE'
const fs = require("node:fs");
const evidence = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const builds = evidence.runs?.filter(run => run.phase === "build") || [];
if (evidence.schemaVersion !== "generation-real-provider-case-evidence@2"
  || !new Set(["docs", "website"]).has(evidence.kind)
  || evidence.status !== "accepted" || builds.length !== 1) {
  throw new Error("Runtime Restart requires one accepted docs or website Build Run");
}
if (!evidence.projectId || !builds[0].runId || !evidence.expectedRoute || !evidence.expectedText) {
  throw new Error("Runtime Restart case evidence is missing Project, Run, route, or acceptance marker");
}
if (evidence.kind === "docs") {
  process.stdout.write(`${evidence.projectId} ${builds[0].runId} ${evidence.expectedRoute} current_version - -\n`);
} else {
  const snapshotId = evidence.draftPreview?.snapshotId || evidence.draftPreview?.durableSnapshotId;
  const leaseId = evidence.draftPreview?.leaseId;
  if (!snapshotId || !leaseId || evidence.draftPreview?.expectedTextFound !== true) {
    throw new Error("Website Runtime Restart requires an accepted durable Draft Preview identity");
  }
  process.stdout.write(`${evidence.projectId} ${builds[0].runId} ${evidence.expectedRoute} draft_preview ${snapshotId} ${leaseId}\n`);
}
NODE
)

work_dir="$(dirname "${OUTPUT_FILE}")/runtime-restart-${SIDE}-work"
mkdir "${work_dir}"
marker_file="$(mktemp)"
port_forward_pid=""
port_forward_port=""

cleanup() {
  local status=$?
  if [[ -n "${port_forward_pid}" ]]; then
    kill "${port_forward_pid}" >/dev/null 2>&1 || true
    wait "${port_forward_pid}" >/dev/null 2>&1 || true
  fi
  rm -f "${marker_file}"
  exit "${status}"
}
trap cleanup EXIT

node - "${CASE_EVIDENCE}" "${marker_file}" <<'NODE'
const fs = require("node:fs");
const evidence = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
fs.writeFileSync(process.argv[3], evidence.expectedText, { mode: 0o600 });
NODE

deployment_identity() {
  "${KUBECTL}" --context "${context}" get deployment "${DEPLOYMENT}" -n anydesign-runtime -o json \
    | node -e '
const crypto = require("node:crypto");
let raw = "";
process.stdin.on("data", chunk => { raw += chunk; });
process.stdin.on("end", () => {
  const deployment = JSON.parse(raw);
  const canonical = value => Array.isArray(value)
    ? `[${value.map(canonical).join(",")}]`
    : value && typeof value === "object"
      ? `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`
      : JSON.stringify(value);
  const digest = crypto.createHash("sha256").update(canonical(deployment.spec.template)).digest("hex");
  process.stdout.write(`${deployment.metadata.uid} ${deployment.metadata.generation} ${digest}\n`);
});'
}

ready_pod_identity() {
  "${KUBECTL}" --context "${context}" get pods -n anydesign-runtime \
    -l "anydesign.io/generation-context-cohort=${SIDE}" -o json \
    | node -e '
let raw = "";
process.stdin.on("data", chunk => { raw += chunk; });
process.stdin.on("end", () => {
  const pods = JSON.parse(raw).items.filter(pod => !pod.metadata.deletionTimestamp
    && pod.status.phase === "Running"
    && pod.status.conditions?.some(condition => condition.type === "Ready" && condition.status === "True"));
  if (pods.length !== 1) process.exit(3);
  process.stdout.write(`${pods[0].metadata.name} ${pods[0].metadata.uid}\n`);
});'
}

start_port_forward() {
  local log_file="$1"
  port_forward_port="$(node -e '
const net = require("node:net");
const server = net.createServer();
server.unref();
server.listen(0, "127.0.0.1", () => {
  process.stdout.write(String(server.address().port));
  server.close();
});')"
  "${KUBECTL}" --context "${context}" port-forward -n anydesign-runtime \
    service/"${DEPLOYMENT}" "${port_forward_port}:8080" >"${log_file}" 2>&1 &
  port_forward_pid=$!
  for _ in $(seq 1 90); do
    if ! kill -0 "${port_forward_pid}" >/dev/null 2>&1; then
      sed -n '1,100p' "${log_file}" >&2
      return 3
    fi
    if curl --fail --silent "http://127.0.0.1:${port_forward_port}/health" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  printf 'generation_context_restart.port_forward_not_ready\n' >&2
  return 3
}

stop_port_forward() {
  if [[ -n "${port_forward_pid}" ]]; then
    kill "${port_forward_pid}" >/dev/null 2>&1 || true
    wait "${port_forward_pid}" >/dev/null 2>&1 || true
    port_forward_pid=""
  fi
}

probe_snapshot() {
  local label="$1"
  start_port_forward "${work_dir}/port-forward-${label}.log"
  GENERATION_RUNTIME_RESTART_RELEASE_SANDBOX="$([[ "${label}" == "after" ]] && printf 1 || printf 0)" \
    node "${ROOT_DIR}/infra/generation-reliability/probe-generation-context-runtime-restart.mjs" \
    "http://127.0.0.1:${port_forward_port}" \
    "${PRIVATE_KEY_FILE}" \
    "${ADMIN_TOKEN_FILE}" \
    "${project_id}" \
    "${run_id}" \
    "${route}" \
    "${marker_file}" \
    "${artifact_mode}" \
    "${snapshot_id}" \
    "${preview_lease_id}" \
    "${work_dir}/${label}.json"
  stop_port_forward
}

read -r deployment_uid_before deployment_generation_before template_sha_before <<<"$(deployment_identity)"
read -r pod_name_before pod_uid_before <<<"$(ready_pod_identity)"
probe_snapshot before

restart_started_ms="$(node -e 'process.stdout.write(String(Date.now()))')"
"${KUBECTL}" --context "${context}" delete pod "${pod_name_before}" -n anydesign-runtime --wait=false >/dev/null

pod_name_after=""
pod_uid_after=""
for _ in $(seq 1 180); do
  identity="$(ready_pod_identity 2>/dev/null || true)"
  if [[ -n "${identity}" ]]; then
    read -r candidate_name candidate_uid <<<"${identity}"
    if [[ "${candidate_uid}" != "${pod_uid_before}" ]]; then
      pod_name_after="${candidate_name}"
      pod_uid_after="${candidate_uid}"
      break
    fi
  fi
  sleep 1
done
[[ -n "${pod_uid_after}" ]] || {
  printf 'generation_context_restart.new_ready_pod_not_observed: deployment=%s\n' "${DEPLOYMENT}" >&2
  exit 4
}
"${KUBECTL}" --context "${context}" rollout status deployment/"${DEPLOYMENT}" \
  -n anydesign-runtime --timeout=300s >/dev/null
restart_finished_ms="$(node -e 'process.stdout.write(String(Date.now()))')"

read -r deployment_uid_after deployment_generation_after template_sha_after <<<"$(deployment_identity)"
probe_snapshot after

node - "${work_dir}/metadata.json" \
  "${SIDE}" "${DEPLOYMENT}" "${frozen_deployment_revision}" \
  "${deployment_uid_before}" "${deployment_generation_before}" "${template_sha_before}" \
  "${pod_name_before}" "${pod_uid_before}" \
  "${deployment_uid_after}" "${deployment_generation_after}" "${template_sha_after}" \
  "${pod_name_after}" "${pod_uid_after}" \
  "$((restart_finished_ms - restart_started_ms))" <<'NODE'
const fs = require("node:fs");
const [file, side, deployment, runtimeDeploymentRevision,
  deploymentUid, deploymentGeneration, deploymentTemplateSha256,
  podBeforeName, podBeforeUid,
  deploymentUidAfter, deploymentGenerationAfter, deploymentTemplateSha256After,
  podAfterName, podAfterUid, restartDurationMs] = process.argv.slice(2);
const metadata = {
  recordedAt: new Date().toISOString(),
  side,
  deployment,
  runtimeDeploymentRevision,
  deploymentUid,
  deploymentGeneration: Number(deploymentGeneration),
  deploymentTemplateSha256,
  podBefore: { name: podBeforeName, uid: podBeforeUid },
  deploymentUidAfter,
  deploymentGenerationAfter: Number(deploymentGenerationAfter),
  deploymentTemplateSha256After,
  podAfter: { name: podAfterName, uid: podAfterUid },
  restartDurationMs: Number(restartDurationMs),
};
fs.writeFileSync(file, `${JSON.stringify(metadata, null, 2)}\n`, { flag: "wx", mode: 0o600 });
NODE

node "${ROOT_DIR}/services/runtime/scripts/generation-context-runtime-restart-evidence.mjs" \
  "${work_dir}/metadata.json" \
  "${work_dir}/before.json" \
  "${work_dir}/after.json" \
  "${OUTPUT_FILE}"

if rg -q -i '(bearer[[:space:]]+[a-z0-9._-]{20,}|sk-[a-z0-9_-]{12,}|api[_-]?key["[:space:]]*:[[:space:]]*[a-z0-9._-]{12,})' \
  "${work_dir}" "${OUTPUT_FILE}"; then
  printf 'generation_context_restart.credential_material_detected\n' >&2
  exit 7
fi

printf 'Generation Context Runtime Restart verified: side=%s project=%s run=%s evidence=%s\n' \
  "${SIDE}" "${project_id}" "${run_id}" "${OUTPUT_FILE}"
