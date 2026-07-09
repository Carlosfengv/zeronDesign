#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$ROOT_DIR"

RUN_LOCAL_GATES="${RUNTIME_E2E_RUN_LOCAL_GATES:-1}"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
EVIDENCE_DIR="${RUNTIME_E2E_LOG_DIR:-$ROOT_DIR/.runtime-evidence/provider-$TIMESTAMP}"

load_provider_env() {
  if [[ -n "${RUNTIME_E2E_ENV_FILE:-}" ]]; then
    if [[ ! -f "$RUNTIME_E2E_ENV_FILE" ]]; then
      echo "RUNTIME_E2E_ENV_FILE does not exist: $RUNTIME_E2E_ENV_FILE" >&2
      exit 1
    fi
    set -a
    # shellcheck disable=SC1090
    source "$RUNTIME_E2E_ENV_FILE"
    set +a
  fi

  if [[ -z "${DEEPSEEK_API_KEY:-}" && -n "${DEEPSEEK_API_KEY_FILE:-}" ]]; then
    if [[ ! -f "$DEEPSEEK_API_KEY_FILE" ]]; then
      echo "DEEPSEEK_API_KEY_FILE does not exist: $DEEPSEEK_API_KEY_FILE" >&2
      exit 1
    fi
    DEEPSEEK_API_KEY="$(tr -d '\r\n' < "$DEEPSEEK_API_KEY_FILE")"
    export DEEPSEEK_API_KEY
  fi
}

write_dry_run_metadata() {
  mkdir -p "$EVIDENCE_DIR"
  {
    echo "timestampUtc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "providerGateDryRun=1"
    echo "runLocalGates=$RUN_LOCAL_GATES"
    echo "deepseekApiKeyPresent=true"
    echo "npmRegistry=$RUNTIME_E2E_NPM_REGISTRY"
    echo "artifactUrl=${RUNTIME_E2E_ARTIFACT_URL:-}"
    echo "requireComputedStyle=$RUNTIME_E2E_REQUIRE_COMPUTED_STYLE"
    echo "styleProject=${RUNTIME_E2E_STYLE_PROJECT:-real-http-website}"
    echo "styleStage=${RUNTIME_E2E_STYLE_STAGE:-edit}"
    echo "styleSelector=${RUNTIME_E2E_STYLE_SELECTOR:-:root}"
    echo "styleProperty=${RUNTIME_E2E_STYLE_PROPERTY:---runtime-primary}"
    echo "styleExpected=${RUNTIME_E2E_STYLE_EXPECTED:-#f97316}"
  } > "$EVIDENCE_DIR/run-metadata.env"
}

load_provider_env

if [[ -z "${DEEPSEEK_API_KEY:-}" ]]; then
  echo "DEEPSEEK_API_KEY is required for provider-backed runtime E2E." >&2
  echo "Export DEEPSEEK_API_KEY, set DEEPSEEK_API_KEY_FILE, or set RUNTIME_E2E_ENV_FILE, then rerun this script." >&2
  exit 1
fi

mkdir -p "$EVIDENCE_DIR"

export RUNTIME_E2E_LOG_DIR="$EVIDENCE_DIR"
export RUNTIME_E2E_NPM_REGISTRY="${RUNTIME_E2E_NPM_REGISTRY:-https://registry.npmjs.org/}"

export RUNTIME_E2E_REQUIRE_COMPUTED_STYLE="${RUNTIME_E2E_REQUIRE_COMPUTED_STYLE:-1}"

if [[ "${RUNTIME_E2E_DRY_RUN:-0}" == "1" ]]; then
  write_dry_run_metadata
  echo "PROVIDER_GATE_DRY_RUN=1"
  echo "PROVIDER_EVIDENCE_DIR=$EVIDENCE_DIR"
  echo "DEEPSEEK_API_KEY_PRESENT=true"
  echo "RUNTIME_E2E_REQUIRE_COMPUTED_STYLE=$RUNTIME_E2E_REQUIRE_COMPUTED_STYLE"
  echo "RUNTIME_E2E_ARTIFACT_URL=${RUNTIME_E2E_ARTIFACT_URL:-}"
  echo "RUNTIME_E2E_STYLE_PROJECT=${RUNTIME_E2E_STYLE_PROJECT:-real-http-website}"
  echo "RUNTIME_E2E_STYLE_STAGE=${RUNTIME_E2E_STYLE_STAGE:-edit}"
  echo "RUNTIME_E2E_STYLE_SELECTOR=${RUNTIME_E2E_STYLE_SELECTOR:-:root}"
  echo "RUNTIME_E2E_STYLE_PROPERTY=${RUNTIME_E2E_STYLE_PROPERTY:---runtime-primary}"
  echo "RUNTIME_E2E_STYLE_EXPECTED=${RUNTIME_E2E_STYLE_EXPECTED:-#f97316}"
  exit 0
fi

if [[ "$RUN_LOCAL_GATES" == "1" ]]; then
  bash services/runtime/scripts/run-runtime-harness-local-gates.sh
fi

bash services/runtime/scripts/run-real-provider-http-lifecycle-e2e.sh

echo "PROVIDER_EVIDENCE_DIR=$EVIDENCE_DIR"
