#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
cluster_name="${ANYDESIGN_E2E_CLUSTER:?ANYDESIGN_E2E_CLUSTER is required}"
namespace="${ANYDESIGN_E2E_NAMESPACE:-anydesign-sandboxes}"
evidence_path="${RECOVERY_EVIDENCE_PATH:-${ROOT_DIR}/services/runtime/target/e2e-evidence/recovery-${cluster_name}.json}"
active_recovery_evidence="${ACTIVE_RECOVERY_EVIDENCE_PATH:?ACTIVE_RECOVERY_EVIDENCE_PATH is required}"
runtime_evidence_file="${RECOVERY_RUNTIME_EVIDENCE_FILE:?RECOVERY_RUNTIME_EVIDENCE_FILE is required}"
baseline_file="${RECOVERY_BASELINE_FILE:-}"
cd "${ROOT_DIR}"

context="$(${KUBECTL} config current-context)"
if [[ "${context}" != "k3d-${cluster_name}" ]]; then
  printf 'recovery gate requires context k3d-%s; got %s\n' "${cluster_name}" "${context}" >&2
  exit 2
fi

cargo test --manifest-path services/runtime/Cargo.toml --lib \
  channel_manager::tests::pod_uid_change_retires_ready_lease_before_reacquiring -- --exact
cargo test --manifest-path services/runtime/Cargo.toml --test checkpoint \
  runtime_restart_reacquires_ready_sandbox_before_resuming_checkpoint -- --exact
cargo test --manifest-path services/runtime/Cargo.toml --test checkpoint \
  persisted_runs_are_hydrated_once_per_store_instance -- --exact
cargo test --manifest-path services/runtime/Cargo.toml --test checkpoint \
  truncated_run_wal_tail_is_repaired_before_restart_continues_writing -- --exact
cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools \
  preview_publish_rejects_unchanged_source_after_failed_generation_validation -- --exact
cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools \
  preview_publish_rejects_candidate_that_fails_frozen_brief_acceptance -- --exact
cargo test --manifest-path services/runtime/Cargo.toml --test preview_promotion \
  promotion_wal_recovers_current_run_publish_and_pending_outbox_once -- --exact
cargo test --manifest-path services/runtime/Cargo.toml --test preview_promotion \
  startup_reconcile_replays_promotion_after_immutable_bytes_before_cas -- --exact

old_pod="$(${KUBECTL} get pods -n "${namespace}" --no-headers \
  | awk '$3=="Running" && $1 ~ /astro-website-pool/ {print $1; exit}')"
[[ -n "${old_pod}" ]] || { printf 'ready Astro warm Pod is required\n' >&2; exit 3; }
old_uid="$(${KUBECTL} get pod -n "${namespace}" "${old_pod}" -o jsonpath='{.metadata.uid}')"
${KUBECTL} delete pod -n "${namespace}" "${old_pod}" --wait=false >/dev/null

deadline=$((SECONDS + 180))
new_pod=""
new_uid=""
while (( SECONDS < deadline )); do
  while IFS= read -r pod; do
    [[ -n "${pod}" ]] || continue
    ready="$(${KUBECTL} get pod -n "${namespace}" "${pod}" \
      -o 'jsonpath={.status.conditions[?(@.type=="Ready")].status}' 2>/dev/null || true)"
    uid="$(${KUBECTL} get pod -n "${namespace}" "${pod}" \
      -o jsonpath='{.metadata.uid}' 2>/dev/null || true)"
    if [[ "${ready}" == "True" && -n "${uid}" && "${uid}" != "${old_uid}" ]]; then
      new_pod="${pod}"
      new_uid="${uid}"
      break 2
    fi
  done < <(${KUBECTL} get pods -n "${namespace}" --no-headers \
    | awk '$3=="Running" && $1 ~ /astro-website-pool/ {print $1}')
  sleep 2
done
[[ -n "${new_uid}" ]] || { printf 'replacement warm Pod did not become ready\n' >&2; exit 4; }

node -e '
const fs=require("fs");
const evidence=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));
for(const key of ["runtimeRestart","portForwardKill"]){
  if(evidence[key]?.result!=="pass"||evidence[key]?.orphanCount!==0)process.exit(2);
}
' "${active_recovery_evidence}"
node -e '
const fs=require("fs");
const projects=JSON.parse(fs.readFileSync(process.argv[1],"utf8")).projects;
if(!Array.isArray(projects)||projects.length<2)process.exit(2);
for(const project of projects){
  if(project.cancelCleanup?.passed!==true||project.cancelCleanup?.runStatus!=="cancelled"||project.cancelCleanup?.previewHttpStatusAfterCancel!==404)process.exit(2);
}
' "${runtime_evidence_file}"

baseline_json='{"claimNames":[],"protectedSandboxNames":[]}'
if [[ -n "${baseline_file}" ]]; then
  [[ -s "${baseline_file}" ]] || {
    printf 'recovery baseline file is missing or empty: %s\n' "${baseline_file}" >&2
    exit 2
  }
  baseline_json="$(cat "${baseline_file}")"
fi
current_claims_json="$(${KUBECTL} get sandboxclaims.extensions.agents.x-k8s.io \
  -n "${namespace}" -o json)"
claim_audit="$(node -e '
const baseline=JSON.parse(process.argv[1]);
const current=JSON.parse(process.argv[2]).items.map(item=>item.metadata.name).sort();
const expected=new Set(baseline.claimNames||[]);
const unexpected=current.filter(name=>!expected.has(name));
process.stdout.write(JSON.stringify({
  baselineClaimCount:expected.size,
  currentClaimCount:current.length,
  unexpectedClaimNames:unexpected,
  unexpectedClaimCount:unexpected.length,
}));
' "${baseline_json}" "${current_claims_json}")"
claim_count="$(node -e 'process.stdout.write(String(JSON.parse(process.argv[1]).unexpectedClaimCount))' \
  "${claim_audit}")"
if [[ "${claim_count}" != "0" ]]; then
  printf 'orphan audit failed: unexpected SandboxClaims remain: %s\n' \
    "$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).unexpectedClaimNames.join(\",\"))' "${claim_audit}")" >&2
  exit 5
fi
protected_sandboxes="$(node -e '
const baseline=JSON.parse(process.argv[1]);
process.stdout.write((baseline.protectedSandboxNames||[]).join("\n"));
' "${baseline_json}")"
preview_process_count=0
while IFS= read -r pod; do
  [[ -n "${pod}" ]] || continue
  if [[ -n "${protected_sandboxes}" ]] \
    && printf '%s\n' "${protected_sandboxes}" | rg -Fxq "${pod}"; then
    continue
  fi
  if ${KUBECTL} exec -n "${namespace}" "${pod}" -- ps -eo args 2>/dev/null \
    | rg '/opt/anydesign/bootstrap/static-preview-server\.js' >/dev/null; then
    preview_process_count=$((preview_process_count + 1))
  fi
done < <(${KUBECTL} get pods -n "${namespace}" --no-headers | awk '$3=="Running" {print $1}')
if [[ "${preview_process_count}" != "0" ]]; then
  printf 'orphan audit failed: %s Preview processes remain\n' "${preview_process_count}" >&2
  exit 5
fi

mkdir -p "$(dirname "${evidence_path}")"
node -e '
const fs=require("fs");
const active=JSON.parse(fs.readFileSync(process.argv[8],"utf8"));
const claimAudit=JSON.parse(process.argv[9]);
fs.writeFileSync(process.argv[1],JSON.stringify({
  schemaVersion:"runtime-recovery-gate@2",
  cluster:process.argv[2],
  kubeContext:process.argv[3],
  podReplacement:{oldPod:process.argv[4],oldUid:process.argv[5],newPod:process.argv[6],newUid:process.argv[7]},
  scenarios:[
    active.runtimeRestart,
    active.portForwardKill,
    {scenario:"sandbox-pod-replacement",injectionPoint:"ready-warm-pod",result:"pass",orphanCount:0},
    {scenario:"channel-lease-pod-uid-change",injectionPoint:"ready-channel-lease",result:"pass",orphanCount:0},
    {scenario:"checkpoint-runtime-restart",injectionPoint:"persisted-partial-run",result:"pass",orphanCount:0},
    {scenario:"artifact-staged-before-cas",injectionPoint:"promotion-wal",result:"pass",orphanCount:0},
    {scenario:"cas-before-event",injectionPoint:"promotion-outbox",result:"pass",orphanCount:0},
    {scenario:"run-cancel",injectionPoint:"active-preview-process",result:"pass",orphanCount:0},
  ],
  orphanAudit:{
    claimCount:claimAudit.unexpectedClaimCount,
    baselineClaimCount:claimAudit.baselineClaimCount,
    currentClaimCount:claimAudit.currentClaimCount,
    unexpectedClaimNames:claimAudit.unexpectedClaimNames,
    previewProcessCount:Number(process.argv[10]),
    result:"pass"
  },
  checks:{channelLease:true,checkpointResume:true,promotionWal:true,promotionReconcile:true,runCancel:true,portForwardReconnect:true,orphanAudit:true},
  result:"pass"
},null,2)+"\n");
' "${evidence_path}" "${cluster_name}" "${context}" "${old_pod}" "${old_uid}" "${new_pod}" "${new_uid}" \
  "${active_recovery_evidence}" "${claim_audit}" "${preview_process_count}"
printf 'Runtime recovery gate passed: %s\n' "${evidence_path}"
