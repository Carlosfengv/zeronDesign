#!/usr/bin/env sh
set -eu

mkdir -p /workspace/inputs /workspace/project
mkdir -p /workspace/outputs/build /workspace/outputs/export
mkdir -p /workspace/outputs/screenshots /workspace/outputs/reports
mkdir -p /workspace/outputs/tool-results
mkdir -p /workspace/state/checkpoints

test -f /workspace/state/tasks.json || echo '[]' > /workspace/state/tasks.json
test -f /workspace/state/preview.json || echo '{}' > /workspace/state/preview.json
