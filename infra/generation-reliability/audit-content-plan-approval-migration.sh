#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
CONTEXT="${CONTENT_PLAN_MIGRATION_CONTEXT:-k3d-zerondesign-e2e}"
NAMESPACE="${CONTENT_PLAN_MIGRATION_NAMESPACE:-anydesign-runtime}"
DEPLOYMENT="${CONTENT_PLAN_MIGRATION_DEPLOYMENT:-anydesign-runtime}"
POSTGRES_STATEFULSET="${CONTENT_PLAN_MIGRATION_POSTGRES_STATEFULSET:-anydesign-postgres}"
SOURCE_FILE="briefs.jsonl"
SOURCE_PATH="${CONTENT_PLAN_MIGRATION_SOURCE_PATH:-/var/lib/anydesign-runtime/data/${SOURCE_FILE}}"
OUTPUT="${CONTENT_PLAN_MIGRATION_OUTPUT:-${ROOT_DIR}/services/runtime/target/e2e-evidence/${CONTEXT#k3d-}/content-plan-approval-migration.json}"
AUDITOR="${ROOT_DIR}/services/runtime/scripts/audit-content-plan-approval-migration.mjs"

for command in "${KUBECTL}" node date; do
  command -v "${command}" >/dev/null || {
    printf 'content_plan_approval_migration.missing_command: %s\n' "${command}" >&2
    exit 2
  }
done

kube() {
  "${KUBECTL}" --context "${CONTEXT}" "$@"
}

kube get namespace "${NAMESPACE}" >/dev/null
kube get deployment "${DEPLOYMENT}" -n "${NAMESPACE}" >/dev/null
kube get statefulset "${POSTGRES_STATEFULSET}" -n "${NAMESPACE}" >/dev/null

runtime_pod="$(kube get pods -n "${NAMESPACE}" \
  -l app=anydesign-runtime,anydesign.io/runtime-role=primary -o json \
  | node -e '
let input = "";
process.stdin.on("data", chunk => input += chunk).on("end", () => {
  const pod = JSON.parse(input).items
    .filter(item => !item.metadata.deletionTimestamp)
    .find(item => item.status.conditions?.some(condition =>
      condition.type === "Ready" && condition.status === "True"));
  if (!pod) process.exit(2);
  process.stdout.write(pod.metadata.name);
});
')"

source_meta="$(kube exec -n "${NAMESPACE}" "statefulset/${POSTGRES_STATEFULSET}" -- sh -lc \
  'psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -AtF "|" -c "select file_path,content_sha256,revision,updated_at from runtime_control_plane_files where file_path='"'"'briefs.jsonl'"'"'"')"
IFS='|' read -r source_file source_sha256 source_revision source_updated_at <<<"${source_meta}"
[[ "${source_file}" == "${SOURCE_FILE}" && "${source_sha256}" =~ ^[0-9a-f]{64}$ \
  && "${source_revision}" =~ ^[0-9]+$ && -n "${source_updated_at}" ]] || {
  printf 'content_plan_approval_migration.invalid_source_metadata\n' >&2
  exit 2
}

recorded_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
authority="runtime-control-plane-postgres:${CONTEXT}/${NAMESPACE}/runtime_control_plane_files"

kube exec -n "${NAMESPACE}" "pod/${runtime_pod}" -- cat "${SOURCE_PATH}" \
  | node "${AUDITOR}" \
      "--authority=${authority}" \
      "--source-file=${SOURCE_FILE}" \
      "--source-revision=${source_revision}" \
      "--source-updated-at=${source_updated_at}" \
      "--recorded-at=${recorded_at}" \
      "--source-sha256=${source_sha256}" \
      "--source-complete=true" \
      "--output=${OUTPUT}"

printf 'Content Plan Approval migration inventory audited: sourceRevision=%s evidence=%s\n' \
  "${source_revision}" "${OUTPUT}"
