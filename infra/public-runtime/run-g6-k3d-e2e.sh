#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
K3D="${K3D:-k3d}"
KUBECTL="${KUBECTL:-kubectl}"
CLUSTER="${ANYDESIGN_G6_CLUSTER:-zerondesign-g6}"
REGISTRY_NAME="g6-registry.localhost"
REGISTRY_CONTAINER="k3d-${REGISTRY_NAME}"
REGISTRY_HOST_PORT="${ANYDESIGN_G6_REGISTRY_PORT:-5002}"
REGISTRY_INTERNAL="${REGISTRY_CONTAINER}:5000"
REPOSITORY="${REGISTRY_INTERNAL}/anydesign/works"

for command in docker "$K3D" "$KUBECTL" node cargo; do
  command -v "$command" >/dev/null || { printf 'missing required command: %s\n' "$command" >&2; exit 2; }
done
docker info >/dev/null

if ! "$K3D" registry list --no-headers 2>/dev/null | awk '{print $1}' | grep -Fxq "$REGISTRY_CONTAINER"; then
  "$K3D" registry create "$REGISTRY_NAME" --port "$REGISTRY_HOST_PORT"
fi
if ! "$K3D" cluster list --no-headers 2>/dev/null | awk '{print $1}' | grep -Fxq "$CLUSTER"; then
  "$K3D" cluster create "$CLUSTER" \
    --servers 1 --agents 0 --no-lb --wait \
    --registry-use "${REGISTRY_INTERNAL}" \
    --k3s-arg '--disable=traefik@server:*' \
    --k3s-arg '--disable=metrics-server@server:*'
fi
"$KUBECTL" config use-context "k3d-${CLUSTER}" >/dev/null
[[ "$($KUBECTL config current-context)" == "k3d-${CLUSTER}" ]]

release_id() {
  node - "$1" <<'NODE'
const {createHash}=require('node:crypto');
const marker=process.argv[2];
const next=String.fromCharCode(marker.charCodeAt(0)+1);
const fields=[marker.repeat(64),next.repeat(64),`sha256:${'c'.repeat(64)}`,'g6-fixture@1','g6-scan@1'];
const chunks=[];
for(const field of fields){const size=Buffer.alloc(8);size.writeBigUInt64BE(BigInt(Buffer.byteLength(field)));chunks.push(size,Buffer.from(field));}
process.stdout.write(`release-${createHash('sha256').update(Buffer.concat(chunks)).digest('hex').slice(0,32)}`);
NODE
}

tmp="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp"
}
trap cleanup EXIT

build_release() {
  local marker="$1"
  local release="$2"
  local context="$tmp/$marker"
  local host_ref digest
  mkdir -p "$context/public" "$context/metadata"
  printf '<!doctype html><title>G6 %s</title><h1>isolated work %s</h1>\n' "$marker" "$marker" >"$context/public/index.html"
  printf '{"schemaVersion":"release-provenance@1","releaseId":"%s"}\n' "$release" >"$context/metadata/release-provenance.json"
  cp "$ROOT/infra/published-runtime/static-web/Dockerfile" "$context/Dockerfile"
  cp "$ROOT/infra/published-runtime/static-web/nginx.conf" "$context/nginx.conf"
  host_ref="localhost:${REGISTRY_HOST_PORT}/anydesign/works/${release}:latest"
  docker build --platform linux/arm64 --provenance=false -t "$host_ref" "$context" >/dev/null
  docker push "$host_ref" >/dev/null
  digest="$(docker inspect --format '{{index .RepoDigests 0}}' "$host_ref" | sed 's/^.*@//')"
  [[ "$digest" =~ ^sha256:[a-f0-9]{64}$ ]] || { printf 'invalid image digest: %s\n' "$digest" >&2; exit 3; }
  printf '%s' "$digest"
}

build_prober() {
  local context="$tmp/prober"
  local host_ref="localhost:${REGISTRY_HOST_PORT}/anydesign/release-prober:g6"
  local base digest
  base="$(node -e 'const l=require(process.argv[1]);process.stdout.write(`${l.images.staticWebBase.source}@${l.images.staticWebBase.digest}`)' "$ROOT/infra/published-runtime/images.lock.json")"
  mkdir -p "$context"
  printf 'FROM %s\nUSER 101:101\n' "$base" >"$context/Dockerfile"
  docker build --platform linux/arm64 --provenance=false -t "$host_ref" "$context" >/dev/null
  docker push "$host_ref" >/dev/null
  digest="$(docker inspect --format '{{index .RepoDigests 0}}' "$host_ref" | sed 's/^.*@//')"
  [[ "$digest" =~ ^sha256:[a-f0-9]{64}$ ]] || { printf 'invalid prober image digest: %s\n' "$digest" >&2; exit 3; }
  printf '%s' "$digest"
}

release_a="$(release_id a)"
release_b="$(release_id b)"
digest_a="$(build_release a "$release_a")"
digest_b="$(build_release b "$release_b")"
prober_digest="$(build_prober)"

"$KUBECTL" apply -f "$ROOT/infra/public-runtime/base.yaml" >/dev/null
"$KUBECTL" wait --for=jsonpath='{.status.phase}'=Active namespace/anydesign-works --timeout=60s >/dev/null
"$KUBECTL" delete deployment,service,networkpolicy -n anydesign-works \
  -l app.kubernetes.io/managed-by=anydesign-work-runtime-controller \
  --ignore-not-found=true --wait=true >/dev/null

RUN_WORK_RUNTIME_G6_K8S_E2E=1 \
G6_IMAGE_REPOSITORY="$REPOSITORY" \
G6_IMAGE_DIGEST_A="$digest_a" \
G6_IMAGE_DIGEST_B="$digest_b" \
WORK_RUNTIME_PROBER_IMAGE="${REGISTRY_INTERNAL}/anydesign/release-prober@${prober_digest}" \
cargo test --manifest-path "$ROOT/services/runtime/Cargo.toml" \
  --test k8s_work_runtime_g6 -- --nocapture

if "$KUBECTL" get ingress -n anydesign-works -o name | grep -q .; then
  printf 'G6 must not create Ingress resources\n' >&2
  exit 4
fi
printf 'G6 k3d gate passed: cluster=%s releaseA=%s digestA=%s releaseB=%s digestB=%s\n' \
  "$CLUSTER" "$release_a" "$digest_a" "$release_b" "$digest_b"
