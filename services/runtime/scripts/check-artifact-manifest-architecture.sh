#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
MANIFEST="$ROOT/services/runtime/src/artifact_manifest.rs"
SCHEMA="$ROOT/services/runtime/contracts/artifact-manifest-v1.schema.json"
PUBLISHER="$ROOT/services/runtime/src/artifact_publisher.rs"
TEMPLATE_SPEC="$ROOT/services/runtime/src/templates/spec.rs"
ARTIFACT_ROUTES="$ROOT/services/runtime/src/http_api/routes/artifacts.rs"
ARTIFACT_FILE_ADAPTER="$ROOT/services/runtime/src/runtime_storage/artifact.rs"
ARTIFACT_PRESENTER="$ROOT/services/runtime/src/http_api/artifact_presenter.rs"
HTTP_DIR="$ROOT/services/runtime/src/http_api"
status=0

fail() {
  printf '%s\n' "$1" >&2
  status=1
}

if [[ ! -f "$SCHEMA" ]] || ! grep -Eq 'ARTIFACT_MANIFEST_SCHEMA.*artifact-manifest@1' "$MANIFEST"; then
  fail "ART-001: typed artifact-manifest@1 schema is missing"
fi

if ! grep -Eq 'pub artifact_delivery: ArtifactDeliverySpec' "$TEMPLATE_SPEC"; then
  fail "ART-002: TemplateSpec must declare framework-neutral artifact delivery"
fi

if ! grep -Eq 'ArtifactResolver::load_for_version' "$ARTIFACT_FILE_ADAPTER" || \
  grep -Eq 'ArtifactResolver|FileArtifactPublisher' "$ARTIFACT_ROUTES"; then
  fail "ART-003: new artifacts must resolve from the verified manifest through the file adapter"
fi

if grep -Ein 'astro|fumadocs|nextjs' "$MANIFEST" "$PUBLISHER"; then
  fail "ART-004: generic artifact manifest/publisher code must not dispatch on frameworks"
fi

if grep -Ein '/_astro|/docs|fumadocs|astro' "$ARTIFACT_ROUTES"; then
  fail "ART-005: artifact routes must not add framework-specific paths or rewrites"
fi

rewrite_hits="$(grep -RIl --include='*.rs' 'rewrite_legacy_artifact_html' "$HTTP_DIR" || true)"
if [[ "$rewrite_hits" != "$ARTIFACT_PRESENTER" ]]; then
  fail "ART-006: historical framework rewrite must remain isolated in the Artifact HTTP presenter"
fi

if [[ "$status" -ne 0 ]]; then
  exit "$status"
fi

echo "artifact manifest architecture check passed"
