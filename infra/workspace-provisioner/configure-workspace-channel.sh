#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  printf 'usage: %s ws-<stable-id> [ws-<stable-id> ...]\n' "$0" >&2
  exit 64
fi

kubectl_bin="${KUBECTL:-kubectl}"
runtime_namespace="${RUNTIME_SYSTEM_NAMESPACE:-anydesign-runtime}"
rotate="${ROTATE_WORKSPACE_CHANNEL_KEYS:-false}"
work_dir="$(mktemp -d)"
cleanup() { rm -rf "${work_dir}"; }
trap cleanup EXIT

for workspace_namespace in "$@"; do
  if [[ ! "${workspace_namespace}" =~ ^ws-[a-z0-9]([a-z0-9-]*[a-z0-9])?$ ]] \
    || (( ${#workspace_namespace} > 63 )); then
    printf 'invalid Workspace Namespace: %s\n' "${workspace_namespace}" >&2
    exit 64
  fi
  "${kubectl_bin}" get namespace "${workspace_namespace}" >/dev/null
done
"${kubectl_bin}" get namespace "${runtime_namespace}" >/dev/null

if "${kubectl_bin}" get secret anydesign-workspace-channel-signer \
  -n "${runtime_namespace}" >/dev/null 2>&1 && [[ "${rotate}" != "true" ]]; then
  printf 'workspace channel credentials already exist; set ROTATE_WORKSPACE_CHANNEL_KEYS=true to rotate\n' >&2
  exit 65
fi

signer_private="${work_dir}/signer-private.der"
signer_public="${work_dir}/signer-public.der"
ca_key="${work_dir}/ca.key"
ca_cert="${work_dir}/ca.crt"
runtime_key="${work_dir}/runtime.key"
runtime_csr="${work_dir}/runtime.csr"
runtime_cert="${work_dir}/runtime.crt"

node -e '
const {generateKeyPairSync}=require("node:crypto");
const {writeFileSync}=require("node:fs");
const {privateKey,publicKey}=generateKeyPairSync("ed25519");
writeFileSync(process.argv[1],privateKey.export({format:"der",type:"pkcs8"}));
writeFileSync(process.argv[2],publicKey.export({format:"der",type:"spki"}));
' "${signer_private}" "${signer_public}"
openssl req -x509 -newkey rsa:2048 -sha256 -nodes -days 30 \
  -subj '/CN=zeronDesign Workspace Channel CA' \
  -keyout "${ca_key}" -out "${ca_cert}" >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes -subj '/CN=anydesign-runtime' \
  -keyout "${runtime_key}" -out "${runtime_csr}" >/dev/null 2>&1
cat >"${work_dir}/runtime.ext" <<EOF
basicConstraints=CA:FALSE
keyUsage=digitalSignature,keyEncipherment
extendedKeyUsage=clientAuth
subjectAltName=URI:spiffe://anydesign.local/ns/${runtime_namespace}/sa/anydesign-runtime
EOF
openssl x509 -req -sha256 -days 30 -in "${runtime_csr}" \
  -CA "${ca_cert}" -CAkey "${ca_key}" -CAcreateserial \
  -extfile "${work_dir}/runtime.ext" -out "${runtime_cert}" >/dev/null 2>&1

"${kubectl_bin}" create secret generic anydesign-workspace-channel-signer \
  -n "${runtime_namespace}" --from-file="private.der=${signer_private}" \
  --dry-run=client -o yaml | "${kubectl_bin}" apply -f - >/dev/null
"${kubectl_bin}" create secret generic anydesign-runtime-channel-client \
  -n "${runtime_namespace}" \
  --from-file="ca.crt=${ca_cert}" \
  --from-file="tls.crt=${runtime_cert}" \
  --from-file="tls.key=${runtime_key}" \
  --dry-run=client -o yaml | "${kubectl_bin}" apply -f - >/dev/null

for workspace_namespace in "$@"; do
  sandbox_key="${work_dir}/${workspace_namespace}.key"
  sandbox_csr="${work_dir}/${workspace_namespace}.csr"
  sandbox_cert="${work_dir}/${workspace_namespace}.crt"
  openssl req -newkey rsa:2048 -nodes -subj "/CN=anydesign-sandbox.${workspace_namespace}" \
    -keyout "${sandbox_key}" -out "${sandbox_csr}" >/dev/null 2>&1
  cat >"${work_dir}/${workspace_namespace}.ext" <<EOF
basicConstraints=CA:FALSE
keyUsage=digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=URI:spiffe://anydesign.local/ns/${workspace_namespace}/sa/anydesign-sandbox,DNS:*.${workspace_namespace}.svc.cluster.local
EOF
  openssl x509 -req -sha256 -days 30 -in "${sandbox_csr}" \
    -CA "${ca_cert}" -CAkey "${ca_key}" -CAcreateserial \
    -extfile "${work_dir}/${workspace_namespace}.ext" -out "${sandbox_cert}" >/dev/null 2>&1
  "${kubectl_bin}" create configmap anydesign-workspace-channel-verifier \
    -n "${workspace_namespace}" \
    --from-file="current.der=${signer_public}" \
    --from-file="previous.der=${signer_public}" \
    --dry-run=client -o yaml | "${kubectl_bin}" apply -f - >/dev/null
  "${kubectl_bin}" create secret generic anydesign-sandbox-channel-server \
    -n "${workspace_namespace}" \
    --from-file="ca.crt=${ca_cert}" \
    --from-file="tls.crt=${sandbox_cert}" \
    --from-file="tls.key=${sandbox_key}" \
    --dry-run=client -o yaml | "${kubectl_bin}" apply -f - >/dev/null
done

printf 'Configured mTLS and signed workspace-channel credentials for %s Workspace(s).\n' "$#"
