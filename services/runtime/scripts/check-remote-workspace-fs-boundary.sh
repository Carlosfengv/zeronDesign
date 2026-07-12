#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
FILES=(
  "services/runtime/src/agent_loop.rs"
  "services/runtime/src/permission.rs"
  "services/runtime/src/tools/runtime.rs"
)

if [[ -f "$ROOT/services/runtime/src/http_api.rs" ]]; then
  FILES+=("services/runtime/src/http_api.rs")
elif [[ -d "$ROOT/services/runtime/src/http_api" ]]; then
  while IFS= read -r http_api_file; do
    FILES+=("${http_api_file#"$ROOT/"}")
  done < <(find "$ROOT/services/runtime/src/http_api" -type f -name '*.rs' | sort)
fi

if [[ -f "$ROOT/services/runtime/src/tools/sandbox.rs" ]]; then
  FILES+=("services/runtime/src/tools/sandbox.rs")
elif [[ -d "$ROOT/services/runtime/src/tools/sandbox" ]]; then
  while IFS= read -r sandbox_file; do
    FILES+=("${sandbox_file#"$ROOT/"}")
  done < <(find "$ROOT/services/runtime/src/tools/sandbox" -type f -name '*.rs' | sort)
fi

status=0
for relative in "${FILES[@]}"; do
  file="$ROOT/$relative"
  if ! awk -v source="$relative" '
    BEGIN { depth = 0; failed = 0 }
    /remote-fs-boundary: allow-begin [a-z0-9-]+/ { depth += 1; next }
    /remote-fs-boundary: allow-end [a-z0-9-]+/ {
      depth -= 1
      if (depth < 0) {
        printf "%s:%d: unmatched remote-fs-boundary allow-end\n", source, NR
        failed = 1
        depth = 0
      }
      next
    }
    {
      forbidden = ($0 ~ /std::fs::/) ||
        ($0 ~ /fs::(canonicalize|read|read_to_string|write|create_dir|create_dir_all|read_dir|metadata|remove_file|remove_dir_all|rename|File::create|File::open)/) ||
        ($0 ~ /\.exists\(\)/) ||
        ($0 ~ /\.is_dir\(\)/)
      if (forbidden && depth == 0) {
        printf "%s:%d: host filesystem call outside an owned allow boundary: %s\n", source, NR, $0
        failed = 1
      }
    }
    END {
      if (depth != 0) {
        printf "%s: unclosed remote-fs-boundary allow-begin (%d)\n", source, depth
        failed = 1
      }
      exit failed
    }
  ' "$file"; then
    status=1
  fi
done

if [[ "$status" -ne 0 ]]; then
  echo "remote workspace filesystem boundary check failed" >&2
  exit "$status"
fi

echo "remote workspace filesystem boundary check passed"
