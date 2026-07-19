#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  printf 'usage: %s <source-workspace> <other-workspace>\n' "$0" >&2
  exit 64
fi

source_workspace="$1"
other_workspace="$2"
runtime_namespace="${RUNTIME_SYSTEM_NAMESPACE:-anydesign-runtime}"
kubectl_bin="${KUBECTL:-kubectl}"
sandbox_image="${SANDBOX_IMAGE:-ghcr.io/carlosfengv/zerondesign/astro-website-sandbox:0.1.0}"
smoke_name="${WORKSPACE_ISOLATION_SMOKE_NAME:-workspace-isolation-smoke}"
evidence_path="${WORKSPACE_ISOLATION_EVIDENCE_PATH:-}"
client_name="${smoke_name}-runtime-client"
pvc_name="workspace-${smoke_name}"

if [[ "${source_workspace}" == "${other_workspace}" ]]; then
  printf 'source and other Workspace must be different\n' >&2
  exit 64
fi

cleanup() {
  "${kubectl_bin}" delete pod "${client_name}" -n "${runtime_namespace}" \
    --ignore-not-found --wait=false >/dev/null 2>&1 || true
  "${kubectl_bin}" delete sandboxclaim "${smoke_name}" -n "${source_workspace}" \
    --ignore-not-found --wait=false >/dev/null 2>&1 || true
  "${kubectl_bin}" wait --for=delete "pod/${smoke_name}" \
    "pvc/${pvc_name}" -n "${source_workspace}" --timeout=60s \
    >/dev/null 2>&1 || true
}
trap cleanup EXIT

for workspace_namespace in "${source_workspace}" "${other_workspace}"; do
  workspace_label="$("${kubectl_bin}" get namespace "${workspace_namespace}" \
    -o 'jsonpath={.metadata.labels.zerondesign\.dev/workspace}')"
  if [[ "${workspace_label}" != "true" ]]; then
    printf '%s is not a registered Workspace Namespace\n' \
      "${workspace_namespace}" >&2
    exit 1
  fi

  warm_replicas="$("${kubectl_bin}" get sandboxwarmpool \
    -n "${workspace_namespace}" \
    -o 'jsonpath={range .items[*]}{.spec.replicas}{"\n"}{end}' | sort -u)"
  if [[ "${warm_replicas}" != "0" ]]; then
    printf '%s has non-zero Sandbox warm pool replicas: %s\n' \
      "${workspace_namespace}" "${warm_replicas}" >&2
    exit 1
  fi
done

for workspace_namespace in "${source_workspace}" "${other_workspace}"; do
  if [[ "$("${kubectl_bin}" auth can-i \
    --as="system:serviceaccount:${runtime_namespace}:anydesign-runtime" \
    create sandboxclaims.extensions.agents.x-k8s.io \
    -n "${workspace_namespace}")" != "yes" ]]; then
    printf 'Runtime cannot create SandboxClaims in %s\n' \
      "${workspace_namespace}" >&2
    exit 1
  fi
done

if [[ "$("${kubectl_bin}" auth can-i \
  --as="system:serviceaccount:${source_workspace}:anydesign-sandbox" \
  create pods -n "${other_workspace}")" != "no" ]]; then
  printf 'Sandbox identity from %s can create Pods in %s\n' \
    "${source_workspace}" "${other_workspace}" >&2
  exit 1
fi

for workspace_namespace in "${source_workspace}" "${other_workspace}"; do
  if "${kubectl_bin}" get sandboxclaim "${smoke_name}" \
    -n "${workspace_namespace}" >/dev/null 2>&1; then
    printf 'refusing to overwrite existing SandboxClaim %s/%s\n' \
      "${workspace_namespace}" "${smoke_name}" >&2
    exit 65
  fi
done

"${kubectl_bin}" apply -f - <<EOF
apiVersion: extensions.agents.x-k8s.io/v1beta1
kind: SandboxClaim
metadata:
  name: ${smoke_name}
  namespace: ${source_workspace}
  labels:
    anydesign.dev/isolation-smoke: "true"
spec:
  warmPoolRef:
    name: anydesign-astro-website-pool
  lifecycle:
    ttlSecondsAfterFinished: 300
EOF

if ! "${kubectl_bin}" wait --for=condition=Ready \
  "sandboxclaim/${smoke_name}" -n "${source_workspace}" \
  --timeout=180s; then
  "${kubectl_bin}" get sandboxclaim,sandbox,pod,pvc \
    -n "${source_workspace}" -o wide >&2 || true
  exit 1
fi

"${kubectl_bin}" get sandbox "${smoke_name}" -n "${source_workspace}" \
  >/dev/null
"${kubectl_bin}" get pod "${smoke_name}" -n "${source_workspace}" \
  >/dev/null
"${kubectl_bin}" get pvc "${pvc_name}" -n "${source_workspace}" \
  >/dev/null
"${kubectl_bin}" get service "${smoke_name}" -n "${source_workspace}" \
  >/dev/null

for resource in sandboxclaim sandbox pod service; do
  if "${kubectl_bin}" get "${resource}" "${smoke_name}" \
    -n "${other_workspace}" >/dev/null 2>&1; then
    printf 'unexpected %s %s found in %s\n' \
      "${resource}" "${smoke_name}" "${other_workspace}" >&2
    exit 1
  fi
done
if "${kubectl_bin}" get pvc "${pvc_name}" -n "${other_workspace}" \
  >/dev/null 2>&1; then
  printf 'unexpected PVC %s found in %s\n' \
    "${pvc_name}" "${other_workspace}" >&2
  exit 1
fi

pod_service_account="$("${kubectl_bin}" get pod "${smoke_name}" \
  -n "${source_workspace}" -o 'jsonpath={.spec.serviceAccountName}')"
pod_token_mount="$("${kubectl_bin}" get pod "${smoke_name}" \
  -n "${source_workspace}" -o 'jsonpath={.spec.automountServiceAccountToken}')"
if [[ "${pod_service_account}" != "anydesign-sandbox" \
  || "${pod_token_mount}" != "false" ]]; then
  printf 'Sandbox Pod identity hardening is not applied\n' >&2
  exit 1
fi

"${kubectl_bin}" apply -f - <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: ${client_name}
  namespace: ${runtime_namespace}
  labels:
    anydesign.dev/isolation-smoke: "true"
spec:
  automountServiceAccountToken: false
  restartPolicy: Never
  containers:
    - name: mtls-client
      image: ${sandbox_image}
      imagePullPolicy: IfNotPresent
      command: ["node", "-e"]
      args:
        - |
          const fs = require("node:fs");
          const tls = require("node:tls");
          const expected = "URI:spiffe://anydesign.local/ns/${source_workspace}/sa/anydesign-sandbox";
          const deadline = Date.now() + 30000;
          const timer = setTimeout(() => {
            console.error("Workspace Channel TLS handshake timed out");
            process.exit(1);
          }, 35000);
          const connect = () => {
            const socket = tls.connect({
              host: "${smoke_name}.${source_workspace}.svc.cluster.local",
              port: 3001,
              ca: fs.readFileSync("/tls/ca.crt"),
              cert: fs.readFileSync("/tls/tls.crt"),
              key: fs.readFileSync("/tls/tls.key"),
              rejectUnauthorized: true,
              checkServerIdentity: (_host, cert) => {
                if (!cert.subjectaltname?.split(", ").includes(expected)) {
                  return new Error("unexpected Sandbox identity: " + cert.subjectaltname);
                }
              },
            });
            socket.once("secureConnect", () => {
              clearTimeout(timer);
              console.log("mTLS verified " + expected);
              socket.end();
            });
            socket.once("error", (error) => {
              socket.destroy();
              if (Date.now() < deadline && ["ECONNREFUSED", "ECONNRESET", "EAI_AGAIN"].includes(error.code)) {
                setTimeout(connect, 500);
                return;
              }
              clearTimeout(timer);
              console.error(error);
              process.exit(1);
            });
          };
          connect();
      volumeMounts:
        - name: runtime-channel-client
          mountPath: /tls
          readOnly: true
  volumes:
    - name: runtime-channel-client
      secret:
        secretName: anydesign-runtime-channel-client
EOF

if ! "${kubectl_bin}" wait --for=jsonpath='{.status.phase}'=Succeeded \
  "pod/${client_name}" -n "${runtime_namespace}" --timeout=60s; then
  "${kubectl_bin}" logs "${client_name}" -n "${runtime_namespace}" >&2 || true
  "${kubectl_bin}" describe pod "${client_name}" \
    -n "${runtime_namespace}" >&2 || true
  exit 1
fi
"${kubectl_bin}" logs "${client_name}" -n "${runtime_namespace}"

if [[ -n "${evidence_path}" ]]; then
  mkdir -p "$(dirname "${evidence_path}")"
  runtime_cert_serial_hash="$("${kubectl_bin}" get secret \
    anydesign-runtime-channel-client -n "${runtime_namespace}" \
    -o 'jsonpath={.data.tls\.crt}' | base64 -d \
    | openssl x509 -noout -serial | shasum -a 256 | awk '{print $1}')"
  sandbox_cert_serial_hash="$("${kubectl_bin}" get secret \
    anydesign-sandbox-channel-server -n "${source_workspace}" \
    -o 'jsonpath={.data.tls\.crt}' | base64 -d \
    | openssl x509 -noout -serial | shasum -a 256 | awk '{print $1}')"
  runtime_cert_expires_at="$("${kubectl_bin}" get secret \
    anydesign-runtime-channel-client -n "${runtime_namespace}" \
    -o 'jsonpath={.data.tls\.crt}' | base64 -d \
    | openssl x509 -noout -enddate | cut -d= -f2- \
    | node -e 'const fs=require("node:fs");process.stdout.write(new Date(fs.readFileSync(0,"utf8").trim()).toISOString())')"
  sandbox_cert_expires_at="$("${kubectl_bin}" get secret \
    anydesign-sandbox-channel-server -n "${source_workspace}" \
    -o 'jsonpath={.data.tls\.crt}' | base64 -d \
    | openssl x509 -noout -enddate | cut -d= -f2- \
    | node -e 'const fs=require("node:fs");process.stdout.write(new Date(fs.readFileSync(0,"utf8").trim()).toISOString())')"
  node - "${evidence_path}" "${source_workspace}" "${other_workspace}" \
    "$(printf '%s' "spiffe://anydesign.local/ns/${runtime_namespace}/sa/anydesign-runtime" | shasum -a 256 | awk '{print $1}')" \
    "$(printf '%s' "spiffe://anydesign.local/ns/${source_workspace}/sa/anydesign-sandbox" | shasum -a 256 | awk '{print $1}')" \
    "${runtime_cert_serial_hash}" "${sandbox_cert_serial_hash}" \
    "${runtime_cert_expires_at}" "${sandbox_cert_expires_at}" <<'NODE'
const fs = require("node:fs");
const [output, source, other, runtimeSanHash, sandboxSanHash,
  runtimeSerial, sandboxSerial, runtimeExpiry, sandboxExpiry] = process.argv.slice(2);
fs.writeFileSync(output, `${JSON.stringify({
  schemaVersion: "workspace-isolation-evidence@1",
  recordedAt: new Date().toISOString(),
  workspaces: { source, other },
  checks: {
    namespaceLabels: true,
    zeroWarmReplicas: true,
    runtimeRbac: true,
    sandboxCrossNamespaceDenied: true,
    coldClaimReady: true,
    resourcesConfinedToSource: true,
    serviceAccountTokenDisabled: true,
    authenticatedWorkspaceChannel: true,
  },
  transport: {
    mode: "mtls",
    mtlsVerified: true,
    rotationWindowVerified: false,
    runtimeSanHash,
    sandboxSanHash,
    runtimeCertSerialHash: runtimeSerial,
    sandboxCertSerialHash: sandboxSerial,
    runtimeCertExpiresAt: runtimeExpiry,
    sandboxCertExpiresAt: sandboxExpiry,
  },
}, null, 2)}\n`);
NODE
fi

printf 'Workspace isolation verified: %s -> %s remains namespace-scoped.\n' \
  "${source_workspace}" "${other_workspace}"
