#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
K3D="${K3D:-k3d}"
OPENSSL="${OPENSSL:-openssl}"
cd "${ROOT_DIR}"

# macOS ships LibreSSL as /usr/bin/openssl, which cannot generate the Ed25519
# PKCS#8 key used by the workspace-channel signer. Prefer a user-selected
# binary, then a Homebrew OpenSSL 3 installation, and fail before mutating the
# cluster when no compatible binary is available.
if ! "${OPENSSL}" genpkey -algorithm ED25519 >/dev/null 2>&1; then
  for candidate in \
    /opt/homebrew/opt/openssl@3/bin/openssl \
    /usr/local/opt/openssl@3/bin/openssl; do
    if [[ -x "${candidate}" ]] \
      && "${candidate}" genpkey -algorithm ED25519 >/dev/null 2>&1; then
      OPENSSL="${candidate}"
      break
    fi
  done
fi
if ! "${OPENSSL}" genpkey -algorithm ED25519 >/dev/null 2>&1; then
  printf 'OpenSSL 3 with Ed25519 support is required; set OPENSSL to a compatible binary\n' >&2
  exit 2
fi

cluster_name="${ANYDESIGN_E2E_CLUSTER:-zerondesign-e2e}"
sandbox_namespace="${ANYDESIGN_E2E_NAMESPACE:-ws-runtime-rc}"
if [[ ! "${sandbox_namespace}" =~ ^ws-[a-z0-9]([a-z0-9-]*[a-z0-9])?$ ]] \
  || (( ${#sandbox_namespace} > 63 )); then
  printf 'ANYDESIGN_E2E_NAMESPACE must be a ws-* Kubernetes Namespace\n' >&2
  exit 2
fi
lock_file="infra/agent-sandbox/images.lock.json"
locked_image() {
  node -e '
const fs=require("fs");
const lock=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));
const image=lock.images?.[process.argv[2]];
if(!image?.ref||!/^sha256:[a-f0-9]{64}$/.test(image.digest||""))process.exit(2);
process.stdout.write(`${image.ref}@${image.digest}`);
' "${lock_file}" "$1"
}
k3s_image="${K3S_IMAGE:-$(locked_image k3s)}"
sandbox_base_image="${SANDBOX_BASE_IMAGE:-$(locked_image sandboxNode)}"
controller_image="$(locked_image agentSandboxController)"
npm_proxy_image="$(locked_image npmProxy)"
local_path_helper_image="$(locked_image localPathHelper)"
if ! "${K3D}" cluster list --no-headers 2>/dev/null | awk '{print $1}' | grep -Fxq "${cluster_name}"; then
  "${K3D}" cluster create "${cluster_name}" \
    --image "${k3s_image}" \
    --servers 1 \
    --agents 0 \
    --no-lb \
    --registry-create 'k3d-greenfield-registry.localhost:0.0.0.0:5003' \
    --k3s-arg '--cluster-init@server:0' \
    --k3s-arg '--disable=traefik@server:*' \
    --k3s-arg '--disable=metrics-server@server:*' \
    --wait
fi
"${KUBECTL}" config use-context "k3d-${cluster_name}" >/dev/null
context="$(${KUBECTL} config current-context 2>/dev/null || true)"
if [[ "${context}" != "k3d-${cluster_name}" ]]; then
  printf 'failed to select dedicated k3d context k3d-%s; got %s\n' \
    "${cluster_name}" "${context:-<none>}" >&2
  exit 2
fi
local_path_found=false
for _ in $(seq 1 120); do
  if "${KUBECTL}" get deployment/local-path-provisioner -n kube-system >/dev/null 2>&1; then
    local_path_found=true
    break
  fi
  sleep 1
done
if [[ "${local_path_found}" != "true" ]]; then
  printf 'local-path-provisioner deployment did not appear\n' >&2
  exit 3
fi
"${KUBECTL}" rollout status deployment/local-path-provisioner \
  -n kube-system \
  --timeout=300s >/dev/null
assert_deployment_digest() {
  local namespace="$1"
  local deployment="$2"
  local selector="$3"
  local expected_image="$4"
  local expected_digest spec_image pod_image_id
  expected_digest="${expected_image##*@}"
  spec_image="$("${KUBECTL}" get deployment "${deployment}" -n "${namespace}" \
    -o 'jsonpath={.spec.template.spec.containers[0].image}')"
  pod_image_id="$("${KUBECTL}" get pods -n "${namespace}" -l "${selector}" \
    -o 'jsonpath={.items[0].status.containerStatuses[0].imageID}')"
  if [[ "${spec_image}" != *"@${expected_digest}" && "${pod_image_id}" != *"@${expected_digest}" ]]; then
    printf 'deployment image digest mismatch: namespace=%s deployment=%s expected=%s spec=%s imageID=%s\n' \
      "${namespace}" "${deployment}" "${expected_digest}" "${spec_image}" "${pod_image_id}" >&2
    exit 3
  fi
  if [[ "${pod_image_id}" != *"@${expected_digest}" ]]; then
    printf 'deployment runtime imageID mismatch: namespace=%s deployment=%s expected=%s imageID=%s\n' \
      "${namespace}" "${deployment}" "${expected_digest}" "${pod_image_id}" >&2
    exit 3
  fi
}
assert_deployment_digest \
  kube-system \
  local-path-provisioner \
  app=local-path-provisioner \
  "$(locked_image localPathProvisioner)"

# K3s ships local-path-provisioner with a mutable BusyBox helper tag. PVC
# creation is part of the Sandbox supply chain, so pin the helper before any
# SandboxTemplate can request a workspace volume.
"${KUBECTL}" get configmap local-path-config -n kube-system -o json \
  | node -e '
let input="";process.stdin.on("data",chunk=>input+=chunk).on("end",()=>{
  const cm=JSON.parse(input);
  const helper=cm.data?.["helperPod.yaml"];
  if(typeof helper!=="string"||!/^(\s*)image:\s*.+$/m.test(helper))process.exit(2);
  cm.data["helperPod.yaml"]=helper.replace(
    /^(\s*)image:\s*.+$/m,
    (_,indent)=>`${indent}image: "${process.argv[1]}"`,
  );
  delete cm.metadata.managedFields;
  delete cm.metadata.resourceVersion;
  delete cm.metadata.uid;
  delete cm.metadata.creationTimestamp;
  process.stdout.write(JSON.stringify(cm));
});
' "${local_path_helper_image}" \
  | "${KUBECTL}" apply -f - >/dev/null
actual_local_path_helper="$(${KUBECTL} get configmap local-path-config -n kube-system \
  -o 'jsonpath={.data.helperPod\.yaml}' | awk '/^[[:space:]]*image:/ {gsub(/[" ]/, "", $2); print $2; exit}')"
[[ "${actual_local_path_helper}" == "${local_path_helper_image}" ]] || {
  printf 'local-path helper image pin failed: expected=%s actual=%s\n' \
    "${local_path_helper_image}" "${actual_local_path_helper:-<missing>}" >&2
  exit 3
}
"${KUBECTL}" rollout restart deployment/local-path-provisioner -n kube-system >/dev/null
"${KUBECTL}" rollout status deployment/local-path-provisioner \
  -n kube-system --timeout=300s >/dev/null
git_sha="$(git rev-parse --short=12 HEAD)"
dirty="$(git status --porcelain | wc -l | tr -d ' ')"
image_tag="${git_sha}"
if [[ "${dirty}" != "0" ]]; then
  worktree_fingerprint="$({
    git diff --binary
    git ls-files --others --exclude-standard \
      | sort \
      | while IFS= read -r file; do shasum -a 256 "${file}"; done
  } | shasum -a 256 | awk '{print substr($1, 1, 12)}')"
  image_tag="${git_sha}-dirty-${worktree_fingerprint}"
fi
sandbox_image="anydesign/agent-sandbox:${image_tag}"
# Runtime's built-in sandbox execution profile deliberately pins this public
# reference.  The isolated gate imports the locally built bytes under the same
# reference so readiness verifies the production identity instead of accepting
# a test-only tag.
runtime_profile_sandbox_image="ghcr.io/carlosfengv/zerondesign/agent-sandbox:0.1.0"

docker build \
  -f infra/agent-sandbox/base/Dockerfile \
  --provenance=false \
  --build-arg "SANDBOX_BASE_IMAGE=${sandbox_base_image}" \
  -t "${sandbox_image}" \
  infra/agent-sandbox
docker tag "${sandbox_image}" "${runtime_profile_sandbox_image}"
# Docker's containerd image store exposes the manifest digest as `.Id`, while
# Kubernetes reports the image config digest in Pod `imageID`. Keep the two
# identities separate: the manifest digest determines whether k3d needs an
# import, and the config digest proves the running container uses those bytes.
read -r expected_manifest_digest expected_image_id < <(
  docker image inspect "${sandbox_image}" | node -e '
let input="";process.stdin.on("data",chunk=>input+=chunk).on("end",()=>{
  const [image]=JSON.parse(input);
  const descriptor=image?.Descriptor ?? {};
  const repoDigest=Array.isArray(image?.RepoDigests)
    ? image.RepoDigests.find(value=>typeof value==="string"&&value.includes("@sha256:"))
    : undefined;
  const manifestDigest=descriptor.digest ?? repoDigest?.split("@").pop() ?? image?.Id;
  const configDigest=descriptor.annotations?.["config.digest"] ?? image?.Id;
  process.stdout.write(`${manifestDigest ?? ""} ${configDigest ?? ""}\n`);
});
'
)
if [[ ! "${expected_manifest_digest}" =~ ^sha256:[a-f0-9]{64}$ ]] \
  || [[ ! "${expected_image_id}" =~ ^sha256:[a-f0-9]{64}$ ]]; then
  printf 'sandbox image digests are invalid: manifest=%s config=%s\n' \
    "${expected_manifest_digest:-<missing>}" "${expected_image_id:-<missing>}" >&2
  exit 3
fi
cluster_server="k3d-${cluster_name}-server-0"
cluster_sandbox_digest="$(
  docker exec "${cluster_server}" ctr -n k8s.io images ls 2>/dev/null \
    | awk -v ref="${runtime_profile_sandbox_image}" '$1 == ref { print $3; exit }'
)"
if [[ "${cluster_sandbox_digest}" == "${expected_manifest_digest}" ]]; then
  printf 'Sandbox image already imported with expected digest: %s\n' \
    "${expected_manifest_digest}"
else
  "${K3D}" image import --cluster "${cluster_name}" \
    "${runtime_profile_sandbox_image}"
fi

required_crds=(
  "sandboxes.agents.x-k8s.io"
  "sandboxclaims.extensions.agents.x-k8s.io"
  "sandboxtemplates.extensions.agents.x-k8s.io"
  "sandboxwarmpools.extensions.agents.x-k8s.io"
)

missing_crds=()
for crd in "${required_crds[@]}"; do
  if ! "${KUBECTL}" get crd "${crd}" >/dev/null 2>&1; then
    missing_crds+=("${crd}")
  fi
done

if (( ${#missing_crds[@]} > 0 )); then
  printf 'Installing the pinned agent-sandbox controller; missing CRDs:\n'
  printf '  - %s\n' "${missing_crds[@]}"
  AGENT_SANDBOX_CONTROLLER_IMAGE="${controller_image}" \
    bash infra/agent-sandbox/install-controller.sh
fi
assert_deployment_digest \
  agent-sandbox-system \
  agent-sandbox-controller \
  app=agent-sandbox-controller \
  "${controller_image}"

sed "s/anydesign-sandboxes/${sandbox_namespace}/g" \
  infra/agent-sandbox/rbac/runtime-service-account.yaml \
  | "${KUBECTL}" apply -f -

key_dir="$(mktemp -d)"
warm_pool_scaled_for_gate=false
next_app_pool_replicas=0
docs_pool_replicas=0
cleanup_key_dir() {
  if [[ "${warm_pool_scaled_for_gate}" == "true" ]]; then
    "${KUBECTL}" patch sandboxwarmpool anydesign-next-app-pool \
      -n "${sandbox_namespace}" --type=merge \
      -p "{\"spec\":{\"replicas\":${next_app_pool_replicas}}}" >/dev/null 2>&1 || true
    "${KUBECTL}" patch sandboxwarmpool anydesign-fumadocs-docs-pool \
      -n "${sandbox_namespace}" --type=merge \
      -p "{\"spec\":{\"replicas\":${docs_pool_replicas}}}" >/dev/null 2>&1 || true
  fi
  "${KUBECTL}" get sandboxclaim -n "${sandbox_namespace}" -o name 2>/dev/null \
    | rg '^sandboxclaim[^/]*/project-(website-k3d|docs-k3d)-' \
    | xargs -r "${KUBECTL}" delete -n "${sandbox_namespace}" --ignore-not-found=true >/dev/null 2>&1 || true
  rm -rf "${key_dir}"
}
trap cleanup_key_dir EXIT

signer_secret="anydesign-workspace-channel-signer"
verifier_config_map="anydesign-workspace-channel-verifier"
private_key_file="${key_dir}/private.der"
public_key_file="${key_dir}/public.der"
previous_public_key_file="${key_dir}/previous-public.der"
channel_ca_key_file="${key_dir}/channel-ca.key"
channel_ca_file="${key_dir}/channel-ca.crt"
previous_channel_ca_key_file="${key_dir}/previous-channel-ca.key"
previous_channel_ca_file="${key_dir}/previous-channel-ca.crt"
channel_ca_bundle_file="${key_dir}/channel-ca-bundle.crt"
runtime_tls_key_file="${key_dir}/runtime-tls.key"
runtime_tls_csr_file="${key_dir}/runtime-tls.csr"
runtime_tls_cert_file="${key_dir}/runtime-tls.crt"
sandbox_tls_key_file="${key_dir}/sandbox-tls.key"
sandbox_tls_csr_file="${key_dir}/sandbox-tls.csr"
sandbox_tls_cert_file="${key_dir}/sandbox-tls.crt"

if "${KUBECTL}" get secret "${signer_secret}" -n anydesign-runtime >/dev/null 2>&1; then
  "${KUBECTL}" get secret "${signer_secret}" \
    -n anydesign-runtime \
    -o 'jsonpath={.data.private\.der}' \
    | "${OPENSSL}" base64 -d -A >"${private_key_file}"
else
  "${OPENSSL}" genpkey -algorithm ED25519 -outform DER -out "${private_key_file}"
  "${KUBECTL}" create secret generic "${signer_secret}" \
    -n anydesign-runtime \
    --from-file="private.der=${private_key_file}" \
    --dry-run=client \
    -o yaml \
    | "${KUBECTL}" apply -f -
fi

"${OPENSSL}" pkey \
  -inform DER \
  -in "${private_key_file}" \
  -pubout \
  -outform DER \
  -out "${public_key_file}"
if "${KUBECTL}" get configmap "${verifier_config_map}" -n "${sandbox_namespace}" >/dev/null 2>&1; then
  "${KUBECTL}" get configmap "${verifier_config_map}" \
    -n "${sandbox_namespace}" \
    -o 'jsonpath={.binaryData.current\.der}' \
    | "${OPENSSL}" base64 -d -A >"${previous_public_key_file}" || true
fi
if [[ ! -s "${previous_public_key_file}" ]]; then
  cp "${public_key_file}" "${previous_public_key_file}"
fi
"${KUBECTL}" create configmap "${verifier_config_map}" \
  -n "${sandbox_namespace}" \
  --from-file="current.der=${public_key_file}" \
  --from-file="previous.der=${previous_public_key_file}" \
  --dry-run=client \
  -o yaml \
  | "${KUBECTL}" apply -f -

"${OPENSSL}" req -x509 -newkey rsa:2048 -sha256 -nodes -days 2 \
  -subj "/CN=AnyDesign RC Workspace Channel CA" \
  -keyout "${channel_ca_key_file}" \
  -out "${channel_ca_file}" >/dev/null 2>&1
"${OPENSSL}" req -x509 -newkey rsa:2048 -sha256 -nodes -days 2 \
  -subj "/CN=AnyDesign RC Previous Workspace Channel CA" \
  -keyout "${previous_channel_ca_key_file}" \
  -out "${previous_channel_ca_file}" >/dev/null 2>&1
cat "${channel_ca_file}" "${previous_channel_ca_file}" >"${channel_ca_bundle_file}"
"${OPENSSL}" req -newkey rsa:2048 -nodes \
  -subj "/CN=anydesign-runtime" \
  -keyout "${runtime_tls_key_file}" \
  -out "${runtime_tls_csr_file}" >/dev/null 2>&1
cat >"${key_dir}/runtime-tls.ext" <<'EOF'
basicConstraints=CA:FALSE
keyUsage=digitalSignature,keyEncipherment
extendedKeyUsage=clientAuth
subjectAltName=URI:spiffe://anydesign.local/ns/anydesign-runtime/sa/anydesign-runtime
EOF
"${OPENSSL}" x509 -req -sha256 -days 2 \
  -in "${runtime_tls_csr_file}" \
  -CA "${channel_ca_file}" \
  -CAkey "${channel_ca_key_file}" \
  -CAcreateserial \
  -extfile "${key_dir}/runtime-tls.ext" \
  -out "${runtime_tls_cert_file}" >/dev/null 2>&1
"${OPENSSL}" req -newkey rsa:2048 -nodes \
  -subj "/CN=workspace-channel.${sandbox_namespace}.svc.cluster.local" \
  -keyout "${sandbox_tls_key_file}" \
  -out "${sandbox_tls_csr_file}" >/dev/null 2>&1
cat >"${key_dir}/sandbox-tls.ext" <<EOF
basicConstraints=CA:FALSE
keyUsage=digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=DNS:*.${sandbox_namespace}.svc.cluster.local,DNS:*.${sandbox_namespace}.svc,IP:127.0.0.1,URI:spiffe://anydesign.local/ns/${sandbox_namespace}/sa/anydesign-sandbox
EOF
"${OPENSSL}" x509 -req -sha256 -days 2 \
  -in "${sandbox_tls_csr_file}" \
  -CA "${channel_ca_file}" \
  -CAkey "${channel_ca_key_file}" \
  -CAcreateserial \
  -extfile "${key_dir}/sandbox-tls.ext" \
  -out "${sandbox_tls_cert_file}" >/dev/null 2>&1
"${KUBECTL}" create secret generic anydesign-runtime-channel-client \
  -n anydesign-runtime \
  --from-file="ca.crt=${channel_ca_bundle_file}" \
  --from-file="tls.crt=${runtime_tls_cert_file}" \
  --from-file="tls.key=${runtime_tls_key_file}" \
  --dry-run=client -o yaml | "${KUBECTL}" apply -f -
"${KUBECTL}" create secret generic anydesign-sandbox-channel-server \
  -n "${sandbox_namespace}" \
  --from-file="ca.crt=${channel_ca_bundle_file}" \
  --from-file="tls.crt=${sandbox_tls_cert_file}" \
  --from-file="tls.key=${sandbox_tls_key_file}" \
  --dry-run=client -o yaml | "${KUBECTL}" apply -f -

sed "s/anydesign-sandboxes/${sandbox_namespace}/g" \
  infra/agent-sandbox/network/default-deny.yaml \
  | "${KUBECTL}" apply -f -
"${KUBECTL}" apply -f infra/agent-sandbox/npm-proxy/config-map.yaml
"${KUBECTL}" apply -f infra/agent-sandbox/npm-proxy/deployment.yaml
"${KUBECTL}" apply -f infra/agent-sandbox/npm-proxy/service.yaml
sed \
  -e "s/anydesign-sandboxes/${sandbox_namespace}/g" \
  -e "s|image: ghcr.io/carlosfengv/zerondesign/agent-sandbox:0.1.0|image: ${runtime_profile_sandbox_image}|" \
  infra/agent-sandbox/fumadocs-docs/sandbox-template.yaml \
  | "${KUBECTL}" apply -f -
sed "s/anydesign-sandboxes/${sandbox_namespace}/g" \
  infra/agent-sandbox/fumadocs-docs/sandbox-warm-pool.yaml \
  | "${KUBECTL}" apply -f -
sed \
  -e "s/anydesign-sandboxes/${sandbox_namespace}/g" \
  -e "s|image: ghcr.io/carlosfengv/zerondesign/agent-sandbox:0.1.0|image: ${runtime_profile_sandbox_image}|" \
  infra/agent-sandbox/next-app/sandbox-template.yaml \
  | "${KUBECTL}" apply -f -
sed "s/anydesign-sandboxes/${sandbox_namespace}/g" \
  infra/agent-sandbox/next-app/sandbox-warm-pool.yaml \
  | "${KUBECTL}" apply -f -

# Production deliberately keeps both pools at zero resident replicas. The
# isolated image-parity gate needs one warm Pod from each template, so scale
# them only for the duration of this script and restore the manifest values on
# every exit path.
next_app_pool_replicas="$("${KUBECTL}" get sandboxwarmpool anydesign-next-app-pool \
  -n "${sandbox_namespace}" -o 'jsonpath={.spec.replicas}')"
docs_pool_replicas="$("${KUBECTL}" get sandboxwarmpool anydesign-fumadocs-docs-pool \
  -n "${sandbox_namespace}" -o 'jsonpath={.spec.replicas}')"
warm_pool_scaled_for_gate=true
"${KUBECTL}" patch sandboxwarmpool anydesign-next-app-pool \
  -n "${sandbox_namespace}" --type=merge -p '{"spec":{"replicas":1}}' >/dev/null
"${KUBECTL}" patch sandboxwarmpool anydesign-fumadocs-docs-pool \
  -n "${sandbox_namespace}" --type=merge -p '{"spec":{"replicas":1}}' >/dev/null

"${KUBECTL}" rollout status deployment/anydesign-npm-proxy \
  -n anydesign-runtime \
  --timeout=600s
assert_deployment_digest \
  anydesign-runtime \
  anydesign-npm-proxy \
  app.kubernetes.io/name=anydesign-npm-proxy \
  "${npm_proxy_image}"

if [[ "${ANYDESIGN_E2E_RESET_WARM_POOL:-1}" == "1" ]]; then
  "${KUBECTL}" delete sandboxes.agents.x-k8s.io \
    -n "${sandbox_namespace}" \
    -l agents.x-k8s.io/launch-type=warm \
    --ignore-not-found=true
fi

deadline=$((SECONDS + 180))
warm_pod=""
while true; do
  ready_replicas="$("${KUBECTL}" get sandboxwarmpool anydesign-next-app-pool \
    -n "${sandbox_namespace}" \
    -o 'jsonpath={.status.readyReplicas}' 2>/dev/null || true)"
  warm_selector="$("${KUBECTL}" get sandboxwarmpool anydesign-next-app-pool \
    -n "${sandbox_namespace}" \
    -o 'jsonpath={.status.selector}' 2>/dev/null || true)"
  if [[ "${ready_replicas:-0}" -ge 1 && -n "${warm_selector}" ]]; then
    while IFS= read -r pod; do
      [[ -n "${pod}" ]] || continue
      phase="$("${KUBECTL}" get pod "${pod}" -n "${sandbox_namespace}" \
        -o 'jsonpath={.status.phase}' 2>/dev/null || true)"
      ready="$("${KUBECTL}" get pod "${pod}" -n "${sandbox_namespace}" \
        -o 'jsonpath={.status.conditions[?(@.type=="Ready")].status}' 2>/dev/null || true)"
      image="$("${KUBECTL}" get pod "${pod}" -n "${sandbox_namespace}" \
        -o 'jsonpath={.spec.containers[0].image}' 2>/dev/null || true)"
      if [[ "${phase}" == "Running" && "${ready}" == "True" && "${image}" == "${runtime_profile_sandbox_image}" ]]; then
        warm_pod="${pod}"
        break
      fi
    done < <("${KUBECTL}" get pod -n "${sandbox_namespace}" \
      -l "${warm_selector}" \
      -o name 2>/dev/null | sed 's|^pod/||')
  fi
  if [[ -n "${warm_pod}" ]]; then
    break
  fi
  if (( SECONDS >= deadline )); then
    printf 'SandboxWarmPool anydesign-next-app-pool did not become ready; readyReplicas=%s\n' "${ready_replicas:-0}" >&2
    exit 3
  fi
  sleep 2
done

pod_image="$(${KUBECTL} get pod "${warm_pod}" -n "${sandbox_namespace}" \
  -o 'jsonpath={.spec.containers[0].image}')"
pod_image_id="$(${KUBECTL} get pod "${warm_pod}" -n "${sandbox_namespace}" \
  -o 'jsonpath={.status.containerStatuses[0].imageID}')"
if [[ "${pod_image}" != "${runtime_profile_sandbox_image}" || "${pod_image_id}" != "${expected_image_id}" ]]; then
  printf 'sandbox image parity failed: expected=%s expectedID=%s actual=%s imageID=%s\n' \
    "${sandbox_image}" "${expected_image_id}" "${pod_image}" "${pod_image_id}" >&2
  exit 4
fi
docs_deadline=$((SECONDS + 180))
docs_warm_pod=""
while [[ -z "${docs_warm_pod}" ]]; do
  docs_ready_replicas="$("${KUBECTL}" get sandboxwarmpool anydesign-fumadocs-docs-pool \
    -n "${sandbox_namespace}" -o 'jsonpath={.status.readyReplicas}' 2>/dev/null || true)"
  docs_warm_selector="$("${KUBECTL}" get sandboxwarmpool anydesign-fumadocs-docs-pool \
    -n "${sandbox_namespace}" -o 'jsonpath={.status.selector}' 2>/dev/null || true)"
  if [[ "${docs_ready_replicas:-0}" -ge 1 && -n "${docs_warm_selector}" ]]; then
    while IFS= read -r pod; do
      [[ -n "${pod}" ]] || continue
      phase="$("${KUBECTL}" get pod "${pod}" -n "${sandbox_namespace}" -o 'jsonpath={.status.phase}' 2>/dev/null || true)"
      ready="$("${KUBECTL}" get pod "${pod}" -n "${sandbox_namespace}" -o 'jsonpath={.status.conditions[?(@.type=="Ready")].status}' 2>/dev/null || true)"
      image="$("${KUBECTL}" get pod "${pod}" -n "${sandbox_namespace}" -o 'jsonpath={.spec.containers[0].image}' 2>/dev/null || true)"
      if [[ "${phase}" == "Running" && "${ready}" == "True" && "${image}" == "${runtime_profile_sandbox_image}" ]]; then
        docs_warm_pod="${pod}"
        break
      fi
    done < <("${KUBECTL}" get pod -n "${sandbox_namespace}" -l "${docs_warm_selector}" -o name 2>/dev/null | sed 's|^pod/||')
  fi
  if (( SECONDS >= docs_deadline )); then
    printf 'SandboxWarmPool anydesign-fumadocs-docs-pool did not produce a current-image ready Pod\n' >&2
    exit 4
  fi
  [[ -n "${docs_warm_pod}" ]] || sleep 2
done
docs_image_id="$(${KUBECTL} get pod "${docs_warm_pod}" -n "${sandbox_namespace}" -o 'jsonpath={.status.containerStatuses[0].imageID}')"
if [[ "${docs_image_id}" != "${expected_image_id}" ]]; then
  printf 'docs sandbox image parity failed: expectedID=%s imageID=%s\n' "${expected_image_id}" "${docs_image_id}" >&2
  exit 4
fi
printf 'E2E_REPOSITORY_COMMIT=%s\n' "${git_sha}"
printf 'E2E_REPOSITORY_DIRTY_FILES=%s\n' "${dirty}"
printf 'E2E_K3D_CLUSTER=%s\n' "${cluster_name}"
printf 'E2E_SANDBOX_IMAGE=%s\n' "${pod_image}"
printf 'E2E_SANDBOX_IMAGE_ID=%s\n' "${pod_image_id}"
evidence_dir="${E2E_EVIDENCE_DIR:-services/runtime/target/e2e-evidence}"
if [[ "${evidence_dir}" != /* ]]; then
  evidence_dir="${ROOT_DIR}/${evidence_dir}"
fi
mkdir -p "${evidence_dir}"
evidence_path="${evidence_dir}/k3d-channel-${git_sha}.json"
public_runtime_evidence_path="${evidence_dir}/public-runtime-fixture-${git_sha}.json"

cargo test --manifest-path services/runtime/Cargo.toml \
  --test workspace_channel_mtls -- --nocapture

RUN_AGENT_SANDBOX_E2E=1 \
ANYDESIGN_E2E_SKIP_APPLY=1 \
ANYDESIGN_E2E_NAMESPACE="${sandbox_namespace}" \
KUBECTL="${KUBECTL}" \
WORKSPACE_CHANNEL_SIGNING_KEY_FILE="${private_key_file}" \
WORKSPACE_CHANNEL_TLS_MODE=required \
WORKSPACE_CHANNEL_CA_FILE="${channel_ca_bundle_file}" \
WORKSPACE_CHANNEL_CLIENT_CERT_FILE="${runtime_tls_cert_file}" \
WORKSPACE_CHANNEL_CLIENT_KEY_FILE="${runtime_tls_key_file}" \
WORKSPACE_CHANNEL_SERVER_SAN="spiffe://anydesign.local/ns/${sandbox_namespace}/sa/anydesign-sandbox" \
E2E_REPOSITORY_COMMIT="${git_sha}" \
E2E_REPOSITORY_DIRTY_FILES="${dirty}" \
E2E_K3D_CLUSTER="${cluster_name}" \
E2E_SANDBOX_IMAGE="${pod_image}" \
E2E_SANDBOX_IMAGE_ID="${pod_image_id}" \
E2E_EVIDENCE_PATH="${evidence_path}" \
cargo test --manifest-path services/runtime/Cargo.toml --test k8s_sandbox_e2e -- --nocapture

runtime_cert_serial_hash="$("${OPENSSL}" x509 -in "${runtime_tls_cert_file}" -noout -serial \
  | cut -d= -f2 | shasum -a 256 | awk '{print $1}')"
sandbox_cert_serial_hash="$("${OPENSSL}" x509 -in "${sandbox_tls_cert_file}" -noout -serial \
  | cut -d= -f2 | shasum -a 256 | awk '{print $1}')"
runtime_cert_expires_at="$("${OPENSSL}" x509 -in "${runtime_tls_cert_file}" -noout -enddate \
  | cut -d= -f2- | node -e 'const fs=require("fs");const d=new Date(fs.readFileSync(0,"utf8").trim());if(Number.isNaN(d.valueOf()))process.exit(2);process.stdout.write(d.toISOString())')"
sandbox_cert_expires_at="$("${OPENSSL}" x509 -in "${sandbox_tls_cert_file}" -noout -enddate \
  | cut -d= -f2- | node -e 'const fs=require("fs");const d=new Date(fs.readFileSync(0,"utf8").trim());if(Number.isNaN(d.valueOf()))process.exit(2);process.stdout.write(d.toISOString())')"
node -e '
const fs=require("fs");
const evidence=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));
evidence.transport={
  mode:"mtls",mtlsVerified:true,rotationWindowVerified:true,
  runtimeSanHash:process.argv[2],sandboxSanHash:process.argv[3],
  runtimeCertSerialHash:process.argv[4],sandboxCertSerialHash:process.argv[5],
  runtimeCertExpiresAt:process.argv[6],sandboxCertExpiresAt:process.argv[7]
};
fs.writeFileSync(process.argv[1],JSON.stringify(evidence,null,2)+"\n");
' "${evidence_path}" \
  "$(printf '%s' 'spiffe://anydesign.local/ns/anydesign-runtime/sa/anydesign-runtime' | shasum -a 256 | awk '{print $1}')" \
  "$(printf '%s' "spiffe://anydesign.local/ns/${sandbox_namespace}/sa/anydesign-sandbox" | shasum -a 256 | awk '{print $1}')" \
  "${runtime_cert_serial_hash}" "${sandbox_cert_serial_hash}" \
  "${runtime_cert_expires_at}" "${sandbox_cert_expires_at}"

browser_executable="${RUNTIME_BROWSER_EXECUTABLE:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
if [[ ! -x "${browser_executable}" ]]; then
  printf 'Runtime browser executable is required for the Public Runtime k3d gate: %s\n' \
    "${browser_executable}" >&2
  exit 5
fi
RUN_PUBLIC_RUNTIME_K8S_E2E=1 \
ANYDESIGN_E2E_NAMESPACE="${sandbox_namespace}" \
KUBECTL="${KUBECTL}" \
WORKSPACE_CHANNEL_SIGNING_KEY_FILE="${private_key_file}" \
WORKSPACE_CHANNEL_TLS_MODE=required \
WORKSPACE_CHANNEL_CA_FILE="${channel_ca_bundle_file}" \
WORKSPACE_CHANNEL_CLIENT_CERT_FILE="${runtime_tls_cert_file}" \
WORKSPACE_CHANNEL_CLIENT_KEY_FILE="${runtime_tls_key_file}" \
WORKSPACE_CHANNEL_SERVER_SAN="spiffe://anydesign.local/ns/${sandbox_namespace}/sa/anydesign-sandbox" \
RUNTIME_BROWSER_EXECUTABLE="${browser_executable}" \
SANDBOX_CHANNEL_TRANSPORT=port_forward \
E2E_REPOSITORY_COMMIT="${git_sha}" \
E2E_REPOSITORY_DIRTY_FILES="${dirty}" \
E2E_K3D_CLUSTER="${cluster_name}" \
E2E_SANDBOX_IMAGE="${pod_image}" \
E2E_SANDBOX_IMAGE_ID="${pod_image_id}" \
PUBLIC_RUNTIME_EVIDENCE_PATH="${public_runtime_evidence_path}" \
RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}" \
cargo test --manifest-path services/runtime/Cargo.toml --test k8s_public_runtime_e2e -- --nocapture

test -s "${evidence_path}"
test -s "${public_runtime_evidence_path}"
cp "${evidence_path}" "${evidence_dir}/k3d-channel.json"
cp "${public_runtime_evidence_path}" "${evidence_dir}/public-runtime-fixture.json"
node -e '
const fs = require("fs");
const evidence = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
for (const value of [
  evidence.repository?.commit,
  evidence.cluster?.name,
  evidence.cluster?.kubeContext,
  evidence.sandbox?.imageRef,
  evidence.sandbox?.imageId,
]) {
  if (typeof value !== "string" || value.length === 0) process.exit(2);
}
if (!Array.isArray(evidence.claims) || evidence.claims.length !== 2) process.exit(3);
if (!Object.values(evidence.checks || {}).every((value) => value === true)) process.exit(4);
' "${evidence_path}"
node -e '
const fs = require("fs");
const evidence = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
if (evidence.provider?.mode !== "fixture") process.exit(2);
if (!Array.isArray(evidence.projects) || evidence.projects.length !== 2) process.exit(3);
for (const project of evidence.projects) {
  for (const key of ["kind", "projectId", "lifecycle", "buildRunId", "editRunId", "sandboxBindingId", "podUid", "sourceSnapshotUri", "sandboxReleasedAt"]) {
    if (typeof project[key] !== "string" || project[key].length === 0) process.exit(4);
  }
  if (project.buildRunId === project.editRunId) process.exit(4);
  if (project.lifecycle === "draft") {
    for (const key of ["draftSnapshotBeforeEdit", "draftSnapshotAfterEdit", "sourceHashBeforeEdit", "sourceHashAfterEdit", "previewSessionId"]) {
      if (typeof project[key] !== "string" || project[key].length === 0) process.exit(4);
    }
    if (project.draftSnapshotBeforeEdit === project.draftSnapshotAfterEdit || project.sourceHashBeforeEdit === project.sourceHashAfterEdit || project.workVersionCreated !== false || project.artifactHttpStatusAfterRelease !== 404 || project.events?.terminalSeen !== true || project.workspaceRevision !== project.durableRevision) process.exit(5);
  } else if (project.lifecycle === "work-version") {
    for (const key of ["currentVersionAfterCas", "buildId", "candidateManifestHash", "previewLeaseId", "artifactManifestHash", "artifactUri", "artifactUrl"]) {
      if (typeof project[key] !== "string" || project[key].length === 0) process.exit(4);
    }
    if (project.currentVersionAfterCas !== project.versionAfterCas || project.versionBeforeCas === project.versionAfterCas || project.artifactHttpStatusAfterRelease !== 200 || project.previewLeaseStatusAfterRelease !== "stopped" || project.events?.sequenceValid !== true) process.exit(6);
  } else process.exit(4);
  if (project.screenshot?.pngSha256?.length !== 64 || project.screenshot?.documentSha256?.length !== 64 || project.screenshot?.nonblankPixelRatio <= 0.0005) process.exit(7);
  if (project.kind === "website") {
    const dcp = project.designContext;
    if (!dcp || !/^[a-f0-9]{64}$/.test(dcp.contentHash || "") || !/^[a-f0-9]{64}$/.test(dcp.artifactManifestHash || "") || !/^[a-f0-9]{64}$/.test(dcp.materializationHash || "") || dcp.effectiveCompatibilityMode !== "observe" || !Array.isArray(dcp.readFiles) || !["inputs/design-profile.json", "inputs/design-profile-usage.md", "inputs/component-recipes.json", "state/style-contract.json"].every((path) => dcp.readFiles.includes(path))) process.exit(10);
  }
}
if (evidence.projects[0].screenshot.documentSha256 === evidence.projects[1].screenshot.documentSha256) process.exit(9);
' "${public_runtime_evidence_path}"
if rg -n -i '(bearer[[:space:]]+[a-z0-9._-]+|sk-[a-z0-9]{16,}|api[_-]?key["[:space:]]*:)' \
  "${evidence_path}" "${public_runtime_evidence_path}"; then
  printf 'secret-like value found in E2E evidence\n' >&2
  exit 5
fi
printf 'E2E_EVIDENCE_PATH=%s\n' "${evidence_path}"
printf 'PUBLIC_RUNTIME_EVIDENCE_PATH=%s\n' "${public_runtime_evidence_path}"
