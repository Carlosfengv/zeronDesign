#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
MAIN="$ROOT/services/runtime/src/main.rs"
HTTP_DIR="$ROOT/services/runtime/src/http_api"
RUNTIME_DIR="$ROOT/services/runtime/src/runtime"
status=0

fail() {
  printf '%s\n' "$1" >&2
  status=1
}

if ! grep -Eq 'RuntimeBootstrap::new\(config\)\.run\(\)' "$MAIN"; then
  fail "BOOT-001: production main must start exclusively through RuntimeBootstrap::run"
fi

if grep -En 'http_api|router_with_state|recover_startup_runs|tokio::spawn|axum::serve' "$MAIN"; then
  fail "BOOT-001: production main bypasses RuntimeBootstrap or RuntimeSupervisor"
fi

if [[ -f "$HTTP_DIR/startup.rs" ]]; then
  fail "BOOT-002: startup recovery must be owned by runtime/bootstrap.rs"
fi

if ! grep -Eq 'pub supervisor: RuntimeSupervisor' "$HTTP_DIR/mod.rs"; then
  fail "BOOT-003: AppState must carry the Bootstrap-owned RuntimeSupervisor"
fi

if grep -RIn --include='*.rs' 'tokio::spawn' "$HTTP_DIR"; then
  fail "BOOT-004: HTTP handlers must register background tasks through RuntimeSupervisor"
fi

for required in bootstrap.rs supervisor.rs; do
  if [[ ! -f "$RUNTIME_DIR/$required" ]]; then
    fail "BOOT-005: missing runtime lifecycle module: $required"
  fi
done

if [[ "$status" -ne 0 ]]; then
  exit "$status"
fi

echo "Runtime bootstrap architecture check passed"
