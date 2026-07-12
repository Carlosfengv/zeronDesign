#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
HTTP_DIR="$ROOT/services/runtime/src/http_api"
FACADE="$HTTP_DIR/mod.rs"
HTTP_TEST_ROOT="$ROOT/services/runtime/tests/http_api.rs"
HTTP_TEST_DIR="$ROOT/services/runtime/tests/http_api"
RUN_LIFECYCLE_DIR="$ROOT/services/runtime/src/run_lifecycle"
DESIGN_PROFILE_DIR="$ROOT/services/runtime/src/design_profile"
DESIGN_PROFILE_SERVICE_DIR="$ROOT/services/runtime/src/design_profile_service"
ARTIFACT_ACCESS="$ROOT/services/runtime/src/artifact_access.rs"
AUTHORIZATION_POLICY="$ROOT/services/runtime/src/authorization.rs"
PREVIEW_ACCESS="$ROOT/services/runtime/src/preview_access.rs"
RELEASE_EVIDENCE_SERVICE="$ROOT/services/runtime/src/release_evidence.rs"
status=0

fail() {
  printf '%s\n' "$1" >&2
  status=1
}

if [[ -f "$ROOT/services/runtime/src/http_api.rs" ]]; then
  fail "HTTP-001: legacy services/runtime/src/http_api.rs must not be restored"
fi

if [[ ! -f "$FACADE" ]]; then
  fail "HTTP-001: services/runtime/src/http_api/mod.rs is missing"
else
  facade_lines="$(wc -l < "$FACADE" | tr -d ' ')"
  if (( facade_lines > 300 )); then
    fail "HTTP-001/SIZE-001: HTTP facade exceeds 300 lines: $facade_lines"
  fi
  if grep -nE '\.route\(' "$FACADE"; then
    fail "HTTP-002: HTTP facade must compose sub-routers instead of registering routes directly"
  fi
fi

while IFS= read -r module; do
  lines="$(wc -l < "$module" | tr -d ' ')"
  if (( lines > 800 )); then
    fail "SIZE-001: HTTP production module exceeds 800 lines: ${module#$ROOT/} ($lines)"
  fi
done < <(find "$HTTP_DIR" -type f -name '*.rs' ! -path "$FACADE" | sort)

if grep -RInE --include='*.rs' 'axum|RuntimeStore|WorkspaceBackend|StatusCode|Router' "$HTTP_DIR/contracts"; then
  fail "HTTP-003: HTTP contracts depend on transport, store, or workspace implementation types"
fi

if grep -RInE --include='*.rs' '(^|[^[:alnum:]_])axum([^[:alnum:]_]|$)' "$RUN_LIFECYCLE_DIR"; then
  fail "APP-002: RunLifecycle application service must not depend on Axum"
fi

if grep -RInE --include='*.rs' 'std::fs|tokio::fs' "$RUN_LIFECYCLE_DIR"; then
  fail "APP-003: RunLifecycle application service must use filesystem adapters"
fi

while IFS= read -r application_module; do
  lines="$(wc -l < "$application_module" | tr -d ' ')"
  if (( lines > 800 )); then
    fail "APP-004/SIZE-001: RunLifecycle application module exceeds 800 lines: ${application_module#$ROOT/} ($lines)"
  fi
done < <(find "$RUN_LIFECYCLE_DIR" -type f -name '*.rs' | sort)

for mutation_route in cancel continue_run permission start; do
  route_module="$HTTP_DIR/routes/runs/$mutation_route.rs"
  lines="$(wc -l < "$route_module" | tr -d ' ')"
  if (( lines > 50 )); then
    fail "HTTP-009: Run mutation route adapter exceeds 50 lines: ${route_module#$ROOT/} ($lines)"
  fi
done

if grep -RInE --include='*.rs' 'axum|RuntimeStore|HeaderMap|StatusCode|Router' "$DESIGN_PROFILE_DIR"; then
  fail "PROFILE-001: pure DesignProfile parser/validation modules depend on transport or RuntimeStore"
fi

while IFS= read -r profile_module; do
  lines="$(wc -l < "$profile_module" | tr -d ' ')"
  if (( lines > 800 )); then
    fail "PROFILE-002/SIZE-001: DesignProfile module exceeds 800 lines: ${profile_module#$ROOT/} ($lines)"
  fi
done < <(find "$DESIGN_PROFILE_DIR" -type f -name '*.rs' | sort)

if grep -RInE --include='*.rs' 'axum|HeaderMap|StatusCode|Router|std::fs|tokio::fs' "$DESIGN_PROFILE_SERVICE_DIR"; then
  fail "PROFILE-003: DesignProfile application service depends on HTTP or direct filesystem APIs"
fi

while IFS= read -r profile_service_module; do
  lines="$(wc -l < "$profile_service_module" | tr -d ' ')"
  if (( lines > 800 )); then
    fail "PROFILE-004/SIZE-001: DesignProfile service module exceeds 800 lines: ${profile_service_module#$ROOT/} ($lines)"
  fi
done < <(find "$DESIGN_PROFILE_SERVICE_DIR" -type f -name '*.rs' | sort)

if grep -nE 'state\.store|\.store\.' "$HTTP_DIR/routes/design_profiles.rs"; then
  fail "PROFILE-005: DesignProfile HTTP routes must not orchestrate RuntimeStore directly"
fi

if grep -RInE --include='*.rs' 'std::fs|tokio::fs|(^|[^[:alnum:]_])fs::' "$HTTP_DIR/routes"; then
  fail "STORAGE-001: HTTP routes must use Runtime-owned filesystem ports"
fi

if grep -nE 'std::fs|tokio::fs|(^|[^[:alnum:]_])fs::|axum|HeaderMap|StatusCode|Router' "$ARTIFACT_ACCESS"; then
  fail "STORAGE-002: ArtifactAccess application service depends on filesystem or HTTP APIs"
fi

if grep -nE 'state\.store|\.store\.|ArtifactResolver|FileArtifactPublisher|std::fs|tokio::fs' \
  "$HTTP_DIR/routes/artifacts.rs"; then
  fail "STORAGE-003: Artifact HTTP route bypasses ArtifactAccessService or presenter"
fi

if grep -nE 'axum|HeaderMap|AUTHORIZATION|Bearer|std::fs|tokio::fs' \
  "$AUTHORIZATION_POLICY" "$PREVIEW_ACCESS"; then
  fail "AUTH-001: application authorization and PreviewAccess must not depend on HTTP, raw credentials, or filesystem APIs"
fi

if grep -nE 'get_preview_lease|get_sandbox_binding|ChannelManager|PreviewLeaseStatus' \
  "$HTTP_DIR/routes/previews.rs"; then
  fail "AUTH-002: Preview HTTP routes must delegate lease and sandbox authorization to PreviewAccessService"
fi

if grep -nE 'get_project_access|owner_principal_id|principal\.project_id' \
  "$HTTP_DIR/auth/publication.rs" "$HTTP_DIR/auth/candidate_preview.rs"; then
  fail "AUTH-003: HTTP auth adapters must delegate project ownership to the application authorization policy"
fi

if grep -nE '^async fn|state\.store|\.store\.' "$HTTP_DIR/routes/internal.rs" || \
  grep -nE 'state\.store|\.store\.|RuntimeEvidenceStore' \
    "$HTTP_DIR/routes/internal/release_evidence.rs"; then
  fail "INTERNAL-001: Internal facade and Release Evidence route must delegate to use-case modules and service"
fi

for internal_use_case in template_build preview_promotion project_access release_evidence sandbox_release; do
  if [[ ! -f "$HTTP_DIR/routes/internal/$internal_use_case.rs" ]]; then
    fail "INTERNAL-002: Internal use case route module is missing: $internal_use_case"
  fi
done

if grep -nE 'axum|HeaderMap|StatusCode|Router|std::fs|tokio::fs' "$RELEASE_EVIDENCE_SERVICE"; then
  fail "INTERNAL-003: ReleaseEvidence application service depends on HTTP or direct filesystem APIs"
fi

if grep -RInEi --include='*.rs' --exclude='artifacts.rs' \
  'astro-website|fumadocs-docs|docusaurus|template[[:space:]]*==|match[[:space:]]+template' \
  "$HTTP_DIR/routes"; then
  fail "HTTP-004: route handlers contain concrete template or framework dispatch"
fi

for family in artifacts capture design_profiles design_sources internal previews projects run_events runs system; do
  if [[ ! -f "$ROOT/services/runtime/tests/http_api/routes/$family.rs" ]]; then
    fail "HTTP-005: route family is missing an independently discovered test module: $family"
  fi
done

if [[ ! -f "$HTTP_TEST_ROOT" ]] || (( $(wc -l < "$HTTP_TEST_ROOT") > 100 )); then
  fail "HTTP-006: HTTP integration crate root must exist and remain below 100 lines"
fi
if grep -nE '^#\[(tokio::test|test)' "$HTTP_TEST_ROOT" >/dev/null; then
  fail "HTTP-006: HTTP integration crate root must not contain test bodies"
fi
while IFS= read -r test_module; do
  lines="$(wc -l < "$test_module" | tr -d ' ')"
  if (( lines > 800 )); then
    fail "HTTP-007: HTTP test module exceeds 800 lines: ${test_module#$ROOT/} ($lines)"
  fi
done < <(find "$HTTP_TEST_DIR" -type f -name '*.rs' | sort)

test_count="$(grep -RhcE --include='*.rs' '^#\[(tokio::test|test)' "$HTTP_TEST_ROOT" "$HTTP_TEST_DIR" | awk '{ total += $1 } END { print total + 0 }')"
if (( test_count < 86 )); then
  fail "HTTP-008: Cargo-discovered HTTP test inventory fell below the frozen 86-test baseline: $test_count"
fi

if [[ "$status" -ne 0 ]]; then
  exit "$status"
fi

echo "HTTP API architecture check passed"
