#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
K3D="${K3D:-k3d}"
cluster_name="${ANYDESIGN_E2E_CLUSTER:-zerondesign-e2e}"
runtime_port="${RUNTIME_RC_PORT:-18080}"
workspace_namespace="${RUNTIME_RC_WORKSPACE_NAMESPACE:-ws-runtime-rc}"
website_workspace_namespace="${RUNTIME_RC_WEBSITE_WORKSPACE_NAMESPACE:-${workspace_namespace}}"
docs_workspace_namespace="${RUNTIME_RC_DOCS_WORKSPACE_NAMESPACE:-${workspace_namespace}}"
concurrent_workspace_gate="${RUNTIME_RC_CONCURRENT_WORKSPACE_GATE:-0}"
cd "${ROOT_DIR}"

for candidate_namespace in \
  "${workspace_namespace}" "${website_workspace_namespace}" "${docs_workspace_namespace}"; do
  if [[ ! "${candidate_namespace}" =~ ^ws-[a-z0-9]([a-z0-9-]*[a-z0-9])?$ ]] \
    || (( ${#candidate_namespace} > 63 )); then
    printf 'Runtime RC Workspace values must be ws-* Kubernetes Namespaces\n' >&2
    exit 2
  fi
done
if [[ "${concurrent_workspace_gate}" != "0" && "${concurrent_workspace_gate}" != "1" ]]; then
  printf 'RUNTIME_RC_CONCURRENT_WORKSPACE_GATE must be 0 or 1\n' >&2
  exit 2
fi

rc_mode="${RUNTIME_RC_MODE:-audit}"
if [[ "${rc_mode}" != "audit" && "${rc_mode}" != "release" ]]; then
  printf 'RUNTIME_RC_MODE must be audit or release\n' >&2
  exit 2
fi
provider_mode="${RUNTIME_RC_PROVIDER_MODE:-fixture}"
if [[ "${provider_mode}" != "fixture" && "${provider_mode}" != "deepseek" ]]; then
  printf 'RUNTIME_RC_PROVIDER_MODE must be fixture or deepseek\n' >&2
  exit 2
fi
project_filter="${RUNTIME_RC_PROJECT_FILTER:-all}"
if [[ "${project_filter}" != "all" && "${project_filter}" != "website" && "${project_filter}" != "docs" ]]; then
  printf 'RUNTIME_RC_PROJECT_FILTER must be all, website, or docs\n' >&2
  exit 2
fi
if [[ "${rc_mode}" == "release" && "${project_filter}" != "all" ]]; then
  printf 'release mode requires RUNTIME_RC_PROJECT_FILTER=all\n' >&2
  exit 2
fi
if [[ "${provider_mode}" == "deepseek" && -z "${DEEPSEEK_API_KEY:-}" ]]; then
  printf 'DEEPSEEK_API_KEY is required for the DeepSeek provider gate\n' >&2
  exit 2
fi
real_provider_total_token_ceiling="${RUNTIME_RC_REAL_TOTAL_TOKEN_CEILING:-5000000}"
if [[ "${provider_mode}" == "deepseek" ]]; then
  if [[ ! "${real_provider_total_token_ceiling}" =~ ^[0-9]+$ ]] \
    || (( real_provider_total_token_ceiling < 1 || real_provider_total_token_ceiling > 5000000 )); then
    printf 'RUNTIME_RC_REAL_TOTAL_TOKEN_CEILING must be an integer from 1 to 5000000\n' >&2
    exit 2
  fi
fi
real_provider_reserved_runs=0
real_provider_per_run_safety_ceiling=0
reserve_real_provider_run() {
  local phase="$1"
  local project_id="$2"
  local next_reserved_runs next_reserved_tokens
  [[ "${active_provider_mode:-fixture}" == "deepseek" ]] || return 0
  next_reserved_runs=$((real_provider_reserved_runs + 1))
  next_reserved_tokens=$((next_reserved_runs * real_provider_per_run_safety_ceiling))
  if (( next_reserved_tokens > real_provider_total_token_ceiling )); then
    printf 'real Provider total token reservation exhausted before %s: project=%s runs=%s reserved=%s ceiling=%s\n' \
      "${phase}" "${project_id}" "${next_reserved_runs}" "${next_reserved_tokens}" \
      "${real_provider_total_token_ceiling}" >&2
    return 2
  fi
  real_provider_reserved_runs="${next_reserved_runs}"
}
if [[ "${RUNTIME_RC_TOKEN_BUDGET_SELF_TEST:-0}" == "1" ]]; then
  active_provider_mode=deepseek
  real_provider_per_run_safety_ceiling=240000
  reserve_real_provider_run first self-test
  if reserve_real_provider_run second self-test 2>/dev/null; then
    printf 'real Provider token reservation self-test did not fail closed\n' >&2
    exit 2
  fi
  printf 'Runtime RC real Provider token reservation self-test passed\n'
  exit 0
fi
if [[ "${rc_mode}" == "release" && "${provider_mode}" != "deepseek" ]]; then
  printf 'release mode requires RUNTIME_RC_PROVIDER_MODE=deepseek\n' >&2
  exit 2
fi
evidence_dir="${RUNTIME_RC_EVIDENCE_DIR:-services/runtime/target/e2e-evidence/${cluster_name}}"
mkdir -p "${evidence_dir}"
preflight_evidence="${evidence_dir}/preflight.json"
if [[ "${RUNTIME_RC_SKIP_PREFLIGHT:-0}" == "1" ]]; then
  if [[ "${rc_mode}" == "release" ]]; then
    printf 'RUNTIME_RC_SKIP_PREFLIGHT is not allowed in release mode\n' >&2
    exit 2
  fi
  node - "${preflight_evidence}" infra/agent-sandbox/images.lock.json <<'NODE'
const crypto = require("node:crypto");
const fs = require("node:fs");
const [out, lockFile] = process.argv.slice(2);
const lockRaw = fs.readFileSync(lockFile);
fs.writeFileSync(out, `${JSON.stringify({
  schemaVersion: "runtime-rc-preflight-skipped@1",
  recordedAt: new Date().toISOString(),
  lockHash: crypto.createHash("sha256").update(lockRaw).digest("hex"),
  prefetchImages: false,
  entries: [],
  passed: false,
  skipped: true,
  reason: "explicit-audit-skip",
  errors: [],
}, null, 2)}\n`);
NODE
  printf 'Skipping preflight in audit mode by explicit request\n'
else
  if [[ "${rc_mode}" == "release" ]]; then
    PREFLIGHT_PREFETCH_IMAGES=1 PREFLIGHT_EVIDENCE_PATH="${preflight_evidence}" \
      bash infra/agent-sandbox/preflight-runtime-rc.sh
  else
    PREFLIGHT_PREFETCH_IMAGES=0 PREFLIGHT_EVIDENCE_PATH="${preflight_evidence}" \
      bash infra/agent-sandbox/preflight-runtime-rc.sh
  fi
fi

git_sha="$(git rev-parse --short=12 HEAD)"
git_full_sha="$(git rev-parse HEAD)"
dirty_count="$(git status --porcelain | wc -l | tr -d ' ')"
if [[ "${rc_mode}" == "release" && "${dirty_count}" != "0" ]]; then
  printf 'release mode requires a clean worktree; dirty files=%s\n' "${dirty_count}" >&2
  exit 2
fi
cluster_exists=false
if "${K3D}" cluster list --no-headers 2>/dev/null | awk '{print $1}' | rg -Fxq "${cluster_name}"; then
  cluster_exists=true
fi
if [[ "${rc_mode}" == "release" && "${cluster_exists}" == "true" ]]; then
  printf 'release mode requires a new cluster; k3d cluster already exists: %s\n' \
    "${cluster_name}" >&2
  exit 2
fi
if [[ "${cluster_exists}" == "false" ]]; then
  ANYDESIGN_E2E_CLUSTER="${cluster_name}" \
    ANYDESIGN_E2E_NAMESPACE="${workspace_namespace}" \
    E2E_EVIDENCE_DIR="${evidence_dir}" \
    bash infra/agent-sandbox/run-k8s-e2e.sh
else
  "${KUBECTL}" config use-context "k3d-${cluster_name}" >/dev/null
fi
context="$(${KUBECTL} config current-context)"
if [[ "${context}" != "k3d-${cluster_name}" ]]; then
  printf 'RC gate requires context k3d-%s; got %s\n' "${cluster_name}" "${context}" >&2
  exit 2
fi
for required in \
  secret/anydesign-workspace-channel-signer \
  deployment/anydesign-npm-proxy; do
  "${KUBECTL}" get "${required}" -n anydesign-runtime >/dev/null
done
for candidate_namespace in $(printf '%s\n' \
  "${website_workspace_namespace}" "${docs_workspace_namespace}" | sort -u); do
  "${KUBECTL}" get serviceaccount anydesign-runtime \
    -n anydesign-runtime >/dev/null
  "${KUBECTL}" get serviceaccount anydesign-sandbox \
    -n "${candidate_namespace}" >/dev/null
done
for candidate_namespace in $(printf '%s\n' \
  "${website_workspace_namespace}" "${docs_workspace_namespace}" | sort -u); do
  for authorization in \
    'get sandboxes.agents.x-k8s.io' \
    'get sandboxtemplates.extensions.agents.x-k8s.io' \
    'get sandboxwarmpools.extensions.agents.x-k8s.io' \
    'create sandboxclaims.extensions.agents.x-k8s.io'; do
    verb="${authorization%% *}"
    resource="${authorization#* }"
    allowed="$("${KUBECTL}" auth can-i "${verb}" "${resource}" \
      -n "${candidate_namespace}" \
      --as=system:serviceaccount:anydesign-runtime:anydesign-runtime)"
    if [[ "${allowed}" != "yes" ]]; then
      printf 'Runtime ServiceAccount lacks required Kubernetes permission in %s: %s %s\n' \
        "${candidate_namespace}" "${verb}" "${resource}" >&2
      exit 3
    fi
  done
  existing_rc_claims="$(${KUBECTL} get sandboxclaims -n "${candidate_namespace}" -o name 2>/dev/null \
    | rg '/project-rc-' || true)"
  if [[ -n "${existing_rc_claims}" ]]; then
    if [[ "${rc_mode}" == "release" ]]; then
      printf 'release mode requires zero pre-existing RC SandboxClaims in %s:\n%s\n' \
        "${candidate_namespace}" "${existing_rc_claims}" >&2
      exit 2
    fi
    printf '%s\n' "${existing_rc_claims}" \
      | xargs "${KUBECTL}" delete -n "${candidate_namespace}" --ignore-not-found=true >/dev/null
  fi
done
channel_evidence="${RUNTIME_RC_CHANNEL_EVIDENCE:-${evidence_dir}/k3d-channel.json}"
if [[ ! -s "${channel_evidence}" ]]; then
  printf 'workspace channel evidence is required: %s\n' "${channel_evidence}" >&2
  exit 2
fi

lock_hash="$(shasum -a 256 infra/agent-sandbox/images.lock.json | awk '{print $1}')"
build_timestamp="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
image_tag="${git_sha}"
dirty_flag=false
if [[ "${dirty_count}" != "0" ]]; then
  fingerprint="$({
    git diff --binary
    git ls-files --others --exclude-standard | sort | while IFS= read -r file; do
      shasum -a 256 "${file}"
    done
  } | shasum -a 256 | awk '{print substr($1, 1, 12)}')"
  image_tag="${git_sha}-dirty-${fingerprint}"
  dirty_flag=true
fi
runtime_image="anydesign/runtime:${image_tag}"
admin_token="$(openssl rand -hex 32)"
principal_id="rc-harness-principal"
principal_key_dir="$(mktemp -d)"
principal_private_key="${principal_key_dir}/private.pem"
principal_private_key_der="${principal_key_dir}/private.der"
principal_public_key="${principal_key_dir}/public.der"
port_forward_pid=""
gate_id="$(date +%s)"
provider_secret_created=false
provider_secret_file=""
dcp_runtime_flags_enabled=false
gate_work_dir="$(mktemp -d)"
active_recovery_evidence="${evidence_dir}/active-recovery.json"
recovery_baseline_file="${gate_work_dir}/recovery-baseline.json"
rm -f "${active_recovery_evidence}"
"${KUBECTL}" get sandboxclaims.extensions.agents.x-k8s.io \
  -n "${workspace_namespace}" -o json \
  | node -e '
const fs = require("node:fs");
const claims = JSON.parse(fs.readFileSync(0, "utf8")).items;
const claimNames = claims.map(claim => claim.metadata.name).sort();
const protectedSandboxNames = claims
  .map(claim => claim.status?.sandbox?.name)
  .filter(Boolean)
  .sort();
fs.writeFileSync(process.argv[1], `${JSON.stringify({
  schemaVersion: "runtime-recovery-baseline@1",
  claimNames,
  protectedSandboxNames,
}, null, 2)}\n`);
' "${recovery_baseline_file}"
cleanup() {
  if [[ -n "${port_forward_pid}" ]]; then
    kill "${port_forward_pid}" >/dev/null 2>&1 || true
  fi
  if [[ "${provider_secret_created}" == "true" ]]; then
    "${KUBECTL}" set env deployment/anydesign-runtime -n anydesign-runtime \
      DEEPSEEK_API_KEY- AGENT_MODEL- MODEL_STREAMING- \
      MODEL_PROVIDER=internal_gateway >/dev/null 2>&1 || true
    "${KUBECTL}" delete secret anydesign-runtime-provider -n anydesign-runtime \
      --ignore-not-found=true >/dev/null 2>&1 || true
    "${KUBECTL}" rollout status deployment/anydesign-runtime -n anydesign-runtime \
      --timeout=120s >/dev/null 2>&1 || true
  fi
  if [[ "${dcp_runtime_flags_enabled}" == "true" ]]; then
    "${KUBECTL}" set env deployment/anydesign-runtime -n anydesign-runtime \
      RUNTIME_DESIGN_CONTEXT_PACKAGE_V1- \
      RUNTIME_DESIGN_CONTEXT_ENFORCEMENT_V1- \
      RUNTIME_DESIGN_CONTEXT_ENFORCEMENT_ALLOWLIST_JSON- >/dev/null 2>&1 || true
    "${KUBECTL}" rollout status deployment/anydesign-runtime -n anydesign-runtime \
      --timeout=120s >/dev/null 2>&1 || true
  fi
  [[ -z "${provider_secret_file}" ]] || rm -f "${provider_secret_file}"
  if [[ -n "${gate_id}" ]]; then
    for candidate_namespace in $(printf '%s\n' \
      "${website_workspace_namespace}" "${docs_workspace_namespace}" | sort -u); do
      "${KUBECTL}" get sandboxclaims -n "${candidate_namespace}" -o name 2>/dev/null \
        | rg "/project-rc-(website|docs)(-dcp-enforced(-dcp-provider)?)?-${gate_id}-" \
        | xargs -r "${KUBECTL}" delete -n "${candidate_namespace}" --ignore-not-found=true >/dev/null 2>&1 || true
    done
  fi
  rm -rf "${principal_key_dir}"
  rm -rf "${gate_work_dir}"
}
report_error() {
  local status=$?
  printf 'Runtime RC gate failed: status=%s line=%s command=%s\n' \
    "${status}" "${BASH_LINENO[0]:-unknown}" "${BASH_COMMAND:-unknown}" >&2
  return "${status}"
}
trap report_error ERR
trap cleanup EXIT
if "${KUBECTL}" get secret zerondesign-web-runtime-principal \
  -n anydesign-runtime >/dev/null 2>&1; then
  principal_private_key_base64="$("${KUBECTL}" get secret \
    zerondesign-web-runtime-principal -n anydesign-runtime \
    -o 'jsonpath={.data.private-key-base64}' \
    | node -e 'process.stdin.on("data",d=>process.stdout.write(Buffer.from(d.toString(),"base64")))')"
  node -e '
const {createPrivateKey,createPublicKey}=require("node:crypto");
const {writeFileSync}=require("node:fs");
const privateKey=createPrivateKey({key:Buffer.from(process.argv[1],"base64"),format:"der",type:"pkcs8"});
writeFileSync(process.argv[2],privateKey.export({format:"pem",type:"pkcs8"}));
writeFileSync(process.argv[3],privateKey.export({format:"der",type:"pkcs8"}));
writeFileSync(process.argv[4],createPublicKey(privateKey).export({format:"der",type:"spki"}));
' "${principal_private_key_base64}" "${principal_private_key}" \
    "${principal_private_key_der}" "${principal_public_key}"
else
  node -e '
const {generateKeyPairSync}=require("node:crypto");
const {writeFileSync}=require("node:fs");
const {privateKey,publicKey}=generateKeyPairSync("ed25519");
writeFileSync(process.argv[1],privateKey.export({format:"pem",type:"pkcs8"}));
writeFileSync(process.argv[2],privateKey.export({format:"der",type:"pkcs8"}));
writeFileSync(process.argv[3],publicKey.export({format:"der",type:"spki"}));
' "${principal_private_key}" "${principal_private_key_der}" "${principal_public_key}"
  principal_private_key_base64="$(base64 <"${principal_private_key_der}" | tr -d '\n')"
fi

if [[ -n "${RUNTIME_RC_REUSE_IMAGE:-}" ]]; then
  if [[ "${rc_mode}" == "release" ]]; then
    printf 'RUNTIME_RC_REUSE_IMAGE is not allowed in release mode\n' >&2
    exit 2
  fi
  runtime_image="${RUNTIME_RC_REUSE_IMAGE}"
  docker image inspect "${runtime_image}" >/dev/null
else
  rm -rf services/runtime/target/docker-vendor
  vendor_log="services/runtime/target/docker-vendor.log"
  if ! cargo vendor \
    --manifest-path services/runtime/Cargo.toml \
    --locked \
    --versioned-dirs \
    services/runtime/target/docker-vendor > /dev/null 2>"${vendor_log}"; then
    cat "${vendor_log}" >&2
    exit 1
  fi
  docker build \
    -f services/runtime/Dockerfile \
    --provenance=false \
    --build-arg "REPOSITORY_COMMIT=${git_sha}" \
    --build-arg "REPOSITORY_DIRTY=${dirty_flag}" \
    --build-arg "RUNTIME_IMAGE_REF=${runtime_image}" \
    --build-arg "IMAGE_LOCK_HASH=${lock_hash}" \
    --build-arg "BUILD_TIMESTAMP=${build_timestamp}" \
    -t "${runtime_image}" \
    .
fi
runtime_image_archive="${gate_work_dir}/runtime-image.tar"
docker image save -o "${runtime_image_archive}" "${runtime_image}"
expected_image_id="sha256:$(tar -xOf "${runtime_image_archive}" manifest.json \
  | node -e 'const fs=require("fs");const m=JSON.parse(fs.readFileSync(0,"utf8"));process.stdout.write(m[0].Config.split("/").pop())')"
runtime_manifest_digest="$(tar -xOf "${runtime_image_archive}" index.json \
  | node -e 'const fs=require("fs");const i=JSON.parse(fs.readFileSync(0,"utf8"));const d=i.manifests?.[0]?.digest;if(!/^sha256:[a-f0-9]{64}$/.test(d||""))process.exit(2);process.stdout.write(d)')"
rm -f "${runtime_image_archive}"
"${K3D}" image import --cluster "${cluster_name}" "${runtime_image}"

"${KUBECTL}" create secret generic anydesign-runtime-internal-admin \
  -n anydesign-runtime \
  --from-literal="token=${admin_token}" \
  --dry-run=client -o yaml | "${KUBECTL}" apply -f - >/dev/null
"${KUBECTL}" create secret generic anydesign-runtime-public-principal \
  -n anydesign-runtime \
  --from-file="public.der=${principal_public_key}" \
  --dry-run=client -o yaml | "${KUBECTL}" apply -f - >/dev/null
"${KUBECTL}" create secret generic zerondesign-web-runtime-principal \
  -n anydesign-runtime \
  --from-literal="private-key-base64=${principal_private_key_base64}" \
  --dry-run=client -o yaml | "${KUBECTL}" apply -f - >/dev/null
if "${KUBECTL}" -n anydesign-runtime get secret anydesign-runtime-postgres >/dev/null 2>&1; then
  postgres_password="$("${KUBECTL}" -n anydesign-runtime get secret anydesign-runtime-postgres \
    -o 'jsonpath={.data.password}' | node -e 'process.stdin.on("data",d=>process.stdout.write(Buffer.from(d.toString(),"base64")))')"
else
  postgres_password="$(openssl rand -hex 24)"
fi
postgres_url="postgres://anydesign_runtime:${postgres_password}@anydesign-postgres.anydesign-runtime.svc.cluster.local:5432/anydesign_runtime"
"${KUBECTL}" create secret generic anydesign-runtime-postgres \
  -n anydesign-runtime \
  --from-literal="password=${postgres_password}" \
  --from-literal="url=${postgres_url}" \
  --dry-run=client -o yaml | "${KUBECTL}" apply -f - >/dev/null
"${KUBECTL}" apply -f infra/workspace-provisioner/postgres-control-plane.yaml >/dev/null
"${KUBECTL}" -n anydesign-runtime rollout status statefulset/anydesign-postgres \
  --timeout=180s >/dev/null
if "${KUBECTL}" -n anydesign-runtime get secret anydesign-runtime-object-storage >/dev/null 2>&1; then
  object_storage_access_key="$("${KUBECTL}" -n anydesign-runtime \
    get secret anydesign-runtime-object-storage -o 'jsonpath={.data.access-key}' \
    | node -e 'process.stdin.on("data",d=>process.stdout.write(Buffer.from(d.toString(),"base64")))')"
  object_storage_secret_key="$("${KUBECTL}" -n anydesign-runtime \
    get secret anydesign-runtime-object-storage -o 'jsonpath={.data.secret-key}' \
    | node -e 'process.stdin.on("data",d=>process.stdout.write(Buffer.from(d.toString(),"base64")))')"
else
  object_storage_access_key="zerondesign$(openssl rand -hex 8)"
  object_storage_secret_key="$(openssl rand -hex 24)"
fi
"${KUBECTL}" create secret generic anydesign-runtime-object-storage \
  -n anydesign-runtime \
  --from-literal="access-key=${object_storage_access_key}" \
  --from-literal="secret-key=${object_storage_secret_key}" \
  --dry-run=client -o yaml | "${KUBECTL}" apply -f - >/dev/null
"${KUBECTL}" apply -f infra/workspace-provisioner/object-storage.yaml >/dev/null
"${KUBECTL}" -n anydesign-runtime rollout status statefulset/anydesign-object-store \
  --timeout=240s >/dev/null
object_storage_init_job="anydesign-object-store-init-${gate_id}"
"${KUBECTL}" apply -f - >/dev/null <<EOF
apiVersion: batch/v1
kind: Job
metadata:
  name: ${object_storage_init_job}
  namespace: anydesign-runtime
spec:
  backoffLimit: 3
  ttlSecondsAfterFinished: 300
  template:
    metadata:
      labels:
        app.kubernetes.io/managed-by: zerondesign-object-storage-e2e
    spec:
      restartPolicy: Never
      securityContext:
        runAsUser: 1000
        runAsGroup: 1000
        seccompProfile:
          type: RuntimeDefault
      containers:
        - name: init
          image: quay.io/minio/mc:RELEASE.2025-08-13T08-35-41Z
          command: ["sh", "-ec"]
          args:
            - for attempt in \$(seq 1 60); do if mc alias set local http://anydesign-object-store.anydesign-runtime.svc.cluster.local:9000 "\${MINIO_ACCESS_KEY}" "\${MINIO_SECRET_KEY}" && mc mb --ignore-existing local/anydesign-runtime; then exit 0; fi; sleep 2; done; exit 1
          env:
            - name: HOME
              value: /tmp
            - name: MINIO_ACCESS_KEY
              valueFrom:
                secretKeyRef:
                  name: anydesign-runtime-object-storage
                  key: access-key
            - name: MINIO_SECRET_KEY
              valueFrom:
                secretKeyRef:
                  name: anydesign-runtime-object-storage
                  key: secret-key
          securityContext:
            allowPrivilegeEscalation: false
            capabilities:
              drop: ["ALL"]
EOF
if ! "${KUBECTL}" -n anydesign-runtime wait --for=condition=complete \
  "job/${object_storage_init_job}" --timeout=180s >/dev/null; then
  "${KUBECTL}" -n anydesign-runtime logs "job/${object_storage_init_job}" --all-containers=true >&2 || true
  exit 3
fi
"${KUBECTL}" -n anydesign-runtime delete job "${object_storage_init_job}" \
  --wait=true >/dev/null

"${KUBECTL}" create configmap fixture-model-gateway \
  -n anydesign-runtime \
  --from-file="fixture-model-gateway.js=infra/agent-sandbox/runtime/fixture-model-gateway.js" \
  --dry-run=client -o yaml | "${KUBECTL}" apply -f -
"${KUBECTL}" apply -f infra/agent-sandbox/runtime/fixture-model-gateway.yaml
"${KUBECTL}" set image deployment/fixture-model-gateway -n anydesign-runtime \
  "gateway=${runtime_image}" >/dev/null
sed "s|image: anydesign/runtime:dev|image: ${runtime_image}|" \
  infra/agent-sandbox/runtime/deployment.yaml | "${KUBECTL}" apply -f -
"${KUBECTL}" patch deployment anydesign-runtime -n anydesign-runtime \
  --type=strategic \
  --patch-file infra/agent-sandbox/runtime/fixture-gateway-env-patch.yaml >/dev/null
"${KUBECTL}" set env deployment/anydesign-runtime -n anydesign-runtime \
  RUNTIME_DESIGN_CONTEXT_PACKAGE_V1=true \
  RUNTIME_DESIGN_CONTEXT_ENFORCEMENT_V1=true \
  RUNTIME_DESIGN_CONTEXT_ENFORCEMENT_ALLOWLIST_JSON- >/dev/null
dcp_runtime_flags_enabled=true
configure_deepseek_provider() {
  provider_secret_file="$(mktemp)"
  chmod 600 "${provider_secret_file}"
  printf 'DEEPSEEK_API_KEY=%s\n' "${DEEPSEEK_API_KEY}" >"${provider_secret_file}"
  "${KUBECTL}" create secret generic anydesign-runtime-provider \
    -n anydesign-runtime \
    --from-env-file="${provider_secret_file}" \
    --dry-run=client -o yaml | "${KUBECTL}" apply -f - >/dev/null
  "${KUBECTL}" label secret anydesign-runtime-provider -n anydesign-runtime \
    "anydesign.io/rc-gate-id=${gate_id}" --overwrite >/dev/null
  provider_secret_created=true
  rm -f "${provider_secret_file}"
  provider_secret_file=""
  "${KUBECTL}" set env deployment/anydesign-runtime -n anydesign-runtime \
    --from=secret/anydesign-runtime-provider >/dev/null
  "${KUBECTL}" set env deployment/anydesign-runtime -n anydesign-runtime \
    MODEL_PROVIDER=deepseek AGENT_MODEL="${DEEPSEEK_E2E_MODEL:-deepseek-v4-pro}" \
    MODEL_STREAMING=true \
    MODEL_REQUEST_TIMEOUT_SECONDS="${RUNTIME_RC_MODEL_REQUEST_TIMEOUT_SECONDS:-600}" >/dev/null
}
active_provider_mode="fixture"
if [[ "${provider_mode}" == "deepseek" && "${project_filter}" != "all" ]]; then
  configure_deepseek_provider
  active_provider_mode="deepseek"
fi
"${KUBECTL}" rollout restart deployment/fixture-model-gateway -n anydesign-runtime
"${KUBECTL}" rollout restart deployment/anydesign-runtime -n anydesign-runtime
"${KUBECTL}" rollout status deployment/fixture-model-gateway -n anydesign-runtime --timeout=600s
"${KUBECTL}" rollout status deployment/anydesign-runtime -n anydesign-runtime --timeout=300s
deadline=$((SECONDS + 120))
while (( SECONDS < deadline )); do
  runtime_pods_json="$(${KUBECTL} get pods -n anydesign-runtime -l app=anydesign-runtime -o json)"
  runtime_pod_count="$(node -e '
const pods=JSON.parse(process.argv[1]).items.filter(p=>!p.metadata.deletionTimestamp&&p.status.phase==="Running"&&p.status.conditions?.some(c=>c.type==="Ready"&&c.status==="True"));
process.stdout.write(String(pods.length));
' "${runtime_pods_json}")"
  [[ "${runtime_pod_count}" == "1" ]] && break
  sleep 2
done
[[ "${runtime_pod_count}" == "1" ]] || { printf 'Runtime rollout left multiple serving Pods\n' >&2; exit 3; }

runtime_pod="$(node -e '
const pods=JSON.parse(process.argv[1]).items.filter(p=>!p.metadata.deletionTimestamp&&p.status.phase==="Running"&&p.status.conditions?.some(c=>c.type==="Ready"&&c.status==="True"));
if(pods.length!==1)process.exit(1);process.stdout.write(pods[0].metadata.name);
' "${runtime_pods_json}")"
pod_image="$(${KUBECTL} get pod "${runtime_pod}" -n anydesign-runtime \
  -o jsonpath='{.spec.containers[0].image}')"
pod_image_id="$(${KUBECTL} get pod "${runtime_pod}" -n anydesign-runtime \
  -o jsonpath='{.status.containerStatuses[0].imageID}')"
if [[ "${pod_image}" != "${runtime_image}" || "${pod_image_id}" != "${expected_image_id}" ]]; then
  printf 'Runtime image parity failed: expected=%s id=%s actual=%s id=%s\n' \
    "${runtime_image}" "${expected_image_id}" "${pod_image}" "${pod_image_id}" >&2
  exit 3
fi

base_url="http://127.0.0.1:${runtime_port}"
start_runtime_port_forward() {
  "${KUBECTL}" port-forward -n anydesign-runtime service/anydesign-runtime \
    "${runtime_port}:8080" >/tmp/anydesign-runtime-rc-port-forward.log 2>&1 &
  port_forward_pid=$!
  for _ in $(seq 1 60); do
    if ! kill -0 "${port_forward_pid}" >/dev/null 2>&1; then
      cat /tmp/anydesign-runtime-rc-port-forward.log >&2
      return 1
    fi
    if curl --fail --silent "${base_url}/health" >/dev/null 2>&1; then
      if ! kill -0 "${port_forward_pid}" >/dev/null 2>&1; then
        cat /tmp/anydesign-runtime-rc-port-forward.log >&2
        return 1
      fi
      return 0
    fi
    sleep 1
  done
  printf 'Runtime port-forward did not become ready\n' >&2
  return 1
}

stop_runtime_port_forward() {
  if [[ -n "${port_forward_pid}" ]]; then
    kill "${port_forward_pid}" >/dev/null 2>&1 || true
    wait "${port_forward_pid}" >/dev/null 2>&1 || true
    port_forward_pid=""
  fi
}

start_runtime_port_forward

switch_runtime_provider() {
  local mode="$1"
  case "${mode}" in
    fixture)
      "${KUBECTL}" set env deployment/anydesign-runtime -n anydesign-runtime \
        MODEL_PROVIDER=internal_gateway AGENT_MODEL- MODEL_STREAMING- >/dev/null
      ;;
    deepseek)
      "${KUBECTL}" set env deployment/anydesign-runtime -n anydesign-runtime \
        MODEL_PROVIDER=deepseek AGENT_MODEL="${DEEPSEEK_E2E_MODEL:-deepseek-v4-pro}" \
        MODEL_STREAMING=true \
        MODEL_REQUEST_TIMEOUT_SECONDS="${RUNTIME_RC_MODEL_REQUEST_TIMEOUT_SECONDS:-600}" >/dev/null
      ;;
    *)
      printf 'unsupported Runtime provider switch: %s\n' "${mode}" >&2
      return 2
      ;;
  esac
  "${KUBECTL}" rollout restart deployment/anydesign-runtime -n anydesign-runtime >/dev/null
  "${KUBECTL}" rollout status deployment/anydesign-runtime -n anydesign-runtime --timeout=300s >/dev/null
  stop_runtime_port_forward
  start_runtime_port_forward
  runtime_pod="$(${KUBECTL} get pods -n anydesign-runtime -l app=anydesign-runtime -o json \
    | node -e 'const fs=require("fs");const p=JSON.parse(fs.readFileSync(0,"utf8")).items.find(p=>!p.metadata.deletionTimestamp&&p.status.conditions?.some(c=>c.type==="Ready"&&c.status==="True"));if(!p)process.exit(2);process.stdout.write(p.metadata.name)')"
}

version_json="$(curl --fail --silent "${base_url}/version")"
node infra/agent-sandbox/verify-runtime-version.mjs \
  "${version_json}" "${git_sha}" "${git_full_sha}" "${runtime_image}"

if [[ "${provider_mode}" == "deepseek" ]]; then
  runtime_max_input_tokens="$(${KUBECTL} get deployment anydesign-runtime -n anydesign-runtime \
    -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="RUNTIME_AGENT_MAX_INPUT_TOKENS")].value}')"
  runtime_max_output_tokens="$(${KUBECTL} get deployment anydesign-runtime -n anydesign-runtime \
    -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="RUNTIME_AGENT_MAX_OUTPUT_TOKENS")].value}')"
  if [[ ! "${runtime_max_input_tokens}" =~ ^[0-9]+$ || ! "${runtime_max_output_tokens}" =~ ^[0-9]+$ ]]; then
    printf 'real Provider gate requires numeric Runtime input/output token budgets\n' >&2
    exit 2
  fi
  real_provider_per_run_safety_ceiling=$((runtime_max_input_tokens + runtime_max_output_tokens))
  if (( real_provider_per_run_safety_ceiling > real_provider_total_token_ceiling )); then
    printf 'real Provider per-Run safety ceiling exceeds total token ceiling: perRun=%s total=%s\n' \
      "${real_provider_per_run_safety_ceiling}" "${real_provider_total_token_ceiling}" >&2
    exit 2
  fi
fi

issue_principal_token() {
  local project_id="$1"
  node -e '
const crypto=require("crypto");
const fs=require("fs");
const key=crypto.createPrivateKey(fs.readFileSync(process.argv[1]));
const publicDer=crypto.createPublicKey(key).export({type:"spki",format:"der"});
const kid=`ed25519-${crypto.createHash("sha256").update(publicDer).digest("hex").slice(0,16)}`;
const now=Math.floor(Date.now()/1000);
const encode=value=>Buffer.from(JSON.stringify(value)).toString("base64url");
const header=encode({alg:"EdDSA",typ:"JWT",kid});
const payload=encode({iss:"anydesign-bff",aud:"anydesign-runtime-public",sub:process.argv[3],jti:crypto.randomBytes(16).toString("hex"),iat:now,exp:now+120,projectId:process.argv[2],operations:["preview.read","project.read","project.write"]});
const input=`${header}.${payload}`;
process.stdout.write(`${input}.${crypto.sign(null,Buffer.from(input),key).toString("base64url")}`);
' "${principal_private_key}" "${project_id}" "${principal_id}"
}

configure_enforced_dcp_fixture() {
  local project_id="$1"
  local profile_payload created profile_id profile_version policy_payload policy
  profile_payload="$(node -e '
const projectId=process.argv[1];
const profile={
  status:"active",scope:{projectId},source:{kind:"manual"},
  product:{name:"RC Enforced DCP",category:"runtime release gate",audience:["release operators"],primaryUseCases:["verify deployed website DCP"],productQualities:["deterministic","auditable"]},
  brand:{voice:{tone:["clear","precise"],sentenceStyle:"technical",vocabulary:{prefer:["runtime","evidence"],avoid:["magic"]},writingRules:["Use concrete status text."]},messaging:{headlineStyle:"specific",bodyStyle:"concise",ctaStyle:"verb first",proofStyle:"evidence based",forbiddenClaims:["guaranteed"]}},
  visual:{direction:"quiet operational interface",principles:["scan friendly"],moodKeywords:["calm"],avoidKeywords:["flashy"],composition:{},imagery:{},motion:{}},
  tokens:{color:{},typography:{},radius:{},shadow:{},spacing:{}},
  runtimeTokenMapping:{"color.background":"#ffffff","color.surface":"#f8fafc","color.surfaceStrong":"#e2e8f0","color.text":"#0f172a","color.muted":"#475569","color.primary":"#2563eb","color.primaryContrast":"#ffffff","color.border":"#cbd5e1","radius.card":"8px","radius.control":"6px","font.sans":"Inter, sans-serif","shadow.soft":"0 1px 2px rgba(15, 23, 42, 0.12)"},
  components:{primitives:{button:{intent:"clear action",usage:["primary actions"],avoid:["overuse"]},input:{intent:"precise entry",usage:["forms"],avoid:["placeholder-only labels"]},card:{intent:"group repeated items",usage:["lists"],avoid:["nested cards"]},badge:{intent:"show status",usage:["statuses"],avoid:["decorative noise"]}}},
  websiteContext:{enforcementMode:"enforced",craftPacks:["accessibility-baseline","responsive-layout"]},content:{},accessibility:{},
  technical:{allowedTemplates:["astro-website"],preferredTemplates:{website:"astro-website"},cssStrategy:"runtime-style-contract",dependencyPolicy:{},filePolicy:{designProfilePath:"/workspace/inputs/design-profile.json",designMarkdownPath:"/workspace/inputs/design.md",styleContractPath:"/workspace/state/style-contract.json"}},
  governance:{conflictBehavior:"ask"}
};
process.stdout.write(JSON.stringify({projectId,name:"RC Enforced DCP Profile",profile}));
' "${project_id}")"
  created="$(curl --fail --silent -X POST -H 'content-type: application/json' \
    -d "${profile_payload}" "${base_url}/design-profiles")"
  profile_id="$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).designProfile.id)' "${created}")"
  profile_version="$(node -e 'process.stdout.write(String(JSON.parse(process.argv[1]).designProfile.version))' "${created}")"
  curl --fail --silent -X POST -H 'content-type: application/json' \
    -d "$(node -e 'process.stdout.write(JSON.stringify({designProfileId:process.argv[1]}))' "${profile_id}")" \
    "${base_url}/projects/${project_id}/design-profile" >/dev/null
  policy_payload="$(node -e '
process.stdout.write(JSON.stringify({designProfileId:process.argv[1],designProfileVersion:Number(process.argv[2]),enabled:true,expectedRevision:0,updatedBy:"runtime-rc-enforced-dcp-fixture"}));
' "${profile_id}" "${profile_version}")"
  policy="$(curl --fail --silent -X PUT \
    -H 'content-type: application/json' \
    -H 'x-anydesign-internal: true' \
    -H "x-runtime-admin-token: ${admin_token}" \
    -d "${policy_payload}" \
    "${base_url}/internal/projects/${project_id}/design-context-enforcement")"
  node -e '
const policy=JSON.parse(process.argv[1]).policy;
if(!policy?.enabled||policy.revision!==1||policy.designProfileId!==process.argv[2]||policy.designProfileVersion!==Number(process.argv[3]))process.exit(2);
' "${policy}" "${profile_id}" "${profile_version}"
  node -e '
process.stdout.write(JSON.stringify({designProfileId:process.argv[1],designProfileVersion:Number(process.argv[2]),policyRevision:1},null,2));
' "${profile_id}" "${profile_version}"
}

inject_active_preview_recovery() {
  local project_id="$1"
  local runtime_state="$2"
  local lease_id="$3"
  local binding_id preview_url preview_prefix principal_token
  local old_runtime_pod old_runtime_uid new_runtime_uid recovered_state recovered_status
  local killed_port_forward_pid reconnect_status
  binding_id="$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).sandboxBindingId)' "${runtime_state}")"
  preview_url="${base_url}/previews/${lease_id}/"
  preview_prefix="/projects/${project_id}/previews/${lease_id}"
  principal_token="$(issue_principal_token "${project_id}")"
  recovered_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
    -H "authorization: Bearer ${principal_token}" \
    -H "x-anydesign-preview-prefix: ${preview_prefix}" "${preview_url}")"
  [[ "${recovered_status}" == "200" ]] || {
    printf 'active Preview is unavailable before Runtime restart: %s\n' "${recovered_status}" >&2
    return 1
  }
  old_runtime_pod="$(${KUBECTL} get pods -n anydesign-runtime -l app=anydesign-runtime \
    -o jsonpath='{.items[0].metadata.name}')"
  old_runtime_uid="$(${KUBECTL} get pod "${old_runtime_pod}" -n anydesign-runtime \
    -o jsonpath='{.metadata.uid}')"
  "${KUBECTL}" rollout restart deployment/anydesign-runtime -n anydesign-runtime >/dev/null
  "${KUBECTL}" rollout status deployment/anydesign-runtime -n anydesign-runtime --timeout=300s >/dev/null
  stop_runtime_port_forward
  start_runtime_port_forward
  runtime_pod="$(${KUBECTL} get pods -n anydesign-runtime -l app=anydesign-runtime -o json \
    | node -e 'const fs=require("fs");const p=JSON.parse(fs.readFileSync(0,"utf8")).items.find(p=>!p.metadata.deletionTimestamp&&p.status.conditions?.some(c=>c.type==="Ready"&&c.status==="True"));if(!p)process.exit(2);process.stdout.write(p.metadata.name)')"
  new_runtime_uid="$(${KUBECTL} get pod "${runtime_pod}" -n anydesign-runtime \
    -o jsonpath='{.metadata.uid}')"
  [[ "${new_runtime_uid}" != "${old_runtime_uid}" ]] || {
    printf 'Runtime restart did not replace the serving Pod\n' >&2
    return 1
  }
  recovered_state="$(curl --fail --silent -H "authorization: Bearer ${principal_token}" \
    "${base_url}/projects/${project_id}/runtime-state")"
  node -e '
const before=JSON.parse(process.argv[1]);
const after=JSON.parse(process.argv[2]);
if(after.sandboxBindingId!==process.argv[3]||after.currentVersionId!==before.currentVersionId)process.exit(2);
' "${runtime_state}" "${recovered_state}" "${binding_id}" "${lease_id}"
  principal_token="$(issue_principal_token "${project_id}")"
  recovered_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
    -H "authorization: Bearer ${principal_token}" \
    -H "x-anydesign-preview-prefix: ${preview_prefix}" "${preview_url}")"
  [[ "${recovered_status}" == "200" ]] || {
    printf 'active Preview did not recover after Runtime restart: %s\n' "${recovered_status}" >&2
    return 1
  }

  killed_port_forward_pid="${port_forward_pid}"
  stop_runtime_port_forward
  if kill -0 "${killed_port_forward_pid}" >/dev/null 2>&1; then
    printf 'killed Runtime port-forward process is still active\n' >&2
    return 1
  fi
  start_runtime_port_forward
  principal_token="$(issue_principal_token "${project_id}")"
  reconnect_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
    -H "authorization: Bearer ${principal_token}" \
    -H "x-anydesign-preview-prefix: ${preview_prefix}" "${preview_url}")"
  [[ "${reconnect_status}" == "200" ]] || {
    printf 'active Preview did not recover after port-forward replacement: %s\n' \
      "${reconnect_status}" >&2
    return 1
  }
  node -e '
const fs=require("fs");
fs.writeFileSync(process.argv[1],JSON.stringify({
  runtimeRestart:{scenario:"runtime-restart",injectionPoint:"active-preview-lease",result:"pass",orphanCount:0,oldPodUid:process.argv[2],newPodUid:process.argv[3],bindingId:process.argv[4],previewLeaseId:process.argv[5]},
  portForwardKill:{scenario:"port-forward-kill",injectionPoint:"active-preview-lease",result:"pass",orphanCount:0,killedPid:Number(process.argv[6]),reconnectedHttpStatus:Number(process.argv[7])}
},null,2)+"\n");
' "${active_recovery_evidence}" "${old_runtime_uid}" "${new_runtime_uid}" \
    "${binding_id}" "${lease_id}" "${killed_port_forward_pid}" "${reconnect_status}"
}

capture_dependency_workspace_evidence() {
  local project_json="$1" proxy_host="anydesign-npm-proxy.anydesign-runtime.svc.cluster.local"
  local pod_uid pods_json record project_pod project_ip lock_hash request_count proxy_logs_file
  pod_uid="$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).podUid)' "${project_json}")"
  pods_json="$(${KUBECTL} get pods -n "${workspace_namespace}" -o json)"
  record="$(node -e '
const pods=JSON.parse(process.argv[1]).items;
const pod=pods.find(item=>item.metadata.uid===process.argv[2]&&!item.metadata.deletionTimestamp);
if(!pod||pod.status.phase!=="Running")process.exit(2);
process.stdout.write(`${pod.metadata.name}\t${pod.status.podIP||""}`);
' "${pods_json}" "${pod_uid}")"
  IFS=$'\t' read -r project_pod project_ip <<<"${record}"
  ${KUBECTL} exec -n "${workspace_namespace}" "${project_pod}" -- node -e \
    "require('dns').lookup('${proxy_host}',e=>{if(e)throw e})"
  ${KUBECTL} exec -n "${workspace_namespace}" "${project_pod}" -- node -e \
    "fetch('http://${proxy_host}:4873/-/ping').then(r=>{if(!r.ok)process.exit(2)})"
  if ${KUBECTL} exec -n "${workspace_namespace}" "${project_pod}" -- node -e \
    "const t=setTimeout(()=>process.exit(0),4000);fetch('https://registry.npmjs.org/').then(()=>{clearTimeout(t);process.exit(3)}).catch(()=>{clearTimeout(t);process.exit(0)})"; then
    :
  else
    printf 'direct public npm registry unexpectedly reachable from Runtime project Pod: pod=%s uid=%s\n' \
      "${project_pod}" "${pod_uid}" >&2
    return 1
  fi
  lock_hash="$(${KUBECTL} exec -n "${workspace_namespace}" "${project_pod}" -- sh -lc '
test -d /workspace/project/node_modules || exit 10
for file in /workspace/project/package-lock.json /workspace/project/pnpm-lock.yaml; do
  if [ -f "$file" ]; then sha256sum "$file" | awk "{print \$1}"; exit 0; fi
done
exit 11
')"
  proxy_logs_file="$(mktemp)"
  ${KUBECTL} logs -n anydesign-runtime deployment/anydesign-npm-proxy --since=30m >"${proxy_logs_file}"
  request_count="$(rg -F "\"remoteAddress\":\"${project_ip}\"" "${proxy_logs_file}" \
    | rg -c '/[^" ]+\.tgz' || true)"
  rm -f "${proxy_logs_file}"
  [[ "${request_count:-0}" -ge 1 ]] || {
    printf 'Verdaccio has no tarball request from project Pod %s (%s)\n' \
      "${project_pod}" "${project_ip}" >&2
    return 1
  }
  node -e '
process.stdout.write(JSON.stringify({
  podUid:process.argv[1],pod:process.argv[2],podIp:process.argv[3],nodeModulesPresent:true,
  lockfileSha256:process.argv[4],tarballRequestCount:Number(process.argv[5]),
  networkChecks:{dnsResolved:true,proxyReachable:true,directNpmjsDenied:true},passed:true
}));
' "${pod_uid}" "${project_pod}" "${project_ip}" "${lock_hash}" "${request_count}"
}

run_fixture() {
  set -e
  local project_id="$1"
  local kind="$2"
  local expected_text="$3"
  local output_file="$4"
  local inject_recovery="${5:-false}"
  local repair_expected_text="${6:-${expected_text}}"
  local selected_workspace_namespace="${7:-${workspace_namespace}}"
  local workspace_namespace="${selected_workspace_namespace}"
  local brief_payload brief_run conversation brief_id build_payload build_run events artifact_url
  local build_state base_version_id binding_id edit_payload edit_run release_data project_evidence artifact_status artifact_assertions artifact_assertion_path artifact_assertion_url dependency_evidence
  local dcp_build_diagnostics dcp_edit_diagnostics dcp_repair_diagnostics
  local edit_version_id review_payload review_run review_events finding_id repair_payload repair_run repair_events
  local edit_response edit_status
  local build_preview_lease_id
  local principal_token wrong_project_token anonymous_status cross_project_status preview_url preview_prefix owner_status
  local cancel_probe_payload cancel_probe_run cancel_probe_response cancelled_preview_status
  local stage_timeout brief_wait_timeout brief_text edit_text build_expected_text confirmation_ready seeded_review_provider
  seeded_review_provider=false
  stage_timeout=240
  brief_wait_timeout=120
  brief_text="Create a ${kind} RC fixture"
  edit_text="Apply the deterministic deployed RC edit."
  build_expected_text="RC ${kind} Built"
  if [[ "${active_provider_mode}" == "deepseek" ]]; then
    stage_timeout="${RUNTIME_RC_REAL_STAGE_TIMEOUT_SECONDS:-1800}"
    brief_wait_timeout="${RUNTIME_RC_REAL_BRIEF_WAIT_SECONDS:-720}"
    brief_text="Create a polished but compact ${kind} using the initialized Runtime template. This Build-stage artifact must visibly contain the exact text ${build_expected_text}. Use only Runtime tools, make the source changes, call preview.publish to build and validate the candidate, then call run.complete. Never rewrite an existing large file with fs.write; use fs.patch or fs.multi_patch after reading it."
    edit_text="Edit the current ${kind} by replacing the visible Build-stage text ${build_expected_text} with the exact text ${expected_text}. Make and verify that source mutation before the first preview.publish call. Then call preview.publish to validate the candidate and call run.complete only after it succeeds. Never rewrite an existing large file with fs.write; use fs.patch or fs.multi_patch after reading it."
    if [[ "${kind}" == "docs" ]]; then
      build_expected_text="RC Docs Built"
      brief_text="Create a minimal Docs artifact from the initialized Fumadocs template. Keep existing structure and change only the smallest existing source needed. Do not create a full documentation set and do not submit any write over 2000 characters. This Build-stage Docs artifact must contain the exact text ${build_expected_text}. Use fs.patch or fs.multi_patch after fs.read, then publish and complete."
      edit_text="Before any preview.publish call, make one minimal source patch that replaces the exact text ${build_expected_text} with ${expected_text} on the authored Docs page. Do not rewrite whole files and do not submit any write over 2000 characters. Then publish, verify ${expected_text} at the served Docs route, and complete."
    fi
  fi
  curl --fail --silent -X PUT \
    -H 'content-type: application/json' \
    -H 'x-anydesign-internal: true' \
    -H "x-runtime-admin-token: ${admin_token}" \
    -d "$(node -e 'process.stdout.write(JSON.stringify({ownerPrincipalId:process.argv[1],workspaceNamespace:process.argv[2]}))' "${principal_id}" "${workspace_namespace}")" \
    "${base_url}/internal/projects/${project_id}/access" >/dev/null
  principal_token="$(issue_principal_token "${project_id}")"
  reserve_real_provider_run brief "${project_id}"
  brief_payload="$(curl --fail --silent \
    -H 'content-type: application/json' \
    -H "authorization: Bearer ${principal_token}" \
    -d "$(node -e 'process.stdout.write(JSON.stringify({projectId:process.argv[1],phase:"brief",agentProfile:"brief",inputContext:{contentSources:[{id:"source-1",kind:"prompt",text:process.argv[2],readable:true}]}}))' "${project_id}" "${brief_text}")" \
    "${base_url}/runs")"
  brief_run="$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).runId)' "${brief_payload}")"
  confirmation_ready=false
  for _ in $(seq 1 "${brief_wait_timeout}"); do
    principal_token="$(issue_principal_token "${project_id}")"
    conversation="$(curl --fail --silent -H "authorization: Bearer ${principal_token}" \
      "${base_url}/projects/${project_id}/conversation?includeDebug=true")"
    if [[ "${conversation}" == *'"briefId"'* || "${conversation}" == *'"kind":"approval_request"'* || "${conversation}" == *"Requested brief confirmation"* || "${conversation}" == *"confirmation_requested"* || "${conversation}" == *"Confirm this deterministic"* ]]; then
      confirmation_ready=true
      break
    fi
    sleep 1
  done
  if [[ "${confirmation_ready}" != "true" ]]; then
    printf 'Brief confirmation did not become visible: project=%s run=%s conversation=%s\n' \
      "${project_id}" "${brief_run}" "${conversation}" >&2
    exit 4
  fi
  principal_token="$(issue_principal_token "${project_id}")"
  curl --fail --silent \
    -H 'content-type: application/json' \
    -H "authorization: Bearer ${principal_token}" \
    -d '{"userMessage":"confirm"}' \
    "${base_url}/runs/${brief_run}/continue" >/dev/null
  brief_id=""
  for _ in $(seq 1 120); do
    principal_token="$(issue_principal_token "${project_id}")"
    conversation="$(curl --fail --silent -H "authorization: Bearer ${principal_token}" \
      "${base_url}/projects/${project_id}/conversation?includeDebug=true")"
    brief_id="$(node -e '
const c=JSON.parse(process.argv[1]);
const item=[...c.items].reverse().find(x=>x.metadata&&x.metadata.briefId);
process.stdout.write(item?.metadata?.briefId||"");
' "${conversation}")"
    [[ -z "${brief_id}" ]] || break
    sleep 1
  done
  if [[ -z "${brief_id}" ]]; then
    printf 'briefId did not become visible: project=%s run=%s conversation=%s\n' \
      "${project_id}" "${brief_run}" "${conversation}" >&2
    exit 4
  fi
  principal_token="$(issue_principal_token "${project_id}")"
  reserve_real_provider_run build "${project_id}"
  build_payload="$(curl --fail --silent \
    -H 'content-type: application/json' \
    -H "authorization: Bearer ${principal_token}" \
    -d "{\"projectId\":\"${project_id}\",\"phase\":\"build\",\"agentProfile\":\"build\",\"inputContext\":{\"briefId\":\"${brief_id}\"}}" \
    "${base_url}/runs")"
  build_run="$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).runId)' "${build_payload}")"
  events="$(curl --fail --silent --max-time "${stage_timeout}" \
    -H "authorization: Bearer ${principal_token}" "${base_url}/runs/${build_run}/events")"
  if [[ "${events}" != *'"type":"run.completed"'* || "${events}" != *'"status":"completed"'* ]]; then
    printf 'Build did not complete: project=%s run=%s\n%s\n' "${project_id}" "${build_run}" "${events}" >&2
    exit 4
  fi
  # Provider SSE waits may outlive the deliberately short principal JWT.
  principal_token="$(issue_principal_token "${project_id}")"
  if [[ "${project_id}" == *"dcp-enforced"* ]]; then
    dcp_build_diagnostics="$(curl --fail --silent \
      -H "authorization: Bearer ${principal_token}" \
      "${base_url}/runs/${build_run}/design-context-diagnostics")"
  fi
  build_preview_lease_id="$(node -e '
for(const line of process.argv[1].split(/\r?\n/)){
  if(!line.startsWith("data: "))continue;
  try{const event=JSON.parse(line.slice(6));if(event.type==="preview.updated"){
    const match=new URL(event.url).pathname.match(/^\/previews\/([^/]+)\/?$/);
    if(match){process.stdout.write(match[1]);process.exit(0);}
  }}catch{}
}
process.exit(2);
' "${events}")"
  build_state="$(curl --fail --silent -H "authorization: Bearer ${principal_token}" \
    "${base_url}/projects/${project_id}/runtime-state")"
  base_version_id="$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).currentVersionId)' "${build_state}")"
  binding_id="$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).sandboxBindingId)' "${build_state}")"
  if [[ "${inject_recovery}" == "true" && "${kind}" == "website" && ! -s "${active_recovery_evidence}" ]]; then
    inject_active_preview_recovery "${project_id}" "${build_state}" "${build_preview_lease_id}"
    principal_token="$(issue_principal_token "${project_id}")"
    build_state="$(curl --fail --silent -H "authorization: Bearer ${principal_token}" \
      "${base_url}/projects/${project_id}/runtime-state")"
    base_version_id="$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).currentVersionId)' "${build_state}")"
    binding_id="$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).sandboxBindingId)' "${build_state}")"
  fi
  principal_token="$(issue_principal_token "${project_id}")"
  reserve_real_provider_run edit "${project_id}"
  edit_response="$(curl --silent --show-error --write-out $'\n%{http_code}' \
    -H 'content-type: application/json' \
    -H "authorization: Bearer ${principal_token}" \
    -d "{\"projectId\":\"${project_id}\",\"phase\":\"edit\",\"agentProfile\":\"edit\",\"inputContext\":{\"baseVersionId\":\"${base_version_id}\",\"sandboxBindingId\":\"${binding_id}\"}}" \
    "${base_url}/runs")"
  edit_status="${edit_response##*$'\n'}"
  edit_payload="${edit_response%$'\n'*}"
  if [[ "${edit_status}" != "200" ]]; then
    printf 'Edit start failed: project=%s status=%s body=%s\n' \
      "${project_id}" "${edit_status}" "${edit_payload}" >&2
    exit 4
  fi
  edit_run="$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).runId)' "${edit_payload}")"
  principal_token="$(issue_principal_token "${project_id}")"
  curl --fail --silent \
    -H 'content-type: application/json' \
    -H "authorization: Bearer ${principal_token}" \
    -d "$(node -e 'process.stdout.write(JSON.stringify({userMessage:process.argv[1]}))' "${edit_text}")" \
    "${base_url}/runs/${edit_run}/continue" >/dev/null
  events="$(curl --fail --silent --max-time "${stage_timeout}" \
    -H "authorization: Bearer ${principal_token}" "${base_url}/runs/${edit_run}/events")"
  if [[ "${events}" != *'"type":"run.completed"'* || "${events}" != *'"status":"completed"'* ]]; then
    printf 'Edit did not complete cleanly: project=%s run=%s\n%s\n' "${project_id}" "${edit_run}" "${events}" >&2
    exit 4
  fi
  principal_token="$(issue_principal_token "${project_id}")"
  if [[ "${project_id}" == *"dcp-enforced"* ]]; then
    dcp_edit_diagnostics="$(curl --fail --silent \
      -H "authorization: Bearer ${principal_token}" \
      "${base_url}/runs/${edit_run}/design-context-diagnostics")"
    edit_version_id="$(curl --fail --silent \
      -H "authorization: Bearer ${principal_token}" \
      "${base_url}/projects/${project_id}/runtime-state" \
      | node -e 'const fs=require("fs");process.stdout.write(JSON.parse(fs.readFileSync(0,"utf8")).currentVersionId)')"
    if [[ "${active_provider_mode}" == "deepseek" && "${project_id}" == *"dcp-provider"* ]]; then
      # Seed only the Review finding deterministically. Build/Edit/Repair stay
      # on the real Provider so the gate measures mutation compliance
      # independently from stochastic finding discovery.
      switch_runtime_provider fixture
      seeded_review_provider=true
      principal_token="$(issue_principal_token "${project_id}")"
    fi
    reserve_real_provider_run review "${project_id}"
    review_payload="$(curl --fail --silent \
      -H 'content-type: application/json' \
      -H "authorization: Bearer ${principal_token}" \
      -d "$(node -e 'process.stdout.write(JSON.stringify({projectId:process.argv[1],phase:"review",agentProfile:"visual-review",inputContext:{parentRunId:process.argv[2]}}))' "${project_id}" "${edit_run}")" \
      "${base_url}/runs")"
    review_run="$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).runId)' "${review_payload}")"
    review_events="$(curl --fail --silent --max-time "${stage_timeout}" \
      -H "authorization: Bearer ${principal_token}" "${base_url}/runs/${review_run}/events")"
    if [[ "${review_events}" != *'"type":"run.completed"'* || "${review_events}" != *'"status":"completed"'* ]]; then
      printf 'Review did not complete cleanly: project=%s run=%s\n%s\n' \
        "${project_id}" "${review_run}" "${review_events}" >&2
      exit 4
    fi
    if [[ "${seeded_review_provider}" == "true" ]]; then
      switch_runtime_provider deepseek
    fi
    principal_token="$(issue_principal_token "${project_id}")"
    reserve_real_provider_run repair "${project_id}"
    finding_id="$(node -e '
for(const line of process.argv[1].split(/\r?\n/)){
  if(!line.startsWith("data: "))continue;
  try{const event=JSON.parse(line.slice(6));if(event.type==="review.finding"&&event.severity==="blocking"){
    process.stdout.write(event.findingId);process.exit(0);
  }}catch{}
}
process.exit(2);
' "${review_events}")"
    repair_payload="$(curl --fail --silent \
      -H 'content-type: application/json' \
      -H "authorization: Bearer ${principal_token}" \
      -d "$(node -e 'process.stdout.write(JSON.stringify({projectId:process.argv[1],phase:"repair",agentProfile:"repair",inputContext:{parentRunId:process.argv[2],findingIds:[process.argv[3]]}}))' "${project_id}" "${review_run}" "${finding_id}")" \
      "${base_url}/runs")"
    repair_run="$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).runId)' "${repair_payload}")"
    repair_events="$(curl --fail --silent --max-time "${stage_timeout}" \
      -H "authorization: Bearer ${principal_token}" "${base_url}/runs/${repair_run}/events")"
    if [[ "${repair_events}" != *'"type":"run.completed"'* || "${repair_events}" != *'"status":"completed"'* ]]; then
      printf 'Repair did not complete cleanly: project=%s run=%s finding=%s\n%s\n' \
        "${project_id}" "${repair_run}" "${finding_id}" "${repair_events}" >&2
      exit 4
    fi
    principal_token="$(issue_principal_token "${project_id}")"
    dcp_repair_diagnostics="$(curl --fail --silent \
      -H "authorization: Bearer ${principal_token}" \
      "${base_url}/runs/${repair_run}/design-context-diagnostics")"
    node -e '
const build=JSON.parse(process.argv[1]);
const edit=JSON.parse(process.argv[2]);
const repair=JSON.parse(process.argv[3]);
for(const value of [build,edit,repair]) {
  if(value.package?.effectiveCompatibilityMode!=="enforced") throw new Error("DCP fixture did not enter enforced mode");
  if(value.gate!=="ready"||value.missingRequiredReads?.length!==0||value.materialization?.ready!==true||value.styleContract?.verified!==true) throw new Error("DCP required-read/materialization/style-contract gate is incomplete");
  for(const capability of ["computed-style","a11y","viewport"]) if(value.verification?.capabilities?.[capability]?.available!==true) throw new Error(`DCP verifier is unavailable: ${capability}`);
}
if(new Set([build.runId,edit.runId,repair.runId]).size!==3) throw new Error("DCP Build/Edit/Repair run IDs must be distinct");
if(new Set([build.package.contentHash,edit.package.contentHash,repair.package.contentHash]).size!==1) throw new Error("Edit/Repair did not inherit frozen DCP content hash");
if(new Set([build.materialization.hash,edit.materialization.hash,repair.materialization.hash]).size!==1) throw new Error("DCP materialization hash changed across Build/Edit/Repair");
for(const path of ["inputs/design-profile-usage.md","inputs/component-recipes.json","state/style-contract.json"]) if(!repair.readFiles?.includes(path)) throw new Error(`Repair did not read required DCP file: ${path}`);
' "${dcp_build_diagnostics}" "${dcp_edit_diagnostics}" "${dcp_repair_diagnostics}"
  fi
  # Never reuse a token issued before a model/build wait for artifact access.
  principal_token="$(issue_principal_token "${project_id}")"
  artifact_url="${base_url}/artifacts/${project_id}/current/"
  artifact_assertion_path="/"
  artifact_assertion_url="${artifact_url}"
  if [[ "${kind}" == "docs" && "${active_provider_mode}" == "deepseek" ]]; then
    # The initialized Fumadocs source renders authored content at /docs/;
    # the root page is only its stable navigation shell.
    artifact_assertion_path="/docs/"
    artifact_assertion_url="${artifact_url}docs/"
  fi
  artifact_assertions="$(node -e 'process.stdout.write(JSON.stringify({url:process.argv[1],expectedText:process.argv[2],headers:{authorization:`Bearer ${process.argv[3]}`}}))' \
    "${artifact_assertion_url}" "${repair_expected_text}" "${principal_token}" \
    | node services/runtime/scripts/assert-artifact-render.mjs)"
  artifact_assertions="$(node -e '
const evidence=JSON.parse(process.argv[1]);
evidence.route=process.argv[2];
process.stdout.write(JSON.stringify(evidence));
' "${artifact_assertions}" "${artifact_assertion_path}")"
  project_evidence="$(curl --fail --silent \
    -H 'x-anydesign-internal: true' \
    -H "x-runtime-admin-token: ${admin_token}" \
    "${base_url}/internal/projects/${project_id}/release-evidence")"
  node -e '
const evidence=JSON.parse(process.argv[1]);
if(evidence.terminalToolFailureCount!==0) throw new Error(`terminal tool failures: ${evidence.terminalToolFailureCount}`);
' "${project_evidence}"
  dependency_evidence="$(capture_dependency_workspace_evidence "${project_evidence}")"
  project_evidence="$(node -e '
const evidence=JSON.parse(process.argv[1]);
evidence.dependencyEvidence=JSON.parse(process.argv[2]);
process.stdout.write(JSON.stringify(evidence));
' "${project_evidence}" "${dependency_evidence}")"
  if [[ "${project_id}" == *"dcp-enforced"* ]]; then
    project_evidence="$(node -e '
const evidence=JSON.parse(process.argv[1]);
const [buildRunId,editRunId,reviewRunId,repairRunId,findingId,candidateVersionId]=process.argv.slice(2,8);
const reviewRepair=evidence.reviewRepair;
if(!reviewRepair||reviewRepair.editRunId!==editRunId||reviewRepair.reviewRunId!==reviewRunId||reviewRepair.repairRunId!==repairRunId) throw new Error("release evidence Review/Repair lineage mismatch");
if(reviewRepair.findings?.length!==1) throw new Error("release evidence must contain exactly one repaired finding");
const finding=reviewRepair.findings[0];
if(finding.findingId!==findingId||finding.versionId!==candidateVersionId||finding.severity!=="blocking"||finding.repairable!==true||finding.status!=="fixed") throw new Error("release evidence repaired finding is incomplete");
evidence.designContextEnforced={
  lifecycle:{buildRunId,editRunId,reviewRunId,repairRunId,findingId,candidateVersionId,findingStatus:"fixed"},
  build:JSON.parse(process.argv[8]),edit:JSON.parse(process.argv[9]),repair:JSON.parse(process.argv[10])
};
if(process.argv[11]==="true") evidence.designContextEnforced.reviewProvenance={
  source:"fixture-seeded",
  mutationProvider:"deepseek",
  reviewProvider:"deterministic-tool-sequence",
  repairProvider:"deepseek"
};
process.stdout.write(JSON.stringify(evidence));
' "${project_evidence}" "${build_run}" "${edit_run}" "${review_run}" "${repair_run}" "${finding_id}" "${edit_version_id}" \
      "${dcp_build_diagnostics}" "${dcp_edit_diagnostics}" "${dcp_repair_diagnostics}" "${seeded_review_provider}")"
  fi
  principal_token="$(issue_principal_token "${project_id}")"
  wrong_project_token="$(issue_principal_token "wrong-${project_id}")"
  preview_url="${base_url}/previews/$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).previewLeaseId)' "${project_evidence}")/"
  preview_prefix="/projects/${project_id}/previews/$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).previewLeaseId)' "${project_evidence}")"
  anonymous_status="$(curl --silent --output /dev/null --write-out '%{http_code}' "${preview_url}")"
  cross_project_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
    -H "authorization: Bearer ${wrong_project_token}" \
    -H "x-anydesign-preview-prefix: ${preview_prefix}" "${preview_url}")"
  owner_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
    -H "authorization: Bearer ${principal_token}" \
    -H "x-anydesign-preview-prefix: ${preview_prefix}" "${preview_url}")"
  if [[ "${anonymous_status}" != "401" || "${cross_project_status}" != "403" || "${owner_status}" != "200" ]]; then
    printf 'Public principal gate failed: anonymous=%s crossProject=%s owner=%s\n' \
      "${anonymous_status}" "${cross_project_status}" "${owner_status}" >&2
    exit 5
  fi
  reserve_real_provider_run cancel_probe "${project_id}"
  cancel_probe_payload="$(curl --fail --silent \
    -H 'content-type: application/json' \
    -H "authorization: Bearer ${principal_token}" \
    -d "$(node -e '
process.stdout.write(JSON.stringify({
  projectId:process.argv[1],phase:"edit",agentProfile:"edit",
  inputContext:{baseVersionId:process.argv[2],sandboxBindingId:process.argv[3]}
}));
' "${project_id}" "$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).versionAfterCas)' "${project_evidence}")" "${binding_id}")" \
    "${base_url}/runs")"
  cancel_probe_run="$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).runId)' "${cancel_probe_payload}")"
  cancel_probe_response="$(curl --fail --silent -X POST \
    -H "authorization: Bearer ${principal_token}" \
    "${base_url}/runs/${cancel_probe_run}/cancel")"
  node -e '
const response=JSON.parse(process.argv[1]);
if(response.runId!==process.argv[2]||response.status!=="cancelled")process.exit(2);
' "${cancel_probe_response}" "${cancel_probe_run}"
  principal_token="$(issue_principal_token "${project_id}")"
  cancelled_preview_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
    -H "authorization: Bearer ${principal_token}" \
    -H "x-anydesign-preview-prefix: ${preview_prefix}" "${preview_url}")"
  [[ "${cancelled_preview_status}" == "404" ]] || {
    printf 'cancelled run left Preview lease accessible: %s\n' "${cancelled_preview_status}" >&2
    exit 5
  }
  project_evidence="$(node -e '
const evidence=JSON.parse(process.argv[1]);
evidence.cancelCleanup={runId:process.argv[2],runStatus:"cancelled",previewHttpStatusAfterCancel:Number(process.argv[3]),passed:true};
process.stdout.write(JSON.stringify(evidence));
' "${project_evidence}" "${cancel_probe_run}" "${cancelled_preview_status}")"
  release_data="$(curl --fail --silent -X POST \
    -H 'x-anydesign-internal: true' \
    -H "x-runtime-admin-token: ${admin_token}" \
    "${base_url}/internal/projects/${project_id}/release-sandbox")"
  # Artifact routes remain principal-protected after Sandbox release. Refresh the
  # deliberately short JWT and prove project-scoped availability, not anonymous
  # access (which must continue to return 401).
  principal_token="$(issue_principal_token "${project_id}")"
  artifact_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
    -H "authorization: Bearer ${principal_token}" "${artifact_url}")"
  node -e '
const evidence=JSON.parse(process.argv[1]);
const released=JSON.parse(process.argv[2]);
evidence.kind=process.argv[3];
evidence.artifactUrl=new URL(evidence.artifactUrl,process.argv[4]).toString();
evidence.sandboxReleasedAt=released.releasedAt;
evidence.artifactHttpStatusAfterRelease=Number(process.argv[5]);
evidence.artifactAccessAfterRelease={authentication:"project-principal",projectId:evidence.projectId,httpStatus:Number(process.argv[5]),authenticated:true};
evidence.artifactAssertions=JSON.parse(process.argv[6]);
process.stdout.write(JSON.stringify(evidence));
' "${project_evidence}" "${release_data}" "${kind}" "${base_url}" "${artifact_status}" "${artifact_assertions}" \
    >"${output_file}"
}

write_real_project_evidence() {
  local kind="$1"
  local project_json="$2"
  local evidence_mode="fixture" provider_model="deterministic-tool-sequence"
  local credential_present="false"
  if [[ "${active_provider_mode}" == "deepseek" ]]; then
    evidence_mode="real"
    provider_model="${DEEPSEEK_E2E_MODEL:-deepseek-v4-pro}"
    credential_present="true"
  fi
  node -e '
const fs=require("fs");
fs.writeFileSync(process.argv[1],JSON.stringify({
  schemaVersion:"real-provider-project-evidence@1",
  recordedAt:new Date().toISOString(),
  provider:{mode:process.argv[3],model:process.argv[4],credentialPresent:process.argv[5]==="true"},
  project:JSON.parse(process.argv[2]),
},null,2)+"\n");
' "${evidence_dir}/real-provider-${kind}.json" "${project_json}" \
    "${evidence_mode}" "${provider_model}" "${credential_present}"
}

website=""
docs=""
provider_dcp=""
if [[ "${project_filter}" != "all" ]]; then
  if [[ "${project_filter}" == "website" ]]; then
    website_file="${gate_work_dir}/website.json"
    run_fixture "rc-website-${gate_id}" website 'RC Website Edited' "${website_file}" false \
      'RC Website Edited' "${website_workspace_namespace}"
    website="$(cat "${website_file}")"
    write_real_project_evidence website "${website}"
  else
    docs_file="${gate_work_dir}/docs.json"
    run_fixture "rc-docs-${gate_id}" docs 'RC Docs Edited' "${docs_file}" false \
      'RC Docs Edited' "${docs_workspace_namespace}"
    docs="$(cat "${docs_file}")"
    write_real_project_evidence docs "${docs}"
  fi
  printf 'Partial real-provider audit passed for project filter %s\n' "${project_filter}"
  exit 0
fi

fixture_website_file="${gate_work_dir}/fixture-website.json"
fixture_docs_file="${gate_work_dir}/fixture-docs.json"
fixture_started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
run_fixture "rc-website-${gate_id}-fixture" website 'RC Website Edited' \
  "${fixture_website_file}" false 'RC Website Edited' "${website_workspace_namespace}" &
fixture_website_pid=$!
run_fixture "rc-docs-${gate_id}-fixture" docs 'RC Docs Edited' \
  "${fixture_docs_file}" false 'RC Docs Edited' "${docs_workspace_namespace}" &
fixture_docs_pid=$!
wait "${fixture_website_pid}"
wait "${fixture_docs_pid}"
fixture_finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
fixture_website="$(cat "${fixture_website_file}")"
fixture_docs="$(cat "${fixture_docs_file}")"
fixture_evidence="${evidence_dir}/deployed-fixture.json"
node -e '
const fs=require("fs");
const projects=process.argv.slice(2,4).map(JSON.parse);
if(projects[0].podUid===projects[1].podUid||projects[0].artifactManifestHash===projects[1].artifactManifestHash)process.exit(2);
fs.writeFileSync(process.argv[1],JSON.stringify({
  schemaVersion:"deployed-fixture-evidence@1",provider:{mode:"fixture",model:"deterministic-tool-sequence"},
  execution:{mode:"concurrent",startedAt:process.argv[4],finishedAt:process.argv[5],passed:true},projects
},null,2)+"\n");
' "${fixture_evidence}" "${fixture_website}" "${fixture_docs}" "${fixture_started_at}" "${fixture_finished_at}"

recovery_probe_file="${gate_work_dir}/recovery-probe.json"
run_fixture "rc-website-${gate_id}-recovery" website 'RC Website Edited' \
  "${recovery_probe_file}" true 'RC Website Edited' "${website_workspace_namespace}"

if [[ "${concurrent_workspace_gate}" == "1" ]]; then
  concurrent_workspace_evidence="${evidence_dir}/workspace-concurrency-recovery.json"
  node - "${concurrent_workspace_evidence}" "${fixture_evidence}" \
    "${active_recovery_evidence}" "${recovery_probe_file}" <<'NODE'
const fs=require("fs");
const [output,fixturePath,recoveryPath,recoveryProjectPath]=process.argv.slice(2);
const fixture=JSON.parse(fs.readFileSync(fixturePath,"utf8"));
const recovery=JSON.parse(fs.readFileSync(recoveryPath,"utf8"));
const recoveryProject=JSON.parse(fs.readFileSync(recoveryProjectPath,"utf8"));
const namespaces=fixture.projects.map(project=>project.workspaceNamespace);
if(fixture.execution?.mode!=="concurrent"||fixture.execution?.passed!==true)throw new Error("concurrent fixture evidence failed");
if(new Set(namespaces).size!==2)throw new Error("concurrent fixtures did not use distinct Workspaces");
if(recovery.runtimeRestart?.result!=="pass"||recovery.portForwardKill?.result!=="pass")throw new Error("active Preview recovery failed");
fs.writeFileSync(output,`${JSON.stringify({
  schemaVersion:"workspace-concurrency-recovery-evidence@1",
  recordedAt:new Date().toISOString(),
  execution:fixture.execution,
  projects:fixture.projects,
  activePreviewRecovery:recovery,
  recoveryProject,
  passed:true,
},null,2)}\n`);
NODE
  printf 'Workspace concurrent Build and active Preview recovery gate passed: %s\n' \
    "${concurrent_workspace_evidence}"
  exit 0
fi

dcp_project_id="rc-website-dcp-enforced-${gate_id}"
dcp_fixture_policy="$(configure_enforced_dcp_fixture "${dcp_project_id}")"
dcp_fixture_file="${gate_work_dir}/enforced-dcp-website.json"
run_fixture "${dcp_project_id}" website 'RC Enforced DCP Website Repaired' \
  "${dcp_fixture_file}" false
dcp_fixture="$(node -e '
const fixture=JSON.parse(process.argv[1]);
const policy=JSON.parse(process.argv[2]);
if(!fixture.designContextEnforced) throw new Error("missing enforced DCP fixture evidence");
fixture.designContextEnforced.policy=policy;
process.stdout.write(JSON.stringify(fixture));
' "$(cat "${dcp_fixture_file}")" "${dcp_fixture_policy}")"

if [[ "${provider_mode}" == "deepseek" ]]; then
  configure_deepseek_provider
  active_provider_mode="deepseek"
  "${KUBECTL}" rollout restart deployment/anydesign-runtime -n anydesign-runtime >/dev/null
  "${KUBECTL}" rollout status deployment/anydesign-runtime -n anydesign-runtime --timeout=300s >/dev/null
  stop_runtime_port_forward
  start_runtime_port_forward
  runtime_pod="$(${KUBECTL} get pods -n anydesign-runtime -l app=anydesign-runtime -o json \
    | node -e 'const fs=require("fs");const p=JSON.parse(fs.readFileSync(0,"utf8")).items.find(p=>!p.metadata.deletionTimestamp&&p.status.conditions?.some(c=>c.type==="Ready"&&c.status==="True"));if(!p)process.exit(2);process.stdout.write(p.metadata.name)')"
  provider_dcp_project_id="rc-website-dcp-enforced-dcp-provider-${gate_id}"
  provider_dcp_policy="$(configure_enforced_dcp_fixture "${provider_dcp_project_id}")"
  provider_dcp_file="${gate_work_dir}/provider-dcp-website.json"
  run_fixture "${provider_dcp_project_id}" website \
    'RC Enforced DCP Provider Website Edited' "${provider_dcp_file}" false \
    'RC Enforced DCP Provider Website Repaired'
  provider_dcp="$(node -e '
const project=JSON.parse(process.argv[1]);
const policy=JSON.parse(process.argv[2]);
if(!project.designContextEnforced) throw new Error("missing real-provider enforced DCP evidence");
project.designContextEnforced.policy=policy;
process.stdout.write(JSON.stringify(project));
' "$(cat "${provider_dcp_file}")" "${provider_dcp_policy}")"
  write_real_project_evidence dcp-website "${provider_dcp}"
  website_file="${gate_work_dir}/website.json"
  docs_file="${gate_work_dir}/docs.json"
  run_fixture "rc-website-${gate_id}-real" website 'RC Website Edited' "${website_file}" false
  website="$(cat "${website_file}")"
  write_real_project_evidence website "${website}"
  run_fixture "rc-docs-${gate_id}-real" docs 'RC Docs Edited' "${docs_file}" false
  docs="$(cat "${docs_file}")"
  write_real_project_evidence docs "${docs}"
else
  website_file="${fixture_website_file}"
  docs_file="${fixture_docs_file}"
  website="${fixture_website}"
  docs="${fixture_docs}"
fi

if [[ "${provider_mode}" == "deepseek" ]]; then
  provider_model="${DEEPSEEK_E2E_MODEL:-deepseek-v4-pro}"
fi
if [[ -z "${website}" || -z "${docs}" ]]; then
  printf 'Website and Docs project evidence is required\n' >&2
  exit 6
fi

real_provider_budget_evidence=""
if [[ "${provider_mode}" == "deepseek" ]]; then
  real_provider_budget_evidence="${evidence_dir}/real-provider-token-budget.json"
  node - "${real_provider_budget_evidence}" "${real_provider_total_token_ceiling}" \
    "${real_provider_per_run_safety_ceiling}" "${real_provider_reserved_runs}" <<'NODE'
const fs = require("node:fs");
const [output, total, perRun, runs] = process.argv.slice(2);
const evidence = {
  schemaVersion: "runtime-rc-real-provider-token-budget@1",
  recordedAt: new Date().toISOString(),
  totalTokenCeiling: Number(total),
  perRunSafetyCeiling: Number(perRun),
  reservedRunCount: Number(runs),
  reservedTokens: Number(perRun) * Number(runs),
};
evidence.withinCeiling = evidence.reservedTokens <= evidence.totalTokenCeiling;
if (!evidence.withinCeiling) throw new Error("real Provider reservations exceed the total ceiling");
fs.writeFileSync(output, `${JSON.stringify(evidence, null, 2)}\n`);
NODE
fi

evidence="${evidence_dir}/runtime-rc-${image_tag}.json"
provider_evidence_mode="fixture"
provider_model="deterministic-tool-sequence"
provider_credential_present=false
if [[ "${provider_mode}" == "deepseek" ]]; then
  provider_model="${DEEPSEEK_E2E_MODEL:-deepseek-v4-pro}"
  provider_credential_present=true
  provider_evidence_mode="real"
fi
node -e '
const fs=require("fs");
const payload={
  recordedAt:new Date().toISOString(),
  repository:{commit:process.argv[8],dirty:process.argv[9]==="true",lockHash:process.argv[10]},
  cluster:JSON.parse(process.argv[11]),
  images:{runtime:{ref:process.argv[4],configDigest:process.argv[5],manifestDigest:process.argv[22],reportedCommit:JSON.parse(process.argv[2]).repositoryCommit}},
  transport:JSON.parse(fs.readFileSync(process.argv[21],"utf8")).transport,
  auth:{principalMode:process.argv[14],projectOwnershipVerified:process.argv[15]==="true",channelJwtVerified:true},
  provider:{
    mode:process.argv[16],
    model:process.argv[17],
    credentialPresent:process.argv[19]==="true",
    tokenBudget:process.argv[25]?JSON.parse(fs.readFileSync(process.argv[25],"utf8")):null,
  },
  runtimeVersion:JSON.parse(process.argv[2]),
  runtimePod:process.argv[3],
  runtimeImage:process.argv[4],
  runtimeImageId:process.argv[5],
  runtimeManifestDigest:process.argv[22],
  fixture:JSON.parse(fs.readFileSync(process.argv[20],"utf8")),
  projects:[JSON.parse(process.argv[6]),JSON.parse(process.argv[7])],
  enforcedDcpFixture:JSON.parse(process.argv[23]),
  providerDcpProject:process.argv[24]?JSON.parse(process.argv[24]):null,
};
fs.writeFileSync(process.argv[1],JSON.stringify(payload,null,2)+"\n");
' "${evidence}" "${version_json}" "${runtime_pod}" "${runtime_image}" "${pod_image_id}" "${website}" "${docs}" \
  "${git_sha}" "${dirty_flag}" "${lock_hash}" \
  "$(node -e '
const {execFileSync}=require("child_process");
const cluster=JSON.parse(execFileSync(process.argv[1],["get","node","-o","json"],{encoding:"utf8"}));
const node=cluster.items[0];
process.stdout.write(JSON.stringify({name:process.argv[2],kubeContext:process.argv[3],createdAt:node.metadata.creationTimestamp,nodeUid:node.metadata.uid}));
' "${KUBECTL}" "${cluster_name}" "${context}")" \
  "$(printf '%s' 'spiffe://anydesign.local/ns/anydesign-runtime/sa/anydesign-runtime' | shasum -a 256 | awk '{print $1}')" \
  "$(printf '%s' "spiffe://anydesign.local/ns/${workspace_namespace}/sa/anydesign-sandbox" | shasum -a 256 | awk '{print $1}')" \
  "required" "true" "${provider_evidence_mode}" "${provider_model}" \
  "" "${provider_credential_present}" "${fixture_evidence}" \
  "${channel_evidence}" "${runtime_manifest_digest}" "${dcp_fixture}" "${provider_dcp}" \
  "${real_provider_budget_evidence}"
printf 'Runtime RC gate passed: %s\n' "${evidence}"

npm_evidence="${evidence_dir}/npm-proxy.json"
recovery_evidence="${evidence_dir}/recovery.json"
ANYDESIGN_E2E_CLUSTER="${cluster_name}" \
  NPM_PROXY_EVIDENCE_PATH="${npm_evidence}" \
  NPM_PROXY_PROJECT_EVIDENCE_FILE="${evidence}" \
  bash infra/agent-sandbox/run-npm-proxy-gate.sh
ANYDESIGN_E2E_CLUSTER="${cluster_name}" \
  ANYDESIGN_E2E_NAMESPACE="${workspace_namespace}" \
  RECOVERY_EVIDENCE_PATH="${recovery_evidence}" \
  ACTIVE_RECOVERY_EVIDENCE_PATH="${active_recovery_evidence}" \
  RECOVERY_RUNTIME_EVIDENCE_FILE="${evidence}" \
  RECOVERY_BASELINE_FILE="${recovery_baseline_file}" \
  bash infra/agent-sandbox/run-runtime-recovery-gate.sh

release_evidence="${evidence_dir}/release-evidence.json"
aggregate_args=(
  --runtime "${evidence}"
  --preflight "${preflight_evidence}"
  --channel "${channel_evidence}"
  --npm "${npm_evidence}"
  --recovery "${recovery_evidence}"
  --lock infra/agent-sandbox/images.lock.json
  --out "${release_evidence}"
  --mode "${rc_mode}"
)
if [[ -n "${RUNTIME_RC_PROVIDER_EVIDENCE:-}" ]]; then
  aggregate_args+=(--provider "${RUNTIME_RC_PROVIDER_EVIDENCE}")
fi
node services/runtime/scripts/aggregate-release-evidence.mjs "${aggregate_args[@]}"
if [[ "${rc_mode}" == "release" ]]; then
  node services/runtime/scripts/validate-release-evidence.mjs "${release_evidence}"
  release_evidence_absolute="$(cd "$(dirname "${release_evidence}")" && pwd)/$(basename "${release_evidence}")"
  printf 'RC_RELEASE_ELIGIBLE=true\n'
  printf 'RC_EVIDENCE_SUMMARY=%s\n' "${release_evidence_absolute}"
  printf 'RC_COMMIT=%s\n' "${git_full_sha}"
  printf 'RC_RUNTIME_IMAGE=%s@%s\n' "${runtime_image}" "${runtime_manifest_digest}"
else
  printf 'RC audit complete; strict release validation intentionally not asserted: %s\n' \
    "${release_evidence}"
fi
