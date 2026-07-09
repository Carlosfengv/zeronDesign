#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
LOG_DIR="${RUNTIME_E2E_LOG_DIR:-$ROOT_DIR/.runtime-evidence/style-only-local-$TIMESTAMP}"
FIXTURE_DIR="${RUNTIME_E2E_STYLE_FIXTURE_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/runtime-style-fixture.XXXXXX")}"
FIXTURE_PARENT="$(dirname "$FIXTURE_DIR")"

mkdir -p "$FIXTURE_DIR/_astro"
cat > "$FIXTURE_DIR/index.html" <<'HTML'
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Runtime Style Artifact Fixture</title>
    <link rel="stylesheet" href="/_astro/app.css" />
    <link rel="stylesheet" href="/_astro/../../runtime-style-escape.css" />
  </head>
  <body>
    <main>
      <h1 id="probe">Runtime style probe</h1>
    </main>
  </body>
</html>
HTML
cat > "$FIXTURE_DIR/_astro/app.css" <<'CSS'
:root {
  --runtime-primary: #f97316;
}
#probe {
  color: var(--runtime-primary);
}
CSS
cat > "$FIXTURE_PARENT/runtime-style-escape.css" <<'CSS'
#probe {
  color: #000000;
}
CSS

cd "$ROOT_DIR"
RUNTIME_E2E_STYLE_ONLY=1 \
RUNTIME_E2E_LOG_DIR="$LOG_DIR" \
RUNTIME_E2E_ARTIFACT_URL="file://$FIXTURE_DIR/index.html" \
RUNTIME_E2E_STYLE_SELECTOR="${RUNTIME_E2E_STYLE_SELECTOR:-#probe}" \
RUNTIME_E2E_STYLE_PROPERTY="${RUNTIME_E2E_STYLE_PROPERTY:-color}" \
RUNTIME_E2E_STYLE_EXPECTED="${RUNTIME_E2E_STYLE_EXPECTED:-#f97316}" \
bash services/runtime/scripts/run-real-provider-http-lifecycle-e2e.sh

echo "STYLE_FIXTURE_DIR=$FIXTURE_DIR"
echo "STYLE_EVIDENCE_DIR=$LOG_DIR"
