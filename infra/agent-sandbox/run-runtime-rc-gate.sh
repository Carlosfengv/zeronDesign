#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
K3D="${K3D:-k3d}"
cluster_name="${ANYDESIGN_E2E_CLUSTER:-zerondesign-e2e}"
runtime_port="${RUNTIME_RC_PORT:-18080}"
cd "${ROOT_DIR}"

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

git_sha="$(git rev-parse --short=12 HEAD)"
dirty_count="$(git status --porcelain | wc -l | tr -d ' ')"
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

if [[ -n "${RUNTIME_RC_REUSE_IMAGE:-}" ]]; then
  runtime_image="${RUNTIME_RC_REUSE_IMAGE}"
  docker image inspect "${runtime_image}" >/dev/null
else
  rm -rf services/runtime/target/docker-vendor
  vendor_log="services/runtime/target/docker-vendor.log"
  if ! cargo vendor \
    --manifest-path services/runtime/Cargo.toml \
    --locked \
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
    -t "${runtime_image}" \
    .
fi
expected_image_id="sha256:$(docker image save "${runtime_image}" \
  | tar -xOf - manifest.json \
  | node -e 'const fs=require("fs");const m=JSON.parse(fs.readFileSync(0,"utf8"));process.stdout.write(m[0].Config.split("/").pop())')"
"${K3D}" image import --cluster "${cluster_name}" "${runtime_image}"

"${KUBECTL}" create configmap fixture-model-gateway \
  -n anydesign-runtime \
  --from-file="fixture-model-gateway.js=infra/agent-sandbox/runtime/fixture-model-gateway.js" \
  --dry-run=client -o yaml | "${KUBECTL}" apply -f -
"${KUBECTL}" apply -f infra/agent-sandbox/runtime/fixture-model-gateway.yaml
sed "s|image: anydesign/runtime:dev|image: ${runtime_image}|" \
  infra/agent-sandbox/runtime/deployment.yaml | "${KUBECTL}" apply -f -
"${KUBECTL}" rollout restart deployment/fixture-model-gateway -n anydesign-runtime
"${KUBECTL}" rollout status deployment/fixture-model-gateway -n anydesign-runtime --timeout=180s
"${KUBECTL}" rollout status deployment/anydesign-runtime -n anydesign-runtime --timeout=300s

runtime_pod="$(${KUBECTL} get pods -n anydesign-runtime -l app=anydesign-runtime \
  -o jsonpath='{.items[0].metadata.name}')"
pod_image="$(${KUBECTL} get pod "${runtime_pod}" -n anydesign-runtime \
  -o jsonpath='{.spec.containers[0].image}')"
pod_image_id="$(${KUBECTL} get pod "${runtime_pod}" -n anydesign-runtime \
  -o jsonpath='{.status.containerStatuses[0].imageID}')"
if [[ "${pod_image}" != "${runtime_image}" || "${pod_image_id}" != "${expected_image_id}" ]]; then
  printf 'Runtime image parity failed: expected=%s id=%s actual=%s id=%s\n' \
    "${runtime_image}" "${expected_image_id}" "${pod_image}" "${pod_image_id}" >&2
  exit 3
fi

"${KUBECTL}" port-forward -n anydesign-runtime service/anydesign-runtime \
  "${runtime_port}:8080" >/tmp/anydesign-runtime-rc-port-forward.log 2>&1 &
port_forward_pid=$!
cleanup() {
  kill "${port_forward_pid}" >/dev/null 2>&1 || true
}
trap cleanup EXIT
base_url="http://127.0.0.1:${runtime_port}"
for _ in $(seq 1 60); do
  curl --fail --silent "${base_url}/health" >/dev/null 2>&1 && break
  sleep 1
done
version_json="$(curl --fail --silent "${base_url}/version")"
node -e '
const v=JSON.parse(process.argv[1]);
if(v.repositoryCommit!==process.argv[2]||v.imageRef!==process.argv[3]) {
  throw new Error(`Runtime version mismatch: ${JSON.stringify(v)}`);
}
' "${version_json}" "${git_sha}" "${runtime_image}"

run_fixture() {
  local project_id="$1"
  local kind="$2"
  local expected_text="$3"
  local brief_payload brief_run conversation brief_id build_payload build_run events artifact_url
  brief_payload="$(curl --fail --silent \
    -H 'content-type: application/json' \
    -d "{\"projectId\":\"${project_id}\",\"phase\":\"brief\",\"agentProfile\":\"brief\",\"inputContext\":{\"contentSources\":[{\"id\":\"source-1\",\"kind\":\"prompt\",\"text\":\"Create a ${kind} RC fixture\",\"readable\":true}]}}" \
    "${base_url}/runs")"
  brief_run="$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).runId)' "${brief_payload}")"
  for _ in $(seq 1 120); do
    conversation="$(curl --fail --silent "${base_url}/projects/${project_id}/conversation?includeDebug=true")"
    if [[ "${conversation}" == *"confirmation_requested"* || "${conversation}" == *"Confirm this deterministic"* ]]; then
      break
    fi
    sleep 1
  done
  curl --fail --silent \
    -H 'content-type: application/json' \
    -d '{"userMessage":"confirm"}' \
    "${base_url}/runs/${brief_run}/continue" >/dev/null
  conversation="$(curl --fail --silent "${base_url}/projects/${project_id}/conversation?includeDebug=true")"
  brief_id="$(node -e '
const c=JSON.parse(process.argv[1]);
const item=[...c.items].reverse().find(x=>x.metadata&&x.metadata.briefId);
if(!item) throw new Error("briefId missing from conversation");
process.stdout.write(item.metadata.briefId);
' "${conversation}")"
  build_payload="$(curl --fail --silent \
    -H 'content-type: application/json' \
    -d "{\"projectId\":\"${project_id}\",\"phase\":\"build\",\"agentProfile\":\"build\",\"inputContext\":{\"briefId\":\"${brief_id}\"}}" \
    "${base_url}/runs")"
  build_run="$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).runId)' "${build_payload}")"
  events="$(curl --fail --silent --max-time 240 "${base_url}/runs/${build_run}/events")"
  if [[ "${events}" != *'"type":"run.completed"'* || "${events}" != *'"status":"completed"'* ]]; then
    printf 'Build did not complete: project=%s run=%s\n%s\n' "${project_id}" "${build_run}" "${events}" >&2
    exit 4
  fi
  artifact_url="${base_url}/artifacts/${project_id}/current/"
  curl --fail --silent "${artifact_url}" | rg -F "${expected_text}" >/dev/null
  printf '{"projectId":"%s","briefRunId":"%s","briefId":"%s","buildRunId":"%s","artifactUrl":"%s"}' \
    "${project_id}" "${brief_run}" "${brief_id}" "${build_run}" "${artifact_url}"
}

gate_id="$(date +%s)"
website="$(run_fixture "rc-website-${gate_id}" website 'RC Website')"
docs="$(run_fixture "rc-docs-${gate_id}" docs 'RC Docs')"
mkdir -p services/runtime/target/e2e-evidence
evidence="services/runtime/target/e2e-evidence/runtime-rc-${image_tag}.json"
node -e '
const fs=require("fs");
const payload={
  recordedAt:new Date().toISOString(),
  runtimeVersion:JSON.parse(process.argv[2]),
  runtimePod:process.argv[3],
  runtimeImage:process.argv[4],
  runtimeImageId:process.argv[5],
  website:JSON.parse(process.argv[6]),
  docs:JSON.parse(process.argv[7]),
};
fs.writeFileSync(process.argv[1],JSON.stringify(payload,null,2)+"\n");
' "${evidence}" "${version_json}" "${runtime_pod}" "${runtime_image}" "${pod_image_id}" "${website}" "${docs}"
printf 'Runtime RC gate passed: %s\n' "${evidence}"
