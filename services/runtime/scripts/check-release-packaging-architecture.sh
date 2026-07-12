#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
RELEASE_DIR="$ROOT/services/runtime/src/release"
CONTRACT_DIR="$ROOT/services/runtime/contracts"
SANDBOX_DIR="$ROOT/services/runtime/src/tools/sandbox"
PUBLISHED_RUNTIME_DIR="$ROOT/infra/published-runtime"
status=0

fail() {
  printf '%s\n' "$1" >&2
  status=1
}

for required in runtime-manifest-v1.schema.json work-release-v1.schema.json release-packaging-v1.schema.json; do
  if [[ ! -f "$CONTRACT_DIR/$required" ]]; then
    fail "REL-001: missing frozen release contract: $required"
  fi
done

for required in manifest.rs model.rs store.rs packager.rs process_backend.rs garbage_collector.rs; do
  if [[ ! -f "$RELEASE_DIR/$required" ]]; then
    fail "REL-002: missing release domain module: $required"
  fi
done

while IFS= read -r file; do
  lines="$(wc -l < "$file" | tr -d ' ')"
  if (( lines > 800 )); then
    fail "REL-003: release module exceeds 800 lines: ${file#"$ROOT/"} ($lines)"
  fi
done < <(find "$RELEASE_DIR" -type f -name '*.rs' -print)

if grep -RInE --include='*.rs' 'axum|Ingress|Deployment|StatefulSet|kube::|k8s_openapi' "$RELEASE_DIR"; then
  fail "REL-004: packaging domain must not create HTTP or Published Kubernetes resources"
fi

for required in images.lock.json buildkitd.local.toml static-web/Dockerfile static-web/nginx.conf; do
  if [[ ! -f "$PUBLISHED_RUNTIME_DIR/$required" ]]; then
    fail "REL-008: missing trusted published runtime input: $required"
  fi
done

if ! grep -Eq 'env_clear\(\)' "$RELEASE_DIR/process_backend.rs" \
  || ! grep -Eq 'verify_program\(\)' "$RELEASE_DIR/process_backend.rs"; then
  fail "REL-009: process backend must clear inherited environment and hash-check its helper"
fi

if grep -Eq '(eval\(|execSync|shell:[[:space:]]*true)' "$ROOT/services/runtime/scripts/release-packaging-backend.mjs"; then
  fail "REL-010: packaging helper must not execute shell fragments"
fi

if ! grep -Eq 'USER 101:101' "$PUBLISHED_RUNTIME_DIR/static-web/Dockerfile" \
  || ! grep -Eq 'location \^~ /\.anydesign/' "$PUBLISHED_RUNTIME_DIR/static-web/nginx.conf"; then
  fail "REL-011: published runtime must run non-root and deny internal metadata"
fi

base_digest="$(jq -r '.images.staticWebBase.digest' "$PUBLISHED_RUNTIME_DIR/images.lock.json")"
if ! grep -Fq "$base_digest" "$PUBLISHED_RUNTIME_DIR/static-web/Dockerfile"; then
  fail "REL-012: static runtime Dockerfile base digest differs from the reviewed lock"
fi

if grep -RInE --include='*.rs' 'WORK_RELEASE_REGISTRY|registry[_ -]?(password|token|credential)|docker[[:space:]]+push|cosign[[:space:]]+sign' "$SANDBOX_DIR"; then
  fail "REL-005: Agent/Sandbox code must not receive Registry or signing credentials"
fi

if ! grep -Eq 'trait TrustedReleasePackagingBackend' "$RELEASE_DIR/packager.rs"; then
  fail "REL-006: Registry, scan, and signing actions require a trusted backend boundary"
fi

if ! grep -Eq 'ArtifactResolver::load_for_version' "$RELEASE_DIR/packager.rs"; then
  fail "REL-007: packaging must revalidate immutable Artifact bytes"
fi

if [[ "$status" -ne 0 ]]; then
  exit "$status"
fi

echo "release packaging architecture check passed"
