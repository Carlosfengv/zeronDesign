#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
HTTP_DIR="$ROOT/services/runtime/src/http_api"
FACADE="$HTTP_DIR/mod.rs"
HTTP_TEST_ROOT="$ROOT/services/runtime/tests/http_api.rs"
HTTP_TEST_DIR="$ROOT/services/runtime/tests/http_api"
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
  if rg -n '\.route\(' "$FACADE"; then
    fail "HTTP-002: HTTP facade must compose sub-routers instead of registering routes directly"
  fi
fi

while IFS= read -r module; do
  lines="$(wc -l < "$module" | tr -d ' ')"
  if (( lines > 800 )); then
    fail "SIZE-001: HTTP production module exceeds 800 lines: ${module#$ROOT/} ($lines)"
  fi
done < <(find "$HTTP_DIR" -type f -name '*.rs' ! -path "$FACADE" | sort)

if rg -n 'axum|RuntimeStore|WorkspaceBackend|StatusCode|Router' "$HTTP_DIR/contracts" --glob '*.rs'; then
  fail "HTTP-003: HTTP contracts depend on transport, store, or workspace implementation types"
fi

if rg -n -i 'astro-website|fumadocs-docs|docusaurus|template\s*==|match\s+template' \
  "$HTTP_DIR/routes" --glob '*.rs' --glob '!artifacts.rs'; then
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
if rg -n '^#\[(tokio::test|test)' "$HTTP_TEST_ROOT" >/dev/null; then
  fail "HTTP-006: HTTP integration crate root must not contain test bodies"
fi
while IFS= read -r test_module; do
  lines="$(wc -l < "$test_module" | tr -d ' ')"
  if (( lines > 800 )); then
    fail "HTTP-007: HTTP test module exceeds 800 lines: ${test_module#$ROOT/} ($lines)"
  fi
done < <(find "$HTTP_TEST_DIR" -type f -name '*.rs' | sort)

test_count="$(rg -n '^#\[(tokio::test|test)' "$HTTP_TEST_ROOT" "$HTTP_TEST_DIR" -g '*.rs' | wc -l | tr -d ' ')"
if (( test_count < 86 )); then
  fail "HTTP-008: Cargo-discovered HTTP test inventory fell below the frozen 86-test baseline: $test_count"
fi

if [[ "$status" -ne 0 ]]; then
  exit "$status"
fi

echo "HTTP API architecture check passed"
