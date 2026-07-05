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

if [[ -d apps/web ]]; then
  printf 'Phase A runtime must not contain apps/web code, but apps/web exists.\n' >&2
  exit 1
fi

run_step "Rust runtime formatting" \
  cargo fmt --manifest-path services/runtime/Cargo.toml -- --check

run_step "Rust runtime tests" \
  cargo test --manifest-path services/runtime/Cargo.toml

run_step "Shared package tests" \
  npm test --prefix packages/shared

run_step "Shared package typecheck" \
  npm run typecheck --prefix packages/shared

if [[ "${ANYDESIGN_SKIP_K8S_E2E:-0}" == "1" ]]; then
  printf '\n==> K8s sandbox E2E skipped because ANYDESIGN_SKIP_K8S_E2E=1\n'
  exit 0
fi

run_step "K8s sandbox E2E" \
  bash infra/agent-sandbox/run-k8s-e2e.sh
