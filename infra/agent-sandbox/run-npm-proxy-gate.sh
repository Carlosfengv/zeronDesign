#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
cluster_name="${ANYDESIGN_E2E_CLUSTER:?ANYDESIGN_E2E_CLUSTER is required}"
namespace="${ANYDESIGN_E2E_NAMESPACE:-anydesign-sandboxes}"
proxy_host="anydesign-npm-proxy.anydesign-runtime.svc.cluster.local"
evidence_path="${NPM_PROXY_EVIDENCE_PATH:-${ROOT_DIR}/services/runtime/target/e2e-evidence/npm-proxy-${cluster_name}.json}"
project_evidence_file="${NPM_PROXY_PROJECT_EVIDENCE_FILE:-}"
cd "${ROOT_DIR}"

context="$(${KUBECTL} config current-context)"
[[ "${context}" == "k3d-${cluster_name}" ]] || {
  printf 'npm gate requires context k3d-%s; got %s\n' "${cluster_name}" "${context}" >&2
  exit 2
}
pod="$(${KUBECTL} get pods -n "${namespace}" --no-headers \
  | awk '$3=="Running" {print $1; exit}')"
[[ -n "${pod}" ]] || { printf 'ready Sandbox Pod is required\n' >&2; exit 3; }
[[ -s "${project_evidence_file}" ]] || {
  printf 'NPM_PROXY_PROJECT_EVIDENCE_FILE is required to prove Runtime-driven installs\n' >&2
  exit 3
}

${KUBECTL} exec -n "${namespace}" "${pod}" -- node -e \
  "require('dns').lookup('${proxy_host}',e=>{if(e)throw e})"
${KUBECTL} exec -n "${namespace}" "${pod}" -- node -e \
  "fetch('http://${proxy_host}:4873/-/ping').then(r=>{if(!r.ok)process.exit(2)})"
if ${KUBECTL} exec -n "${namespace}" "${pod}" -- node -e \
  "const t=setTimeout(()=>process.exit(0),4000);fetch('https://registry.npmjs.org/').then(()=>{clearTimeout(t);process.exit(3)}).catch(()=>{clearTimeout(t);process.exit(0)})"; then
  :
else
  printf 'direct public npm registry unexpectedly reachable\n' >&2
  exit 4
fi

proxy_logs_file="$(mktemp)"
trap 'rm -f "${proxy_logs_file}"' EXIT
${KUBECTL} logs -n anydesign-runtime deployment/anydesign-npm-proxy --since=30m >"${proxy_logs_file}"
if ! rg -F '/-/ping' "${proxy_logs_file}" >/dev/null; then
  printf 'Verdaccio access log does not contain the Sandbox ping request\n' >&2
  exit 5
fi

projects_json="$(node -e '
const fs=require("fs");
const projects=JSON.parse(fs.readFileSync(process.argv[1],"utf8")).projects;
if(projects.length<2||!projects.some(p=>p.kind==="website")||!projects.some(p=>p.kind==="docs"))process.exit(2);
for(const project of projects){
  const d=project.dependencyEvidence;
  if(d?.passed!==true||d?.nodeModulesPresent!==true||!/^[a-f0-9]{64}$/.test(d?.lockfileSha256||"")||!(d?.tarballRequestCount>0))process.exit(3);
}
if(projects[0].podUid===projects[1].podUid)process.exit(4);
process.stdout.write(JSON.stringify(projects.map(p=>({kind:p.kind,projectId:p.projectId,...p.dependencyEvidence}))));
' "${project_evidence_file}")" || {
  printf 'Runtime dependency install evidence is missing or not isolated\n' >&2
  exit 7
}
cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools \
  project_ensure_dependencies_registry_failure_is_typed_infrastructure_error -- --exact >/dev/null

mkdir -p "$(dirname "${evidence_path}")"
node -e '
const fs=require("fs");
fs.writeFileSync(process.argv[1],JSON.stringify({
  schemaVersion:"npm-proxy-gate@1",cluster:process.argv[2],kubeContext:process.argv[3],
  sandboxPod:process.argv[4],proxyHost:process.argv[5],
  projects:JSON.parse(process.argv[6]),
  checks:{
    dnsResolved:true,proxyReachable:true,directNpmjsDenied:true,accessLogObserved:true,
    runtimeInstallObserved:true,lockfilesPresent:true,projectIsolation:true,
    upstreamFailureTyped:"infrastructure.registry_unavailable"
  },result:"pass"
},null,2)+"\n");
' "${evidence_path}" "${cluster_name}" "${context}" "${pod}" "${proxy_host}" "${projects_json}"
printf 'npm proxy gate passed: %s\n' "${evidence_path}"
