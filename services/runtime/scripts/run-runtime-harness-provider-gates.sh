#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$ROOT_DIR"

RUN_LOCAL_GATES="${RUNTIME_E2E_RUN_LOCAL_GATES:-1}"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
EVIDENCE_DIR="${RUNTIME_E2E_LOG_DIR:-$ROOT_DIR/.runtime-evidence/provider-$TIMESTAMP}"

load_gateway_env() {
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

  if [[ -z "${MODEL_GATEWAY_AUTH_TOKEN:-}" && -n "${MODEL_GATEWAY_AUTH_TOKEN_FILE:-}" ]]; then
    if [[ ! -f "$MODEL_GATEWAY_AUTH_TOKEN_FILE" ]]; then
      echo "MODEL_GATEWAY_AUTH_TOKEN_FILE does not exist: $MODEL_GATEWAY_AUTH_TOKEN_FILE" >&2
      exit 1
    fi
    MODEL_GATEWAY_AUTH_TOKEN="$(tr -d '\r\n' < "$MODEL_GATEWAY_AUTH_TOKEN_FILE")"
    export MODEL_GATEWAY_AUTH_TOKEN
  fi
}

write_dry_run_metadata() {
  mkdir -p "$EVIDENCE_DIR"
  {
    echo "timestampUtc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "providerGateDryRun=1"
    echo "runLocalGates=$RUN_LOCAL_GATES"
    echo "modelGatewayUrl=$MODEL_GATEWAY_URL"
    echo "modelResourceId=$DEEPSEEK_E2E_MODEL"
    echo "modelGatewayAuthTokenPresent=$([[ -n "${MODEL_GATEWAY_AUTH_TOKEN:-}" ]] && echo true || echo false)"
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

load_gateway_env

if [[ -z "${MODEL_GATEWAY_URL:-}" ]]; then
  echo "MODEL_GATEWAY_URL is required for Provider Resource-backed runtime E2E." >&2
  echo "Configure the Provider Secret through the Gateway Admin API or mounted Secret; never pass it to this harness." >&2
  exit 1
fi
export DEEPSEEK_E2E_MODEL="${DEEPSEEK_E2E_MODEL:-deepseek-v4-pro}"

mkdir -p "$EVIDENCE_DIR"

export RUNTIME_E2E_LOG_DIR="$EVIDENCE_DIR"
export RUNTIME_E2E_NPM_REGISTRY="${RUNTIME_E2E_NPM_REGISTRY:-https://registry.npmjs.org/}"

export RUNTIME_E2E_REQUIRE_COMPUTED_STYLE="${RUNTIME_E2E_REQUIRE_COMPUTED_STYLE:-1}"

if [[ "${RUNTIME_E2E_DRY_RUN:-0}" == "1" ]]; then
  write_dry_run_metadata
  echo "PROVIDER_GATE_DRY_RUN=1"
  echo "PROVIDER_EVIDENCE_DIR=$EVIDENCE_DIR"
  echo "MODEL_GATEWAY_URL=$MODEL_GATEWAY_URL"
  echo "MODEL_RESOURCE_ID=$DEEPSEEK_E2E_MODEL"
  echo "MODEL_GATEWAY_AUTH_TOKEN_PRESENT=$([[ -n "${MODEL_GATEWAY_AUTH_TOKEN:-}" ]] && echo true || echo false)"
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

if [[ -n "${RUNTIME_E2E_GENERATION_CONTEXT_COHORT_LEDGER:-}" && -n "${RUNTIME_E2E_GENERATION_CONTEXT_ROLLOUT_EVIDENCE:-}" ]]; then
  echo "RUNTIME_E2E_GENERATION_CONTEXT_COHORT_LEDGER and RUNTIME_E2E_GENERATION_CONTEXT_ROLLOUT_EVIDENCE are mutually exclusive" >&2
  exit 1
fi

if [[ -n "${RUNTIME_E2E_GENERATION_CONTEXT_COHORT_LEDGER:-}" ]]; then
  if [[ ! -f "$RUNTIME_E2E_GENERATION_CONTEXT_COHORT_LEDGER" ]]; then
    echo "RUNTIME_E2E_GENERATION_CONTEXT_COHORT_LEDGER does not exist: $RUNTIME_E2E_GENERATION_CONTEXT_COHORT_LEDGER" >&2
    exit 1
  fi
  node services/runtime/scripts/generation-context-paired-cohort-ledger.mjs verify \
    "$RUNTIME_E2E_GENERATION_CONTEXT_COHORT_LEDGER" \
    > "$EVIDENCE_DIR/generation-context-paired-cohort-ledger-verification.json"
  node services/runtime/scripts/generation-context-paired-cohort-ledger.mjs assemble \
    "$RUNTIME_E2E_GENERATION_CONTEXT_COHORT_LEDGER" \
    "$EVIDENCE_DIR/generation-context-rollout-evidence.json"
  RUNTIME_E2E_GENERATION_CONTEXT_ROLLOUT_EVIDENCE="$EVIDENCE_DIR/generation-context-rollout-evidence.json"
fi

if [[ -n "${RUNTIME_E2E_GENERATION_CONTEXT_ROLLOUT_EVIDENCE:-}" ]]; then
  if [[ ! -f "$RUNTIME_E2E_GENERATION_CONTEXT_ROLLOUT_EVIDENCE" ]]; then
    echo "RUNTIME_E2E_GENERATION_CONTEXT_ROLLOUT_EVIDENCE does not exist: $RUNTIME_E2E_GENERATION_CONTEXT_ROLLOUT_EVIDENCE" >&2
    exit 1
  fi
  node services/runtime/scripts/evaluate-generation-context-rollout.mjs \
    "$RUNTIME_E2E_GENERATION_CONTEXT_ROLLOUT_EVIDENCE" \
    > "$EVIDENCE_DIR/generation-context-rollout-evaluation.json"
  echo "GENERATION_CONTEXT_ROLLOUT_EVALUATION=$EVIDENCE_DIR/generation-context-rollout-evaluation.json"
fi

echo "PROVIDER_EVIDENCE_DIR=$EVIDENCE_DIR"
