#!/usr/bin/env bash
set -euo pipefail

STYLE_ONLY="${RUNTIME_E2E_STYLE_ONLY:-0}"
LOG_DIR="${RUNTIME_E2E_LOG_DIR:-}"
REQUIRE_COMPUTED_STYLE="${RUNTIME_E2E_REQUIRE_COMPUTED_STYLE:-0}"
SUMMARY_WRITTEN=0
APPROVAL_REFERENCE="${RUNTIME_PROVIDER_APPROVAL_ID:-}"

export DEEPSEEK_BASE_URL="${DEEPSEEK_BASE_URL:-https://api.deepseek.com}"
export DEEPSEEK_E2E_MODEL="${DEEPSEEK_E2E_MODEL:-deepseek-chat}"
export RUNTIME_E2E_NPM_REGISTRY="${RUNTIME_E2E_NPM_REGISTRY:-https://registry.npmjs.org/}"
export RUNTIME_BROWSER_EXECUTABLE="${RUNTIME_BROWSER_EXECUTABLE:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
export RUNTIME_BROWSER_COLLECTOR_EXECUTABLE="${RUNTIME_BROWSER_COLLECTOR_EXECUTABLE:-$(command -v node || true)}"

write_metadata() {
  if [[ -z "$LOG_DIR" ]]; then
    return
  fi

  mkdir -p "$LOG_DIR"
  {
    echo "timestampUtc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "styleOnly=$STYLE_ONLY"
    echo "deepseekBaseUrl=$DEEPSEEK_BASE_URL"
    echo "deepseekModel=$DEEPSEEK_E2E_MODEL"
    echo "deepseekApiKeyPresent=$([[ -n "${DEEPSEEK_API_KEY:-}" ]] && echo true || echo false)"
    echo "providerApprovalReference=$APPROVAL_REFERENCE"
    echo "npmRegistry=$RUNTIME_E2E_NPM_REGISTRY"
    echo "artifactUrl=${RUNTIME_E2E_ARTIFACT_URL:-}"
    echo "styleProject=${RUNTIME_E2E_STYLE_PROJECT:-real-http-website}"
    echo "styleStage=${RUNTIME_E2E_STYLE_STAGE:-edit}"
    echo "styleSelector=${RUNTIME_E2E_STYLE_SELECTOR:-:root}"
    echo "styleProperty=${RUNTIME_E2E_STYLE_PROPERTY:---runtime-primary}"
    echo "styleExpected=${RUNTIME_E2E_STYLE_EXPECTED:-#f97316}"
    echo "requireComputedStyle=$REQUIRE_COMPUTED_STYLE"
  } > "$LOG_DIR/run-metadata.env"
}

run_with_optional_log() {
  local log_name="$1"
  shift

  if [[ -z "$LOG_DIR" ]]; then
    "$@"
    return
  fi

  mkdir -p "$LOG_DIR"
  "$@" 2>&1 | tee "$LOG_DIR/$log_name"
}

write_evidence_summary() {
  if [[ -z "$LOG_DIR" ]]; then
    return
  fi

  local provider_log="$LOG_DIR/provider-lifecycle.log"
  local computed_style_log="$LOG_DIR/computed-style.log"
  local summary_args=(
    services/runtime/scripts/summarize-real-provider-evidence.mjs
    --out "$LOG_DIR/evidence-summary.json"
    --computed-style-project "${RUNTIME_E2E_STYLE_PROJECT:-real-http-website}"
    --computed-style-stage "${RUNTIME_E2E_STYLE_STAGE:-edit}"
  )

  if [[ -f "$provider_log" ]]; then
    summary_args+=(--log "$provider_log")
    summary_args+=(--require-approval-reference "$APPROVAL_REFERENCE")
  else
    summary_args+=(--provider-optional)
  fi

  if [[ -f "$computed_style_log" ]]; then
    summary_args+=(--computed-style-log "$computed_style_log")
  fi

  case "${REAL_PROVIDER_PROJECT_FILTER:-}" in
    website)
      summary_args+=(--project real-http-website)
      ;;
    docs)
      summary_args+=(--project real-http-docs)
      ;;
  esac

  if [[ -n "${RUNTIME_E2E_ARTIFACT_URL:-}" || "$REQUIRE_COMPUTED_STYLE" == "1" ]]; then
    summary_args+=(--require-computed-style)
  fi
  if [[ "${REAL_PROVIDER_PROJECT_FILTER:-}" != "docs" ]]; then
    summary_args+=(--require-dcp-project real-http-website)
    summary_args+=(--require-repair-project real-http-website)
  fi
  if [[ -f "$provider_log" || -f "$computed_style_log" ]]; then
    SUMMARY_NODE="${RUNTIME_E2E_NODE:-node}"
    local summary_status=0
    "$SUMMARY_NODE" "${summary_args[@]}"
    summary_status=$?
    SUMMARY_WRITTEN=1
    return "$summary_status"
  fi
}

resolve_artifact_url_from_provider_log() {
  if [[ -n "${RUNTIME_E2E_ARTIFACT_URL:-}" || -z "$LOG_DIR" ]]; then
    return
  fi

  local provider_log="$LOG_DIR/provider-lifecycle.log"
  if [[ ! -f "$provider_log" ]]; then
    return
  fi

  local node_bin="${RUNTIME_E2E_NODE:-node}"
  local artifact_url
  artifact_url="$("$node_bin" services/runtime/scripts/extract-provider-artifact-url.mjs \
    --log "$provider_log" \
    --project "${RUNTIME_E2E_STYLE_PROJECT:-real-http-website}" \
    --stage "${RUNTIME_E2E_STYLE_STAGE:-edit}" 2>/dev/null || true)"
  if [[ -z "$artifact_url" ]]; then
    return
  fi

  export RUNTIME_E2E_ARTIFACT_URL="$artifact_url"
  if [[ -n "$LOG_DIR" ]]; then
    echo "resolvedArtifactUrl=$artifact_url" >> "$LOG_DIR/run-metadata.env"
  fi
}

finish() {
  local exit_code=$?
  local summary_code=0
  trap - EXIT
  set +e

  if [[ "$SUMMARY_WRITTEN" != "1" ]]; then
    write_evidence_summary
    summary_code=$?
  fi

  if [[ "$exit_code" -eq 0 && "$summary_code" -ne 0 ]]; then
    exit_code="$summary_code"
  fi

  exit "$exit_code"
}

trap finish EXIT

write_metadata

if [[ "$STYLE_ONLY" != "1" ]]; then
  if [[ -z "${DEEPSEEK_API_KEY:-}" ]]; then
    echo "DEEPSEEK_API_KEY is required" >&2
    exit 1
  fi
  if [[ -z "$APPROVAL_REFERENCE" ]]; then
    echo "RUNTIME_PROVIDER_APPROVAL_ID is required" >&2
    exit 1
  fi
  if [[ "${REAL_PROVIDER_PROJECT_FILTER:-}" != "docs" ]]; then
    if [[ ! -x "$RUNTIME_BROWSER_EXECUTABLE" ]]; then
      echo "RUNTIME_BROWSER_EXECUTABLE is not executable: $RUNTIME_BROWSER_EXECUTABLE" >&2
      exit 1
    fi
    if [[ -z "$RUNTIME_BROWSER_COLLECTOR_EXECUTABLE" || ! -x "$RUNTIME_BROWSER_COLLECTOR_EXECUTABLE" ]]; then
      echo "RUNTIME_BROWSER_COLLECTOR_EXECUTABLE is not executable: $RUNTIME_BROWSER_COLLECTOR_EXECUTABLE" >&2
      exit 1
    fi
  fi

  run_with_optional_log provider-lifecycle.log \
    cargo test \
    --manifest-path services/runtime/Cargo.toml \
    --test http_api \
    real_provider_public_runtime_website_and_docs_lifecycle_matrix \
    -- --ignored --nocapture
fi

resolve_artifact_url_from_provider_log

if [[ -n "${RUNTIME_E2E_ARTIFACT_URL:-}" ]]; then
  NODE_BIN="${RUNTIME_E2E_NODE:-node}"
  BUNDLED_NODE_MODULES="${HOME}/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules"
  if [[ -z "${NODE_PATH:-}" && -d "$BUNDLED_NODE_MODULES" ]]; then
    export NODE_PATH="$BUNDLED_NODE_MODULES"
  fi
  run_with_optional_log computed-style.log \
    "$NODE_BIN" services/runtime/scripts/verify-computed-style.mjs \
    --url "${RUNTIME_E2E_ARTIFACT_URL}" \
    --selector "${RUNTIME_E2E_STYLE_SELECTOR:-:root}" \
    --property "${RUNTIME_E2E_STYLE_PROPERTY:---runtime-primary}" \
    --expected "${RUNTIME_E2E_STYLE_EXPECTED:-#f97316}"
elif [[ "$STYLE_ONLY" == "1" ]]; then
  echo "RUNTIME_E2E_ARTIFACT_URL is required when RUNTIME_E2E_STYLE_ONLY=1" >&2
  exit 1
fi
