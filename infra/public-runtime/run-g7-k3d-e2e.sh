#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
K3D="${K3D:-k3d}"
KUBECTL="${KUBECTL:-kubectl}"
CLUSTER="${ANYDESIGN_G7_CLUSTER:-zerondesign-g7}"
REGISTRY_NAME="g6-registry.localhost"
REGISTRY_CONTAINER="k3d-${REGISTRY_NAME}"
REGISTRY_HOST_PORT="${ANYDESIGN_G7_REGISTRY_PORT:-5002}"
REGISTRY_INTERNAL="${REGISTRY_CONTAINER}:5000"
HTTPS_PORT="${ANYDESIGN_G7_HTTPS_PORT:-8443}"
BASE_DOMAIN="g7.test"
REPOSITORY="${REGISTRY_INTERNAL}/anydesign/works"

for command in docker "$K3D" "$KUBECTL" node cargo openssl curl; do
  command -v "$command" >/dev/null || { printf 'missing required command: %s\n' "$command" >&2; exit 2; }
done
docker info >/dev/null

import_k3s_image() {
  local image="$1" context
  context="$(mktemp -d)"
  printf 'FROM %s\n' "$image" >"$context/Dockerfile"
  docker build --platform linux/arm64 --provenance=false -t "$image" "$context" >/dev/null
  rm -rf "$context"
  "$K3D" image import --cluster "$CLUSTER" "$image" >/dev/null
}

if ! "$K3D" registry list --no-headers 2>/dev/null | awk '{print $1}' | grep -Fxq "$REGISTRY_CONTAINER"; then
  "$K3D" registry create "$REGISTRY_NAME" --port "$REGISTRY_HOST_PORT"
fi
if ! "$K3D" cluster list --no-headers 2>/dev/null | awk '{print $1}' | grep -Fxq "$CLUSTER"; then
  k3s_image="$(node -e 'const l=require(process.argv[1]);const i=l.images.k3s;process.stdout.write(`${i.ref}@${i.digest}`)' "$ROOT/infra/agent-sandbox/images.lock.json")"
  "$K3D" cluster create "$CLUSTER" \
    --image "$k3s_image" --servers 1 --agents 0 --wait \
    --registry-use "$REGISTRY_INTERNAL" \
    --port "127.0.0.1:${HTTPS_PORT}:443@loadbalancer" \
    --k3s-arg '--disable=metrics-server@server:*'
fi
"$KUBECTL" config use-context "k3d-${CLUSTER}" >/dev/null
[[ "$($KUBECTL config current-context)" == "k3d-${CLUSTER}" ]]
if ! "$KUBECTL" get pod -n kube-system -l k8s-app=kube-dns \
  -o jsonpath='{.items[0].status.containerStatuses[0].ready}' 2>/dev/null | grep -Fxq true; then
  import_k3s_image rancher/mirrored-coredns-coredns:1.12.0
  "$KUBECTL" delete pod -n kube-system -l k8s-app=kube-dns --ignore-not-found=true >/dev/null
fi
if "$KUBECTL" get pod -n kube-system -l job-name=helm-install-traefik \
  -o jsonpath='{.items[0].status.containerStatuses[0].state.waiting.reason}' 2>/dev/null | grep -Eq 'ImagePull|BackOff|ContainerCreating'; then
  import_k3s_image rancher/klipper-helm:v0.9.3-build20241008
  "$KUBECTL" delete pod -n kube-system -l job-name=helm-install-traefik --ignore-not-found=true >/dev/null
  "$KUBECTL" delete pod -n kube-system -l job-name=helm-install-traefik-crd --ignore-not-found=true >/dev/null
fi
traefik_found=false
for _ in $(seq 1 120); do
  if "$KUBECTL" get deployment/traefik -n kube-system >/dev/null 2>&1; then
    traefik_found=true
    break
  fi
  sleep 1
done
[[ "$traefik_found" == "true" ]] || { printf 'Traefik deployment did not appear\n' >&2; exit 3; }
"$KUBECTL" rollout status deployment/traefik -n kube-system --timeout=300s >/dev/null

release_id="$(node <<'NODE'
const {createHash}=require('node:crypto');
const fields=['e'.repeat(64),'f'.repeat(64),`sha256:${'c'.repeat(64)}`,'g7-fixture@1','g7-scan@1'];
const chunks=[];
for(const field of fields){const size=Buffer.alloc(8);size.writeBigUInt64BE(BigInt(Buffer.byteLength(field)));chunks.push(size,Buffer.from(field));}
process.stdout.write(`release-${createHash('sha256').update(Buffer.concat(chunks)).digest('hex').slice(0,32)}`);
NODE
)"

tmp="$(mktemp -d)"
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT

context="$tmp/release"
mkdir -p "$context/public" "$context/metadata"
printf '<!doctype html><title>G7 Published</title><h1>stable ingress work</h1>\n' >"$context/public/index.html"
printf '{"schemaVersion":"release-provenance@1","releaseId":"%s"}\n' "$release_id" >"$context/metadata/release-provenance.json"
cp "$ROOT/infra/published-runtime/static-web/Dockerfile" "$context/Dockerfile"
cp "$ROOT/infra/published-runtime/static-web/nginx.conf" "$context/nginx.conf"
release_host_ref="localhost:${REGISTRY_HOST_PORT}/anydesign/works/${release_id}:latest"
docker build --platform linux/arm64 --provenance=false \
  --build-arg "RELEASE_ID=${release_id}" -t "$release_host_ref" "$context" >/dev/null
docker push "$release_host_ref" >/dev/null
release_digest="$(docker inspect --format '{{index .RepoDigests 0}}' "$release_host_ref" | sed 's/^.*@//')"
[[ "$release_digest" =~ ^sha256:[a-f0-9]{64}$ ]]

prober_context="$tmp/prober"
mkdir -p "$prober_context"
base_image="$(node -e 'const l=require(process.argv[1]);const i=l.images.staticWebBase;process.stdout.write(`${i.source}@${i.digest}`)' "$ROOT/infra/published-runtime/images.lock.json")"
printf 'FROM %s\nUSER 101:101\n' "$base_image" >"$prober_context/Dockerfile"
prober_host_ref="localhost:${REGISTRY_HOST_PORT}/anydesign/release-prober:g7"
docker build --platform linux/arm64 --provenance=false -t "$prober_host_ref" "$prober_context" >/dev/null
docker push "$prober_host_ref" >/dev/null
prober_digest="$(docker inspect --format '{{index .RepoDigests 0}}' "$prober_host_ref" | sed 's/^.*@//')"
[[ "$prober_digest" =~ ^sha256:[a-f0-9]{64}$ ]]

"$KUBECTL" apply -f "$ROOT/infra/public-runtime/base.yaml" >/dev/null
"$KUBECTL" delete deployment,service,ingress,networkpolicy -n anydesign-works \
  -l app.kubernetes.io/managed-by=anydesign-work-runtime-controller \
  --ignore-not-found=true --wait=true >/dev/null

ca_key="$tmp/ca.key"
ca_cert="$tmp/ca.crt"
tls_key="$tmp/tls.key"
tls_csr="$tmp/tls.csr"
tls_cert="$tmp/tls.crt"
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj '/CN=AnyDesign G7 Test CA' \
  -keyout "$ca_key" -out "$ca_cert" >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes -subj "/CN=*.${BASE_DOMAIN}" \
  -keyout "$tls_key" -out "$tls_csr" >/dev/null 2>&1
printf 'subjectAltName=DNS:*.%s\nextendedKeyUsage=serverAuth\n' "$BASE_DOMAIN" >"$tmp/tls.ext"
openssl x509 -req -sha256 -days 2 -in "$tls_csr" -CA "$ca_cert" -CAkey "$ca_key" \
  -CAcreateserial -extfile "$tmp/tls.ext" -out "$tls_cert" >/dev/null 2>&1
"$KUBECTL" create secret tls anydesign-works-wildcard-tls -n anydesign-works \
  --cert="$tls_cert" --key="$tls_key" --dry-run=client -o yaml | "$KUBECTL" apply -f - >/dev/null

RUN_WORK_RUNTIME_G7_K8S_E2E=1 \
G7_IMAGE_REPOSITORY="$REPOSITORY" \
G7_IMAGE_DIGEST="$release_digest" \
WORK_RUNTIME_PROBER_IMAGE="${REGISTRY_INTERNAL}/anydesign/release-prober@${prober_digest}" \
WORK_RUNTIME_EXPOSURE=ingress \
WORKS_BASE_DOMAIN="$BASE_DOMAIN" \
WORKS_INGRESS_CLASS=traefik \
WORKS_TLS_SECRET_NAME=anydesign-works-wildcard-tls \
WORKS_PROBE_SCHEME=https \
WORKS_PROBE_RESOLVE="127.0.0.1:${HTTPS_PORT}" \
WORKS_PROBE_CA_FILE="$ca_cert" \
cargo test --manifest-path "$ROOT/services/runtime/Cargo.toml" \
  --test k8s_work_runtime_g7 -- --nocapture

host="$($KUBECTL get ingress -n anydesign-works -l app.kubernetes.io/managed-by=anydesign-work-runtime-controller -o jsonpath='{.items[0].spec.rules[0].host}')"
[[ -n "$host" ]]
curl --fail --silent --show-error --cacert "$ca_cert" \
  --resolve "${host}:${HTTPS_PORT}:127.0.0.1" \
  "https://${host}:${HTTPS_PORT}/.well-known/anydesign/release" | grep -F "$release_id" >/dev/null
printf 'G7 k3d gate passed: cluster=%s host=%s release=%s digest=%s\n' \
  "$CLUSTER" "$host" "$release_id" "$release_digest"
