#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
STRICT="${SANDBOX_ARCHITECTURE_STRICT:-0}"
SANDBOX_FILE="$ROOT/services/runtime/src/tools/sandbox.rs"
SANDBOX_DIR="$ROOT/services/runtime/src/tools/sandbox"
TEMPLATES_DIR="$ROOT/services/runtime/src/templates"
PROFILES_BUILD="$ROOT/services/runtime/src/profiles/build.rs"

status=0

fail() {
  printf '%s\n' "$1" >&2
  status=1
}

if [[ -f "$SANDBOX_FILE" ]]; then
  lines="$(wc -l < "$SANDBOX_FILE" | tr -d ' ')"
  if (( lines > 10269 )); then
    fail "SBX-001/SIZE-001: sandbox.rs grew beyond the frozen 10269-line baseline: $lines"
  fi
  if [[ "$STRICT" == "1" ]]; then
    fail "SBX-001: strict mode requires tools/sandbox/mod.rs compatibility facade"
  fi
fi

if [[ -d "$SANDBOX_DIR" ]]; then
  facade="$SANDBOX_DIR/mod.rs"
  if [[ ! -f "$facade" ]]; then
    fail "SBX-001: tools/sandbox/mod.rs is missing"
  elif [[ "$STRICT" == "1" ]]; then
    lines="$(wc -l < "$facade" | tr -d ' ')"
    if (( lines > 200 )); then
      fail "SBX-001/SIZE-001: sandbox facade exceeds 200 lines: $lines"
    fi
  fi

  if [[ -d "$SANDBOX_DIR/fs" ]] && rg -n -i 'astro|fumadocs|next\.config|docusaurus' "$SANDBOX_DIR/fs"; then
    fail "FS-001: generic fs tools contain framework-specific knowledge"
  fi

  if [[ "$STRICT" == "1" ]]; then
    while IFS= read -r production_file; do
      lines="$(wc -l < "$production_file" | tr -d ' ')"
      if (( lines > 800 )); then
        fail "SIZE-001: production sandbox module exceeds 800 lines: ${production_file#$ROOT/} ($lines)"
      fi
    done < <(find "$SANDBOX_DIR" -type f -name '*.rs' -print | sort)
  fi
fi

if [[ -d "$TEMPLATES_DIR" ]]; then
  if rg -n 'std::fs|tokio::process|Command::new|reqwest::|RuntimeStore|WorkspaceBackend' "$TEMPLATES_DIR"; then
    fail "TPL-002/PORT-001: pure templates depend on I/O, process, network, store, or workspace adapters"
  fi
fi

if [[ -f "$PROFILES_BUILD" ]]; then
  writer_count="$(rg -c '^fn write_(astro|fumadocs)_project' "$PROFILES_BUILD" || true)"
  writer_count="${writer_count:-0}"
  if (( writer_count > 2 )); then
    fail "TPL-001: profiles/build.rs added another template writer"
  fi
  if [[ "$STRICT" == "1" ]] && (( writer_count > 0 )); then
    fail "TPL-001: strict mode forbids duplicate template writers in profiles/build.rs"
  fi
  if rg -n 'TemplateKind|customize_(astro|fumadocs)_project|write_(astro|fumadocs)_project' "$PROFILES_BUILD"; then
    fail "TPL-001/TPL-003: profiles/build.rs contains template-specific build dispatch"
  fi
  if [[ "$STRICT" == "1" ]]; then
    lines="$(wc -l < "$PROFILES_BUILD" | tr -d ' ')"
    if (( lines > 800 )); then
      fail "SIZE-001: profiles/build.rs exceeds 800 lines: $lines"
    fi
  fi
fi

if rg -n 'template\s*==\s*Some\("(astro-website|fumadocs-docs)"\)|template\s*==\s*"(astro-website|fumadocs-docs)"|match\s+template[^\n]*\{' \
  "$SANDBOX_DIR" --glob '*.rs' --glob '!**/templates/**'; then
  fail "TPL-003: generic sandbox code branches on a concrete template id"
fi

if rg -n 'format!\([^\n]*template[^\n]*pool|format!\([^\n]*pool[^\n]*template' \
  "$ROOT/services/runtime/src"; then
  fail "INFRA-001: Runtime derives WarmPool names from template ids"
fi

if [[ "$status" -ne 0 ]]; then
  exit "$status"
fi

printf 'sandbox architecture check passed (strict=%s)\n' "$STRICT"
