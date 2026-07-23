#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SESSION_DIR="${1:-}"
BATCH_ID="${2:-}"
CASE_ID="${3:-}"
BUCKET="${4:-greenfield}"
RUNNER="${GENERATION_COHORT_REAL_PROVIDER_RUNNER:-${ROOT_DIR}/infra/generation-reliability/run-real-provider-examples.sh}"
RUNTIME_RESTART_CANARY="${GENERATION_COHORT_RUNTIME_RESTART:-0}"
RUNTIME_RESTART_RUNNER="${GENERATION_COHORT_RUNTIME_RESTART_RUNNER:-${ROOT_DIR}/infra/generation-reliability/verify-generation-context-runtime-restart.sh}"
NONVISUAL_REFERENCE_CANARY="${GENERATION_COHORT_NONVISUAL_REFERENCE:-0}"
MULTIMODAL_REFERENCE_CANARY="${GENERATION_COHORT_MULTIMODAL_REFERENCE:-0}"
MAX_CASE_ATTEMPTS="${GENERATION_COHORT_MAX_CASE_ATTEMPTS:-1}"

usage() {
  printf 'usage: %s <prepared-session-dir> <batch-id> <case-id> <greenfield|warm_copy_css|warm_structural|cold_dev|repair>\n' "$0" >&2
}

[[ -n "${SESSION_DIR}" && -n "${BATCH_ID}" && -n "${CASE_ID}" ]] || {
  usage
  exit 2
}
[[ "${BATCH_ID}" =~ ^[a-z0-9][a-z0-9-]{0,62}$ ]] || {
  printf 'generation_cohort_pair.invalid_batch_id: %s\n' "${BATCH_ID}" >&2
  exit 2
}
[[ "${CASE_ID}" =~ ^[a-z0-9][a-z0-9-]{0,62}$ ]] || {
  printf 'generation_cohort_pair.invalid_case_id: %s\n' "${CASE_ID}" >&2
  exit 2
}
case "${BUCKET}" in
  greenfield|warm_copy_css|warm_structural|cold_dev|repair) ;;
  *)
    printf 'generation_cohort_pair.unsupported_bucket: %s\n' "${BUCKET}" >&2
    exit 2
    ;;
esac
[[ "${RUNTIME_RESTART_CANARY}" =~ ^[01]$ ]] || {
  printf 'generation_cohort_pair.invalid_runtime_restart_flag: %s\n' "${RUNTIME_RESTART_CANARY}" >&2
  exit 2
}
[[ "${NONVISUAL_REFERENCE_CANARY}" =~ ^[01]$ ]] || {
  printf 'generation_cohort_pair.invalid_nonvisual_reference_flag: %s\n' "${NONVISUAL_REFERENCE_CANARY}" >&2
  exit 2
}
[[ "${MULTIMODAL_REFERENCE_CANARY}" =~ ^[01]$ ]] || {
  printf 'generation_cohort_pair.invalid_multimodal_reference_flag: %s\n' "${MULTIMODAL_REFERENCE_CANARY}" >&2
  exit 2
}
if [[ "${NONVISUAL_REFERENCE_CANARY}" == "1" && "${MULTIMODAL_REFERENCE_CANARY}" == "1" ]]; then
  printf 'generation_cohort_pair.reference_modes_mutually_exclusive\n' >&2
  exit 2
fi
[[ "${MAX_CASE_ATTEMPTS}" =~ ^[1-3]$ ]] || {
  printf 'generation_cohort_pair.invalid_max_case_attempts: %s\n' "${MAX_CASE_ATTEMPTS}" >&2
  exit 2
}
if [[ "${RUNTIME_RESTART_CANARY}" == "1" && "${BUCKET}" != "greenfield" ]]; then
  printf 'generation_cohort_pair.runtime_restart_requires_greenfield_bucket\n' >&2
  exit 2
fi
if [[ "${NONVISUAL_REFERENCE_CANARY}" == "1" && "${BUCKET}" != "warm_copy_css" ]]; then
  printf 'generation_cohort_pair.nonvisual_reference_requires_warm_copy_css_bucket\n' >&2
  exit 2
fi
if [[ "${MULTIMODAL_REFERENCE_CANARY}" == "1" && "${BUCKET}" != "warm_copy_css" ]]; then
  printf 'generation_cohort_pair.multimodal_reference_requires_warm_copy_css_bucket\n' >&2
  exit 2
fi

SESSION_DIR="$(cd "${SESSION_DIR}" 2>/dev/null && pwd)" || {
  printf 'generation_cohort_pair.session_missing: %s\n' "${SESSION_DIR}" >&2
  exit 2
}
SESSION_FILE="${SESSION_DIR}/session.json"
SESSION_META_FILE="${SESSION_DIR}/session-meta.json"
SESSION_INVALIDATION_FILE="${SESSION_DIR}/session-invalidation.json"
LEDGER_FILE="${SESSION_DIR}/cohort.ndjson"
CASES_FILE="${GENERATION_COHORT_CASES_FILE:-${ROOT_DIR}/infra/generation-reliability/real-provider-cases.json}"
PAIR_ID="${BATCH_ID}-${CASE_ID}-${BUCKET//_/-}"
PAIR_DIR="${SESSION_DIR}/pairs/${PAIR_ID}"
SPEC_FILE="${PAIR_DIR}/pair-spec.json"
RESULT_FILE="${PAIR_DIR}/pair-result.json"

for command in node bash; do
  command -v "${command}" >/dev/null || {
    printf 'generation_cohort_pair.missing_command: %s\n' "${command}" >&2
    exit 2
  }
done
for file in "${SESSION_FILE}" "${SESSION_META_FILE}" "${LEDGER_FILE}" "${CASES_FILE}" "${RUNNER}"; do
  [[ -s "${file}" ]] || {
    printf 'generation_cohort_pair.required_file_missing: %s\n' "${file}" >&2
    exit 2
  }
done
if [[ -e "${SESSION_INVALIDATION_FILE}" ]]; then
  [[ -s "${SESSION_INVALIDATION_FILE}" ]] || {
    printf 'generation_cohort_pair.session_invalidation_malformed: %s\n' "${SESSION_INVALIDATION_FILE}" >&2
    exit 2
  }
  invalidation_reason="$({
    node - "${SESSION_FILE}" "${SESSION_INVALIDATION_FILE}" <<'NODE'
const fs = require("node:fs");
const [sessionFile, invalidationFile] = process.argv.slice(2);
const session = JSON.parse(fs.readFileSync(sessionFile, "utf8"));
const invalidation = JSON.parse(fs.readFileSync(invalidationFile, "utf8"));
if (invalidation.schemaVersion !== "generation-context-cohort-session-invalidation@1") {
  throw new Error("unsupported cohort session invalidation schema");
}
if (invalidation.sessionId !== session.sessionId) {
  throw new Error("cohort session invalidation identity mismatch");
}
if (typeof invalidation.reason !== "string" || !invalidation.reason.trim()) {
  throw new Error("cohort session invalidation reason is required");
}
process.stdout.write(invalidation.reason);
NODE
  } 2>&1)" || {
    printf 'generation_cohort_pair.session_invalidation_malformed: %s\n' "${SESSION_INVALIDATION_FILE}" >&2
    exit 2
  }
  printf 'generation_cohort_pair.session_invalidated: session=%s reason=%s\n' \
    "${SESSION_DIR}" "${invalidation_reason}" >&2
  exit 2
fi
if [[ "${RUNTIME_RESTART_CANARY}" == "1" && ! -s "${RUNTIME_RESTART_RUNNER}" ]]; then
  printf 'generation_cohort_pair.required_file_missing: %s\n' "${RUNTIME_RESTART_RUNNER}" >&2
  exit 2
fi
[[ ! -e "${PAIR_DIR}" ]] || {
  printf 'generation_cohort_pair.already_exists: %s\n' "${PAIR_DIR}" >&2
  exit 2
}

read -r context workspace_namespace control_deployment candidate_deployment fixture_kind < <(
  node - "${SESSION_FILE}" "${SESSION_META_FILE}" "${CASES_FILE}" "${CASE_ID}" "${PAIR_ID}" \
    "${NONVISUAL_REFERENCE_CANARY}" "${MULTIMODAL_REFERENCE_CANARY}" <<'NODE'
const crypto = require("node:crypto");
const fs = require("node:fs");
const [sessionFile, metaFile, casesFile, caseId, pairId, nonvisualReference, multimodalReference] = process.argv.slice(2);
const session = JSON.parse(fs.readFileSync(sessionFile, "utf8"));
const meta = JSON.parse(fs.readFileSync(metaFile, "utf8"));
const casesRaw = fs.readFileSync(casesFile);
const cases = JSON.parse(casesRaw);
if (session.schemaVersion !== "generation-context-paired-cohort-session@1") throw new Error("invalid cohort session");
if (meta.schemaVersion !== "generation-context-cohort-session-meta@1" || meta.sessionId !== session.sessionId) {
  throw new Error("cohort session metadata mismatch");
}
if (session.source?.commit !== meta.sourceCommit || session.source?.dirty !== meta.sourceDirty) {
  throw new Error("cohort session source identity mismatch");
}
const fixture = cases.cases?.find(item => item.id === caseId);
if (!fixture) throw new Error(`unknown fixture: ${caseId}`);
if (!["website", "docs"].includes(fixture.kind)) throw new Error(`unsupported fixture kind: ${fixture.kind}`);
if (!meta.context || !meta.workspaceNamespace) throw new Error("session target metadata is incomplete");
if (crypto.createHash("sha256").update(casesRaw).digest("hex") !== session.fixtureManifestSha256) {
  throw new Error("case manifest drifted from the prepared session");
}
const provider = session.providers?.find(item => item.modelResourceId === cases.provider?.modelResourceId);
if (!provider) throw new Error("case manifest Provider Resource is outside the prepared session allowlist");
if (nonvisualReference === "1" && provider.visionCapable !== false) {
  throw new Error("non-visual canary requires a frozen non-vision Provider Resource");
}
if (
  multimodalReference === "1" &&
  (provider.visionCapable !== true ||
    !provider.supportedImageMediaTypes?.includes("image/png") ||
    !Number.isSafeInteger(provider.maxImageCount) || provider.maxImageCount < 1)
) {
  throw new Error("multimodal canary requires a frozen bounded PNG-capable Provider Resource");
}
const deployment = side => meta.deployments?.find(item => item.side === side)?.deployment;
if (!deployment("control") || !deployment("candidate")) throw new Error("session deployment metadata is incomplete");
const records = fs.readFileSync(sessionFile.replace(/session\.json$/, "cohort.ndjson"), "utf8")
  .split(/\r?\n/).filter(Boolean).map(line => JSON.parse(line));
if (records.some(record => record.kind === "sample" && record.payload?.pairId === pairId)) {
  throw new Error(`pair already exists in ledger: ${pairId}`);
}
process.stdout.write(`${meta.context} ${meta.workspaceNamespace} ${deployment("control")} ${deployment("candidate")} ${fixture.kind}\n`);
NODE
)

if [[ "${BUCKET}" =~ ^(warm_copy_css|warm_structural|cold_dev)$ && "${fixture_kind}" != "website" ]]; then
  printf 'generation_cohort_pair.draft_lifecycle_template_unsupported: fixture=%s kind=%s bucket=%s\n' \
    "${CASE_ID}" "${fixture_kind}" "${BUCKET}" >&2
  exit 2
fi
if [[ "${BUCKET}" == "repair" && "${fixture_kind}" != "docs" ]]; then
  printf 'generation_cohort_pair.repair_template_unsupported: fixture=%s kind=%s bucket=%s\n' \
    "${CASE_ID}" "${fixture_kind}" "${BUCKET}" >&2
  exit 2
fi
mkdir -p "${PAIR_DIR}/control" "${PAIR_DIR}/candidate"
warm_marker="PAIR_WARM_$(node -e 'process.stdout.write(require("node:crypto").createHash("sha256").update(process.argv[1]).digest("hex").slice(0,16))' "${PAIR_ID}")"
repair_marker="PAIR_REPAIR_$(node -e 'process.stdout.write(require("node:crypto").createHash("sha256").update(process.argv[1]).digest("hex").slice(0,16))' "${PAIR_ID}")"

run_side() {
  local side="$1"
  local deployment="$2"
  local role="generation-${side}"
  local evidence_dir="${PAIR_DIR}/${side}"
  local status_file="${PAIR_DIR}/${side}-runner-status.json"
  local exit_code=0
  # next-app Build completion is Draft-only and must be accepted through its
  # durable Draft Preview. fumadocs-docs still uses the legacy Candidate/Version
  # lifecycle, where run.complete atomically promotes the staged artifact.
  local -a runner_env=(
    "GENERATION_REAL_DRAFT_PREVIEW_ACCEPTANCE=$([[ "${fixture_kind}" == "website" ]] && printf 1 || printf 0)"
    "GENERATION_REAL_DRAFT_WARM_EDIT_CANARY=0"
    "GENERATION_REAL_DRAFT_COLD_DEV_EDIT_CANARY=0"
    "GENERATION_REAL_REPAIR_CANARY=0"
  )
  if [[ "${RUNTIME_RESTART_CANARY}" == "1" ]]; then
    runner_env+=("GENERATION_REAL_KEEP_SANDBOX=true")
  fi
  if [[ "${BUCKET}" == "warm_copy_css" ]]; then
    runner_env+=(
      "GENERATION_REAL_DRAFT_WARM_EDIT_CANARY=1"
      "GENERATION_REAL_DRAFT_WARM_EDIT_KIND=copy_css"
      "GENERATION_REAL_WARM_EDIT_MARKER=${warm_marker}"
    )
  elif [[ "${BUCKET}" == "warm_structural" ]]; then
    runner_env+=(
      "GENERATION_REAL_DRAFT_WARM_EDIT_CANARY=1"
      "GENERATION_REAL_DRAFT_WARM_EDIT_KIND=structural"
      "GENERATION_REAL_WARM_EDIT_MARKER=${warm_marker}"
    )
  elif [[ "${BUCKET}" == "cold_dev" ]]; then
    runner_env+=(
      "GENERATION_REAL_DRAFT_COLD_DEV_EDIT_CANARY=1"
      "GENERATION_REAL_WARM_EDIT_MARKER=${warm_marker}"
    )
  elif [[ "${BUCKET}" == "repair" ]]; then
    runner_env+=(
      "GENERATION_REAL_REPAIR_CANARY=1"
      "GENERATION_REAL_REPAIR_MARKER=${repair_marker}"
    )
  fi
  if [[ "${NONVISUAL_REFERENCE_CANARY}" == "1" ]]; then
    runner_env+=("GENERATION_REAL_NONVISUAL_REFERENCE=true")
    if [[ "${side}" == "candidate" ]]; then
      runner_env+=("GENERATION_REAL_EXPECT_NONVISUAL_UNAVAILABLE=true")
    fi
  fi
  if [[ "${MULTIMODAL_REFERENCE_CANARY}" == "1" ]]; then
    runner_env+=("GENERATION_REAL_MULTIMODAL_REFERENCE=true")
    if [[ "${side}" == "candidate" ]]; then
      runner_env+=("GENERATION_REAL_EXPECT_MULTIMODAL_DELIVERED=true")
    fi
  fi

  set +e
  env \
    "GENERATION_REAL_PREPARED_SESSION_DIR=${SESSION_DIR}" \
    "GENERATION_REAL_CLUSTER=${context#k3d-}" \
    "GENERATION_REAL_WORKSPACE_NAMESPACE=${workspace_namespace}" \
    "GENERATION_REAL_RUNTIME_DEPLOYMENT=${deployment}" \
    "GENERATION_REAL_RUNTIME_SERVICE=${deployment}" \
    "GENERATION_REAL_RUNTIME_ROLE=${role}" \
    "GENERATION_REAL_CASES_FILE=${CASES_FILE}" \
    "GENERATION_REAL_CASE_IDS=${CASE_ID}" \
    "GENERATION_REAL_MAX_CASE_ATTEMPTS=${MAX_CASE_ATTEMPTS}" \
    "GENERATION_REAL_CASE_RETRY_COOLDOWN_MS=0" \
    "GENERATION_REAL_EVIDENCE_DIR=${evidence_dir}" \
    "${runner_env[@]}" \
    bash "${RUNNER}"
  exit_code=$?
  set -e

  node - "${status_file}" "${side}" "${exit_code}" <<'NODE'
const fs = require("node:fs");
const [file, side, exitCode] = process.argv.slice(2);
fs.writeFileSync(file, `${JSON.stringify({
  schemaVersion: "generation-context-pair-runner-status@1",
  side,
  exitCode: Number(exitCode),
  finishedAt: new Date().toISOString(),
}, null, 2)}\n`, { flag: "wx", mode: 0o600 });
NODE

  if [[ "${RUNTIME_RESTART_CANARY}" == "1" ]]; then
    [[ "${exit_code}" == "0" ]] || {
      printf 'generation_cohort_pair.runtime_restart_base_run_failed: side=%s exit=%s\n' \
        "${side}" "${exit_code}" >&2
      exit 3
    }
    local side_case_evidence=""
    side_case_evidence="$(find "${evidence_dir}" -type f -name "real-provider-case-${CASE_ID}.json" -print)"
    [[ "$(printf '%s\n' "${side_case_evidence}" | sed '/^$/d' | wc -l | tr -d ' ')" == "1" ]] || {
      printf 'generation_cohort_pair.runtime_restart_case_evidence_count_invalid: side=%s\n' "${side}" >&2
      exit 3
    }
    bash "${RUNTIME_RESTART_RUNNER}" \
      "${SESSION_DIR}" \
      "${side}" \
      "${deployment}" \
      "${side_case_evidence}" \
      "${PAIR_DIR}/${side}/runtime-restart-evidence.json"
  fi
}

run_side control "${control_deployment}"
run_side candidate "${candidate_deployment}"

control_evidence="$(find "${PAIR_DIR}/control" -type f -name "real-provider-case-${CASE_ID}.json" -print)"
candidate_evidence="$(find "${PAIR_DIR}/candidate" -type f -name "real-provider-case-${CASE_ID}.json" -print)"
[[ "$(printf '%s\n' "${control_evidence}" | sed '/^$/d' | wc -l | tr -d ' ')" == "1" ]] || {
  printf 'generation_cohort_pair.control_evidence_count_invalid\n' >&2
  exit 3
}
[[ "$(printf '%s\n' "${candidate_evidence}" | sed '/^$/d' | wc -l | tr -d ' ')" == "1" ]] || {
  printf 'generation_cohort_pair.candidate_evidence_count_invalid\n' >&2
  exit 3
}

selection_source="runs"
selection_phase="build"
if [[ "${BUCKET}" == warm_* ]]; then
  selection_source="warmEdit"
  selection_phase="edit"
elif [[ "${BUCKET}" == "cold_dev" ]]; then
  selection_source="coldDevEdit"
  selection_phase="edit"
elif [[ "${BUCKET}" == "repair" ]]; then
  selection_source="repair"
  selection_phase="repair"
fi

node - "${SPEC_FILE}" "${PAIR_DIR}" "${PAIR_ID}" "${BATCH_ID}" "${BUCKET}" \
  "${selection_source}" "${selection_phase}" "${control_evidence}" "${candidate_evidence}" \
  "${CASES_FILE}" "${CASE_ID}" "${RUNTIME_RESTART_CANARY}" "${NONVISUAL_REFERENCE_CANARY}" \
  "${MULTIMODAL_REFERENCE_CANARY}" <<'NODE'
const fs = require("node:fs");
const path = require("node:path");
const [file, pairDir, pairId, batchId, bucket, source, phase, controlFile, candidateFile, casesFile, caseId, runtimeRestart, nonvisualReference, multimodalReference] = process.argv.slice(2);
const fixture = JSON.parse(fs.readFileSync(casesFile, "utf8")).cases.find(item => item.id === caseId);
const coverage = fixture.kind === "docs" ? ["fumadocsTemplate"] : ["nextTemplate"];
if (runtimeRestart === "1") coverage.push("runtimeRestart");
if (nonvisualReference === "1") coverage.push("nonVisualUnavailableMainTaskPassed");
if (multimodalReference === "1") coverage.push("multimodalVisualDelivered");
const selection = (side, evidenceFile) => ({
  source,
  phase,
  evidenceFile: path.relative(pairDir, evidenceFile),
  ...(runtimeRestart === "1"
    ? { restartEvidenceFile: `${side}/runtime-restart-evidence.json` }
    : {}),
});
const spec = {
  schemaVersion: "generation-context-real-provider-pair-spec@1",
  pairId,
  batchId,
  bucket,
  control: selection("control", controlFile),
  candidate: selection("candidate", candidateFile),
  coverage,
};
fs.writeFileSync(file, `${JSON.stringify(spec, null, 2)}\n`, { flag: "wx", mode: 0o600 });
NODE

node "${ROOT_DIR}/services/runtime/scripts/collect-generation-context-paired-sample.mjs" \
  "${SESSION_FILE}" "${LEDGER_FILE}" "${SPEC_FILE}"

node - "${RESULT_FILE}" "${PAIR_ID}" "${LEDGER_FILE}" <<'NODE'
const fs = require("node:fs");
const [file, pairId, ledgerFile] = process.argv.slice(2);
const samples = fs.readFileSync(ledgerFile, "utf8").split(/\r?\n/).filter(Boolean)
  .map(line => JSON.parse(line)).filter(record => record.kind === "sample" && record.payload?.pairId === pairId)
  .map(record => ({ side: record.payload.side, status: record.payload.status, recordHash: record.recordHash }));
if (samples.length !== 2 || new Set(samples.map(item => item.side)).size !== 2) {
  throw new Error("paired append verification failed");
}
fs.writeFileSync(file, `${JSON.stringify({
  schemaVersion: "generation-context-paired-pair-result@1",
  pairId,
  appendedAt: new Date().toISOString(),
  samples,
}, null, 2)}\n`, { flag: "wx", mode: 0o600 });
NODE

printf 'Generation Context paired sample appended: pair=%s result=%s\n' "${PAIR_ID}" "${RESULT_FILE}"
