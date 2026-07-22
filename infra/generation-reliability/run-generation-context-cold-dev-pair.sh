#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SESSION_DIR="${1:-}"
BATCH_ID="${2:-}"
CASE_ID="${3:-}"

if [[ -z "${SESSION_DIR}" || -z "${BATCH_ID}" || -z "${CASE_ID}" ]]; then
  printf 'usage: %s <prepared-session-dir> <batch-id> <website-case-id>\n' "$0" >&2
  exit 2
fi

exec bash "${ROOT_DIR}/infra/generation-reliability/run-generation-context-paired-pair.sh" \
  "${SESSION_DIR}" "${BATCH_ID}" "${CASE_ID}" cold_dev
