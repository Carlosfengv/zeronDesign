#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
NAMESPACE="${PROVIDER_GATEWAY_NAMESPACE:-provider-system}"
DEPLOYMENT="${PROVIDER_GATEWAY_DEPLOYMENT:-provider-gateway}"
IMAGE="${PROVIDER_GATEWAY_IMAGE:?PROVIDER_GATEWAY_IMAGE must name an image already imported into k3d}"
RESOURCE_CONFIG="${PROVIDER_GATEWAY_RESOURCE_CONFIG:-${ROOT_DIR}/infra/provider-gateway/model-resources.deepseek-v4-pro.json}"
RUNTIME_NAMESPACE="${RUNTIME_NAMESPACE:-anydesign-runtime}"
RUNTIME_DEPLOYMENT="${RUNTIME_DEPLOYMENT:-anydesign-runtime}"
MIGRATION_POD="provider-gateway-sqlite-migration"

for required in sqlite3 "${KUBECTL}" node openssl; do
  command -v "${required}" >/dev/null || {
    printf 'required command is unavailable: %s\n' "${required}" >&2
    exit 2
  }
done

"${KUBECTL}" get namespace "${NAMESPACE}" >/dev/null
node - "${RESOURCE_CONFIG}" <<'NODE'
const fs = require("node:fs");
const config = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const resource = config.resources?.find(item => item.id === "deepseek-v4-pro");
if (!resource || resource.revision !== 4 || resource.physicalModel !== "deepseek-v4-pro") {
  throw new Error("authoritative deepseek-v4-pro revision 4 resource is required");
}
if (resource.auth?.secretRef !== "file:/var/run/secrets/deepseek/api-key") {
  throw new Error("authoritative resource must use the mounted file secret reference");
}
if (config.resources.length !== 1 || config.policies?.length !== 1) {
  throw new Error("authoritative local configuration must contain exactly one resource and policy");
}
if (resource.defaults?.requestTimeoutMs !== 120000 || resource.defaults?.maxAttempts !== 3) {
  throw new Error("authoritative resource must use the 120s per-attempt timeout and three attempts");
}
NODE
"${KUBECTL}" create configmap provider-gateway-model-resources -n "${NAMESPACE}" \
  --from-file="model-resources.json=${RESOURCE_CONFIG}" \
  --dry-run=client -o yaml | "${KUBECTL}" apply -f - >/dev/null
"${KUBECTL}" get secret deepseek-v4-pro-api -n "${NAMESPACE}" >/dev/null
"${KUBECTL}" get secret provider-gateway-runtime-auth -n "${NAMESPACE}" >/dev/null
if ! "${KUBECTL}" get secret provider-gateway-admin-auth -n "${NAMESPACE}" >/dev/null 2>&1; then
  (
    umask 077
    admin_token_file="$(mktemp)"
    trap 'rm -f "${admin_token_file}"' EXIT
    openssl rand -hex 32 >"${admin_token_file}"
    "${KUBECTL}" create secret generic provider-gateway-admin-auth -n "${NAMESPACE}" \
      --from-file="token=${admin_token_file}" \
      --dry-run=client -o yaml | "${KUBECTL}" apply -f - >/dev/null
  )
fi
"${KUBECTL}" apply -f "${ROOT_DIR}/infra/provider-gateway/k3d-sqlite-pvc.yaml" >/dev/null

work_dir="$(mktemp -d)"
runtime_replicas="$("${KUBECTL}" get deployment "${RUNTIME_DEPLOYMENT}" \
  -n "${RUNTIME_NAMESPACE}" -o jsonpath='{.spec.replicas}' 2>/dev/null || true)"
runtime_scaled=false
cleanup() {
  "${KUBECTL}" delete pod "${MIGRATION_POD}" -n "${NAMESPACE}" \
    --ignore-not-found=true --wait=false >/dev/null 2>&1 || true
  if [[ "${runtime_scaled}" == "true" && -n "${runtime_replicas}" ]]; then
    "${KUBECTL}" scale deployment "${RUNTIME_DEPLOYMENT}" -n "${RUNTIME_NAMESPACE}" \
      --replicas="${runtime_replicas}" >/dev/null 2>&1 || true
  fi
  rm -rf "${work_dir}"
}
trap cleanup EXIT

current_volume="$("${KUBECTL}" get deployment "${DEPLOYMENT}" -n "${NAMESPACE}" \
  -o jsonpath='{.spec.template.spec.volumes[?(@.name=="data")].persistentVolumeClaim.claimName}' \
  2>/dev/null || true)"
if [[ "${current_volume}" != "provider-gateway-sqlite" ]]; then
  if [[ -n "${runtime_replicas}" && "${runtime_replicas}" != "0" ]]; then
    "${KUBECTL}" scale deployment "${RUNTIME_DEPLOYMENT}" -n "${RUNTIME_NAMESPACE}" \
      --replicas=0 >/dev/null
    "${KUBECTL}" rollout status deployment/"${RUNTIME_DEPLOYMENT}" \
      -n "${RUNTIME_NAMESPACE}" --timeout=120s >/dev/null
    runtime_scaled=true
  fi

  source_pod="$("${KUBECTL}" get pods -n "${NAMESPACE}" \
    -l app.kubernetes.io/name=provider-gateway \
    -o jsonpath='{.items[?(@.metadata.deletionTimestamp=="")].metadata.name}' \
    | awk '{print $1}')"
  if [[ -n "${source_pod}" ]] && "${KUBECTL}" exec -n "${NAMESPACE}" "${source_pod}" -- \
    sh -lc 'test -s /var/lib/provider-gateway/gateway.db'; then
    mkdir -p "${work_dir}/source" "${work_dir}/backup"
    "${KUBECTL}" cp \
      "${NAMESPACE}/${source_pod}:/var/lib/provider-gateway/gateway.db" \
      "${work_dir}/source/gateway.db"
    for sidecar in gateway.db-wal gateway.db-shm; do
      "${KUBECTL}" cp \
        "${NAMESPACE}/${source_pod}:/var/lib/provider-gateway/${sidecar}" \
        "${work_dir}/source/${sidecar}" >/dev/null 2>&1 || true
    done
    sqlite3 "${work_dir}/source/gateway.db" \
      ".timeout 30000" \
      ".backup '${work_dir}/backup/gateway.db'"
    sqlite3 "${work_dir}/backup/gateway.db" "PRAGMA quick_check;" | grep -qx ok
  fi

  if "${KUBECTL}" get deployment "${DEPLOYMENT}" -n "${NAMESPACE}" >/dev/null 2>&1; then
    "${KUBECTL}" scale deployment "${DEPLOYMENT}" -n "${NAMESPACE}" --replicas=0 >/dev/null
    "${KUBECTL}" rollout status deployment/"${DEPLOYMENT}" \
      -n "${NAMESPACE}" --timeout=120s >/dev/null
  fi

  sed "s|zerondesign/provider-gateway:k3d-local|${IMAGE}|g" \
    "${ROOT_DIR}/infra/provider-gateway/k3d-sqlite-migration-pod.yaml" \
    | "${KUBECTL}" apply -f - >/dev/null
  "${KUBECTL}" wait pod/"${MIGRATION_POD}" -n "${NAMESPACE}" \
    --for=condition=Ready --timeout=120s >/dev/null
  if [[ -s "${work_dir}/backup/gateway.db" ]]; then
    "${KUBECTL}" cp "${work_dir}/backup/gateway.db" \
      "${NAMESPACE}/${MIGRATION_POD}:/var/lib/provider-gateway/gateway.db"
  fi
  "${KUBECTL}" delete pod "${MIGRATION_POD}" -n "${NAMESPACE}" --wait=true >/dev/null
fi

sed "s|zerondesign/provider-gateway:k3d-local|${IMAGE}|g" \
  "${ROOT_DIR}/infra/provider-gateway/k3d-persistent-sqlite-deployment.yaml" \
  | "${KUBECTL}" apply -f - >/dev/null
"${KUBECTL}" rollout status deployment/"${DEPLOYMENT}" \
  -n "${NAMESPACE}" --timeout=300s

"${KUBECTL}" patch deployment "${RUNTIME_DEPLOYMENT}" -n "${RUNTIME_NAMESPACE}" \
  --type=strategic \
  --patch-file "${ROOT_DIR}/infra/agent-sandbox/runtime/provider-gateway-env-patch.yaml" \
  >/dev/null
if [[ "${runtime_scaled}" == "true" ]]; then
  "${KUBECTL}" scale deployment "${RUNTIME_DEPLOYMENT}" -n "${RUNTIME_NAMESPACE}" \
    --replicas="${runtime_replicas}" >/dev/null
  runtime_scaled=false
fi
"${KUBECTL}" rollout status deployment/"${RUNTIME_DEPLOYMENT}" \
  -n "${RUNTIME_NAMESPACE}" --timeout=300s

actual_pvc="$("${KUBECTL}" get deployment "${DEPLOYMENT}" -n "${NAMESPACE}" \
  -o jsonpath='{.spec.template.spec.volumes[?(@.name=="data")].persistentVolumeClaim.claimName}')"
[[ "${actual_pvc}" == "provider-gateway-sqlite" ]] || {
  printf 'provider gateway did not bind the expected PVC\n' >&2
  exit 3
}
"${KUBECTL}" exec -n "${NAMESPACE}" deployment/"${DEPLOYMENT}" -- \
  sh -lc 'test -s /var/lib/provider-gateway/gateway.db'

printf 'Provider Gateway is ready with persistent SQLite storage on PVC provider-gateway-sqlite.\n'
