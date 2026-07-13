#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
FIXTURE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/runtime-fumadocs-theme.XXXXXX")"
trap 'rm -rf "$FIXTURE_DIR"' EXIT

cat > "$FIXTURE_DIR/theme.css" <<'CSS'
:root {
  --runtime-bg: #fafafa;
  --runtime-text: #202020;
  --runtime-primary: #2563eb;
  --runtime-surface: #ffffff;
  --runtime-surface-strong: #e2e8f0;
  --runtime-muted: #475569;
  --runtime-border: #cbd5e1;
  --runtime-font-sans: sans-serif;
  --color-fd-background: #ffffff;
  --color-fd-foreground: #111111;
  --color-fd-primary: #2563eb;
  --color-fd-muted: #f4f4f5;
  --color-fd-muted-foreground: #71717a;
  --color-fd-border: #e4e4e7;
  --color-fd-card: #ffffff;
}
.dark {
  --color-fd-background: #101010;
  --color-fd-foreground: #f2f2f2;
  --color-fd-primary: #60a5fa;
  --color-fd-muted: #27272a;
  --color-fd-muted-foreground: #a1a1aa;
  --color-fd-border: #3f3f46;
  --color-fd-card: #18181b;
}
CSS
sed '/^@import /d' \
  "$ROOT_DIR/services/runtime/src/templates/fumadocs_docs/files/app/global.css" \
  >> "$FIXTURE_DIR/theme.css"

{
  printf '%s\n' '<!doctype html><html class="dark"><head><style>'
  cat "$FIXTURE_DIR/theme.css"
  printf '%s\n' '</style></head><body><main>Dark theme probe</main></body></html>'
} > "$FIXTURE_DIR/index.html"

export RUNTIME_E2E_ARTIFACT_URL="file://$FIXTURE_DIR/index.html"
export RUNTIME_E2E_STYLE_ONLY=1
export RUNTIME_E2E_STYLE_SELECTOR="body"
export RUNTIME_E2E_STYLE_PROPERTY="background-color"
export RUNTIME_E2E_STYLE_EXPECTED="#101010"
bash "$ROOT_DIR/services/runtime/scripts/run-real-provider-http-lifecycle-e2e.sh"

export RUNTIME_E2E_STYLE_PROPERTY="color"
export RUNTIME_E2E_STYLE_EXPECTED="#f2f2f2"
bash "$ROOT_DIR/services/runtime/scripts/run-real-provider-http-lifecycle-e2e.sh"
