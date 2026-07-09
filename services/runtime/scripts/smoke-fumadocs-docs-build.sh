#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

cd "$ROOT_DIR"
cargo test \
  --manifest-path services/runtime/Cargo.toml \
  --test sandbox_tools \
  fumadocs_docs_real_next_build_smoke \
  -- \
  --ignored \
  --nocapture
