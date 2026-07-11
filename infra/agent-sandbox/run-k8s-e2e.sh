#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
K3D="${K3D:-k3d}"
cd "${ROOT_DIR}"

cluster_name="${ANYDESIGN_E2E_CLUSTER:-zerondesign-e2e}"
if ! "${K3D}" cluster list --no-headers 2>/dev/null | awk '{print $1}' | grep -Fxq "${cluster_name}"; then
  "${K3D}" cluster create "${cluster_name}" --servers 1 --agents 0 --wait
fi
"${KUBECTL}" config use-context "k3d-${cluster_name}" >/dev/null
context="$(${KUBECTL} config current-context 2>/dev/null || true)"
if [[ "${context}" != "k3d-${cluster_name}" ]]; then
  printf 'failed to select dedicated k3d context k3d-%s; got %s\n' \
    "${cluster_name}" "${context:-<none>}" >&2
  exit 2
fi
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
sandbox_image="anydesign/astro-website-sandbox:${image_tag}"
sandbox_base_image="${SANDBOX_BASE_IMAGE:-node:22-bookworm}"

docker build \
  -f infra/agent-sandbox/astro-website/Dockerfile \
  --provenance=false \
  --build-arg "SANDBOX_BASE_IMAGE=${sandbox_base_image}" \
  -t "${sandbox_image}" \
  infra/agent-sandbox
expected_image_id="sha256:$(docker image save "${sandbox_image}" \
  | tar -xOf - manifest.json \
  | node -e 'const fs=require("fs");const manifest=JSON.parse(fs.readFileSync(0,"utf8"));process.stdout.write(manifest[0].Config.split("/").pop())')"
"${K3D}" image import --cluster "${cluster_name}" "${sandbox_image}"

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
  bash infra/agent-sandbox/install-controller.sh
fi

"${KUBECTL}" apply -f infra/agent-sandbox/rbac/runtime-service-account.yaml

key_dir="$(mktemp -d)"
cleanup_key_dir() {
  "${KUBECTL}" get sandboxclaim -n anydesign-sandboxes -o name 2>/dev/null \
    | rg '^sandboxclaim[^/]*/project-(website-k3d|docs-k3d)-' \
    | xargs -r "${KUBECTL}" delete -n anydesign-sandboxes --ignore-not-found=true >/dev/null 2>&1 || true
  rm -rf "${key_dir}"
}
trap cleanup_key_dir EXIT

signer_secret="anydesign-workspace-channel-signer"
verifier_config_map="anydesign-workspace-channel-verifier"
private_key_file="${key_dir}/private.der"
public_key_file="${key_dir}/public.der"
previous_public_key_file="${key_dir}/previous-public.der"

if "${KUBECTL}" get secret "${signer_secret}" -n anydesign-runtime >/dev/null 2>&1; then
  "${KUBECTL}" get secret "${signer_secret}" \
    -n anydesign-runtime \
    -o 'jsonpath={.data.private\.der}' \
    | openssl base64 -d -A >"${private_key_file}"
else
  openssl genpkey -algorithm ED25519 -outform DER -out "${private_key_file}"
  "${KUBECTL}" create secret generic "${signer_secret}" \
    -n anydesign-runtime \
    --from-file="private.der=${private_key_file}" \
    --dry-run=client \
    -o yaml \
    | "${KUBECTL}" apply -f -
fi

openssl pkey \
  -inform DER \
  -in "${private_key_file}" \
  -pubout \
  -outform DER \
  -out "${public_key_file}"
if "${KUBECTL}" get configmap "${verifier_config_map}" -n anydesign-sandboxes >/dev/null 2>&1; then
  "${KUBECTL}" get configmap "${verifier_config_map}" \
    -n anydesign-sandboxes \
    -o 'jsonpath={.binaryData.current\.der}' \
    | openssl base64 -d -A >"${previous_public_key_file}" || true
fi
if [[ ! -s "${previous_public_key_file}" ]]; then
  cp "${public_key_file}" "${previous_public_key_file}"
fi
"${KUBECTL}" create configmap "${verifier_config_map}" \
  -n anydesign-sandboxes \
  --from-file="current.der=${public_key_file}" \
  --from-file="previous.der=${previous_public_key_file}" \
  --dry-run=client \
  -o yaml \
  | "${KUBECTL}" apply -f -

"${KUBECTL}" apply -f infra/agent-sandbox/network/default-deny.yaml
"${KUBECTL}" apply -f infra/agent-sandbox/npm-proxy/config-map.yaml
"${KUBECTL}" apply -f infra/agent-sandbox/npm-proxy/deployment.yaml
"${KUBECTL}" apply -f infra/agent-sandbox/npm-proxy/service.yaml
sed "s|image: ghcr.io/carlosfengv/zerondesign/astro-website-sandbox:0.1.0|image: ${sandbox_image}|" \
  infra/agent-sandbox/astro-website/sandbox-template.yaml \
  | "${KUBECTL}" apply -f -
"${KUBECTL}" apply -f infra/agent-sandbox/astro-website/sandbox-warm-pool.yaml
sed "s|image: ghcr.io/carlosfengv/zerondesign/astro-website-sandbox:0.1.0|image: ${sandbox_image}|" \
  infra/agent-sandbox/fumadocs-docs/sandbox-template.yaml \
  | "${KUBECTL}" apply -f -
"${KUBECTL}" apply -f infra/agent-sandbox/fumadocs-docs/sandbox-warm-pool.yaml

"${KUBECTL}" rollout status deployment/anydesign-npm-proxy \
  -n anydesign-runtime \
  --timeout=180s

if [[ "${ANYDESIGN_E2E_RESET_WARM_POOL:-1}" == "1" ]]; then
  "${KUBECTL}" delete sandboxes.agents.x-k8s.io \
    -n anydesign-sandboxes \
    -l agents.x-k8s.io/launch-type=warm \
    --ignore-not-found=true
fi

deadline=$((SECONDS + 180))
warm_pod=""
while true; do
  ready_replicas="$("${KUBECTL}" get sandboxwarmpool anydesign-astro-website-pool \
    -n anydesign-sandboxes \
    -o 'jsonpath={.status.readyReplicas}' 2>/dev/null || true)"
  warm_selector="$("${KUBECTL}" get sandboxwarmpool anydesign-astro-website-pool \
    -n anydesign-sandboxes \
    -o 'jsonpath={.status.selector}' 2>/dev/null || true)"
  if [[ "${ready_replicas:-0}" -ge 1 && -n "${warm_selector}" ]]; then
    while IFS= read -r pod; do
      [[ -n "${pod}" ]] || continue
      phase="$("${KUBECTL}" get pod "${pod}" -n anydesign-sandboxes \
        -o 'jsonpath={.status.phase}' 2>/dev/null || true)"
      ready="$("${KUBECTL}" get pod "${pod}" -n anydesign-sandboxes \
        -o 'jsonpath={.status.conditions[?(@.type=="Ready")].status}' 2>/dev/null || true)"
      image="$("${KUBECTL}" get pod "${pod}" -n anydesign-sandboxes \
        -o 'jsonpath={.spec.containers[0].image}' 2>/dev/null || true)"
      if [[ "${phase}" == "Running" && "${ready}" == "True" && "${image}" == "${sandbox_image}" ]]; then
        warm_pod="${pod}"
        break
      fi
    done < <("${KUBECTL}" get pod -n anydesign-sandboxes \
      -l "${warm_selector}" \
      -o name 2>/dev/null | sed 's|^pod/||')
  fi
  if [[ -n "${warm_pod}" ]]; then
    break
  fi
  if (( SECONDS >= deadline )); then
    printf 'SandboxWarmPool anydesign-astro-website-pool did not become ready; readyReplicas=%s\n' "${ready_replicas:-0}" >&2
    exit 3
  fi
  sleep 2
done

pod_image="$(${KUBECTL} get pod "${warm_pod}" -n anydesign-sandboxes \
  -o 'jsonpath={.spec.containers[0].image}')"
pod_image_id="$(${KUBECTL} get pod "${warm_pod}" -n anydesign-sandboxes \
  -o 'jsonpath={.status.containerStatuses[0].imageID}')"
if [[ "${pod_image}" != "${sandbox_image}" || "${pod_image_id}" != "${expected_image_id}" ]]; then
  printf 'sandbox image parity failed: expected=%s expectedID=%s actual=%s imageID=%s\n' \
    "${sandbox_image}" "${expected_image_id}" "${pod_image}" "${pod_image_id}" >&2
  exit 4
fi
docs_deadline=$((SECONDS + 180))
docs_warm_pod=""
while [[ -z "${docs_warm_pod}" ]]; do
  docs_ready_replicas="$("${KUBECTL}" get sandboxwarmpool anydesign-fumadocs-docs-pool \
    -n anydesign-sandboxes -o 'jsonpath={.status.readyReplicas}' 2>/dev/null || true)"
  docs_warm_selector="$("${KUBECTL}" get sandboxwarmpool anydesign-fumadocs-docs-pool \
    -n anydesign-sandboxes -o 'jsonpath={.status.selector}' 2>/dev/null || true)"
  if [[ "${docs_ready_replicas:-0}" -ge 1 && -n "${docs_warm_selector}" ]]; then
    while IFS= read -r pod; do
      [[ -n "${pod}" ]] || continue
      phase="$("${KUBECTL}" get pod "${pod}" -n anydesign-sandboxes -o 'jsonpath={.status.phase}' 2>/dev/null || true)"
      ready="$("${KUBECTL}" get pod "${pod}" -n anydesign-sandboxes -o 'jsonpath={.status.conditions[?(@.type=="Ready")].status}' 2>/dev/null || true)"
      image="$("${KUBECTL}" get pod "${pod}" -n anydesign-sandboxes -o 'jsonpath={.spec.containers[0].image}' 2>/dev/null || true)"
      if [[ "${phase}" == "Running" && "${ready}" == "True" && "${image}" == "${sandbox_image}" ]]; then
        docs_warm_pod="${pod}"
        break
      fi
    done < <("${KUBECTL}" get pod -n anydesign-sandboxes -l "${docs_warm_selector}" -o name 2>/dev/null | sed 's|^pod/||')
  fi
  if (( SECONDS >= docs_deadline )); then
    printf 'SandboxWarmPool anydesign-fumadocs-docs-pool did not produce a current-image ready Pod\n' >&2
    exit 4
  fi
  [[ -n "${docs_warm_pod}" ]] || sleep 2
done
docs_image_id="$(${KUBECTL} get pod "${docs_warm_pod}" -n anydesign-sandboxes -o 'jsonpath={.status.containerStatuses[0].imageID}')"
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

RUN_AGENT_SANDBOX_E2E=1 \
ANYDESIGN_E2E_SKIP_APPLY=1 \
KUBECTL="${KUBECTL}" \
WORKSPACE_CHANNEL_SIGNING_KEY_FILE="${private_key_file}" \
E2E_REPOSITORY_COMMIT="${git_sha}" \
E2E_REPOSITORY_DIRTY_FILES="${dirty}" \
E2E_K3D_CLUSTER="${cluster_name}" \
E2E_SANDBOX_IMAGE="${pod_image}" \
E2E_SANDBOX_IMAGE_ID="${pod_image_id}" \
E2E_EVIDENCE_PATH="${evidence_path}" \
cargo test --manifest-path services/runtime/Cargo.toml --test k8s_sandbox_e2e -- --nocapture

browser_executable="${RUNTIME_BROWSER_EXECUTABLE:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
if [[ ! -x "${browser_executable}" ]]; then
  printf 'Runtime browser executable is required for the Public Runtime k3d gate: %s\n' \
    "${browser_executable}" >&2
  exit 5
fi
RUN_PUBLIC_RUNTIME_K8S_E2E=1 \
KUBECTL="${KUBECTL}" \
WORKSPACE_CHANNEL_SIGNING_KEY_FILE="${private_key_file}" \
RUNTIME_BROWSER_EXECUTABLE="${browser_executable}" \
SANDBOX_CHANNEL_TRANSPORT=port_forward \
E2E_REPOSITORY_COMMIT="${git_sha}" \
E2E_REPOSITORY_DIRTY_FILES="${dirty}" \
E2E_K3D_CLUSTER="${cluster_name}" \
E2E_SANDBOX_IMAGE="${pod_image}" \
E2E_SANDBOX_IMAGE_ID="${pod_image_id}" \
PUBLIC_RUNTIME_EVIDENCE_PATH="${public_runtime_evidence_path}" \
cargo test --manifest-path services/runtime/Cargo.toml --test k8s_public_runtime_e2e -- --nocapture

test -s "${evidence_path}"
test -s "${public_runtime_evidence_path}"
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
  for (const key of ["kind", "projectId", "runId", "sandboxBindingId", "podUid", "versionId", "buildId", "candidateManifestHash", "sourceSnapshotUri", "previewLeaseId", "artifactManifestHash", "artifactUri", "artifactUrl", "sandboxReleasedAt"]) {
    if (typeof project[key] !== "string" || project[key].length === 0) process.exit(4);
  }
  if (project.artifactHttpStatusAfterRelease !== 200) process.exit(5);
  if (project.previewLeaseStatusAfterRelease !== "stopped") process.exit(6);
  if (project.screenshot?.pngSha256?.length !== 64 || project.screenshot?.documentSha256?.length !== 64 || project.screenshot?.nonblankPixelRatio <= 0.0005) process.exit(7);
  if (project.events?.sequenceValid !== true) process.exit(8);
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
