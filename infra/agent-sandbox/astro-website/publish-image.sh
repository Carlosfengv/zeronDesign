#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

IMAGE="${SANDBOX_IMAGE:-ghcr.io/carlosfengv/zerondesign/astro-website-sandbox:0.1.0}"
PLATFORMS="${SANDBOX_IMAGE_PLATFORMS:-linux/amd64,linux/arm64}"
PUSH_IMAGE="${PUSH_IMAGE:-1}"

cd "${ROOT_DIR}"

if [[ "${PUSH_IMAGE}" == "0" ]]; then
  docker build \
    -f infra/agent-sandbox/astro-website/Dockerfile \
    -t "${IMAGE}" \
    infra/agent-sandbox
else
  docker buildx build \
    --platform "${PLATFORMS}" \
    -f infra/agent-sandbox/astro-website/Dockerfile \
    -t "${IMAGE}" \
    --push \
    infra/agent-sandbox
fi

