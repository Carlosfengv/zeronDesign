#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

cd "${ROOT_DIR}"

run_step() {
  local label="$1"
  shift
  printf '\n==> %s\n' "${label}"
  "$@"
}

run_step "Rust runtime formatting" \
  cargo fmt --manifest-path services/runtime/Cargo.toml -- --check

run_step "Rust runtime tests" \
  cargo test --manifest-path services/runtime/Cargo.toml

run_step "Shared package tests" \
  npm test --prefix packages/shared

run_step "Shared package typecheck" \
  npm run typecheck --prefix packages/shared

if [[ -d apps/web ]]; then
  run_step "Phase B web typecheck" \
    npm run typecheck --prefix apps/web

  run_step "Phase B web production build" \
    npm run build --prefix apps/web
fi

if [[ "${ANYDESIGN_SKIP_K8S_E2E:-0}" == "1" ]]; then
  printf '\n==> K8s sandbox E2E skipped because ANYDESIGN_SKIP_K8S_E2E=1\n'
  exit 0
fi

run_step "K8s sandbox E2E" \
  bash infra/agent-sandbox/run-k8s-e2e.sh
