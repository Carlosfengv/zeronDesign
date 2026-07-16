#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
cluster_name="${ANYDESIGN_E2E_CLUSTER:?ANYDESIGN_E2E_CLUSTER is required}"
proxy_host="anydesign-npm-proxy.anydesign-runtime.svc.cluster.local"
evidence_path="${NPM_PROXY_EVIDENCE_PATH:-${ROOT_DIR}/services/runtime/target/e2e-evidence/npm-proxy-${cluster_name}.json}"
project_evidence_file="${NPM_PROXY_PROJECT_EVIDENCE_FILE:-}"
cd "${ROOT_DIR}"

context="$(${KUBECTL} config current-context)"
[[ "${context}" == "k3d-${cluster_name}" ]] || {
  printf 'npm gate requires context k3d-%s; got %s\n' "${cluster_name}" "${context}" >&2
  exit 2
}
[[ -s "${project_evidence_file}" ]] || {
  printf 'NPM_PROXY_PROJECT_EVIDENCE_FILE is required to prove Runtime-driven installs\n' >&2
  exit 3
}

projects_json="$(node -e '
const fs=require("fs");
const projects=JSON.parse(fs.readFileSync(process.argv[1],"utf8")).projects;
if(projects.length<2||!projects.some(p=>p.kind==="website")||!projects.some(p=>p.kind==="docs"))process.exit(2);
for(const project of projects){
  const d=project.dependencyEvidence;
  if(d?.passed!==true||d?.nodeModulesPresent!==true||!/^[a-f0-9]{64}$/.test(d?.lockfileSha256||"")||!(d?.tarballRequestCount>0))process.exit(3);
  if(typeof project.podUid!=="string"||project.podUid.length===0)process.exit(5);
  if(d.podUid!==project.podUid||typeof d.pod!=="string"||d.pod.length===0||typeof d.podIp!=="string"||d.podIp.length===0)process.exit(6);
  if(d.networkChecks?.dnsResolved!==true||d.networkChecks?.proxyReachable!==true||d.networkChecks?.directNpmjsDenied!==true)process.exit(7);
}
if(projects[0].podUid===projects[1].podUid)process.exit(4);
process.stdout.write(JSON.stringify(projects.map(p=>({kind:p.kind,projectId:p.projectId,...p.dependencyEvidence}))));
' "${project_evidence_file}")" || {
  printf 'Runtime dependency install evidence is missing or not isolated\n' >&2
  exit 7
}

sandbox_pods_json="$(node -e '
const projects=JSON.parse(process.argv[1]);
process.stdout.write(JSON.stringify(projects.map(project=>({
  kind:project.kind,projectId:project.projectId,podUid:project.podUid,
  podName:project.pod,podIp:project.podIp,networkChecks:project.networkChecks,
}))));
' "${projects_json}")"

proxy_logs_file="$(mktemp)"
trap 'rm -f "${proxy_logs_file}"' EXIT
${KUBECTL} logs -n anydesign-runtime deployment/anydesign-npm-proxy --since=30m >"${proxy_logs_file}"
ping_count="$(rg -F -c '/-/ping' "${proxy_logs_file}" || true)"
project_count="$(node -e 'process.stdout.write(String(JSON.parse(process.argv[1]).length))' "${projects_json}")"
if (( ping_count < project_count )); then
  printf 'Verdaccio access log has fewer Sandbox ping requests than Runtime project Pods: expected=%s actual=%s\n' \
    "${project_count}" "${ping_count}" >&2
  exit 5
fi

cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools \
  project_ensure_dependencies_registry_failure_is_typed_infrastructure_error -- --exact >/dev/null

mkdir -p "$(dirname "${evidence_path}")"
node -e '
const fs=require("fs");
fs.writeFileSync(process.argv[1],JSON.stringify({
  schemaVersion:"npm-proxy-gate@2",cluster:process.argv[2],kubeContext:process.argv[3],
  sandboxPods:JSON.parse(process.argv[4]),proxyHost:process.argv[5],
  projects:JSON.parse(process.argv[6]),
  checks:{
    dnsResolved:true,proxyReachable:true,directNpmjsDenied:true,accessLogObserved:true,
    runtimeInstallObserved:true,lockfilesPresent:true,projectIsolation:true,
    upstreamFailureTyped:"infrastructure.registry_unavailable"
  },result:"pass"
},null,2)+"\n");
' "${evidence_path}" "${cluster_name}" "${context}" "${sandbox_pods_json}" "${proxy_host}" "${projects_json}"
printf 'npm proxy gate passed: %s\n' "${evidence_path}"
