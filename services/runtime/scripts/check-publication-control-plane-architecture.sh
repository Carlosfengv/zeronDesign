#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
PUBLICATION_DIR="$ROOT/services/runtime/src/publication"
ROUTE_FILE="$ROOT/services/runtime/src/http_api/routes/publication.rs"
CONTRACT_FILE="$ROOT/services/runtime/src/http_api/contracts/publication.rs"
SCHEMA_DIR="$ROOT/services/runtime/contracts"
status=0

fail() {
  printf '%s\n' "$1" >&2
  status=1
}

for required in backend.rs model.rs store.rs store_reconcile.rs controller.rs kubernetes.rs; do
  [[ -f "$PUBLICATION_DIR/$required" ]] \
    || fail "PUB-001: missing publication control-plane module: $required"
done

for file in "$PUBLICATION_DIR/backend.rs" "$PUBLICATION_DIR/model.rs" "$PUBLICATION_DIR/store.rs" "$PUBLICATION_DIR/store_reconcile.rs" "$PUBLICATION_DIR/controller.rs" "$PUBLICATION_DIR/kubernetes.rs"; do
  lines="$(wc -l < "$file" | tr -d ' ')"
  (( lines <= 700 )) \
    || fail "PUB-009: publication production module exceeds 700 lines: ${file#"$ROOT/"} ($lines)"
done

for required in publish-operation-v1.schema.json work-runtime-state-v1.schema.json publication-outbox-v1.schema.json; do
  [[ -f "$SCHEMA_DIR/$required" ]] \
    || fail "PUB-002: missing frozen publication contract: $required"
done

[[ -f "$ROUTE_FILE" && -f "$CONTRACT_FILE" ]] \
  || fail "PUB-003: publication HTTP routes and contracts must be isolated modules"

if grep -InE 'kube::|k8s_openapi|kubectl|Command::new\("(docker|helm|kubectl)"\)' \
  "$PUBLICATION_DIR/backend.rs" "$PUBLICATION_DIR/model.rs" "$PUBLICATION_DIR/store.rs" \
  "$PUBLICATION_DIR/store_reconcile.rs" "$PUBLICATION_DIR/controller.rs" "$ROUTE_FILE"; then
  fail "PUB-004: publication application modules must not depend on Kubernetes or container implementations"
fi

if ! grep -Eq 'Client::try_default' "$PUBLICATION_DIR/kubernetes.rs" \
  || grep -Eq 'Command::new\("kubectl"\)|kubectl' "$PUBLICATION_DIR/kubernetes.rs"; then
  fail "PUB-010: production Kubernetes adapter must use the Kubernetes API without kubectl subprocesses"
fi

if ! grep -Eq 'PatchParams::apply\(FIELD_MANAGER\)' "$PUBLICATION_DIR/kubernetes.rs" \
  || ! grep -Eq 'expected_service_resource_version' "$PUBLICATION_DIR/backend.rs"; then
  fail "PUB-011: Kubernetes resources require fixed server-side apply ownership and CAS identity"
fi

if rg -n '"kind"[[:space:]]*:[[:space:]]*"Ingress"|Api<Ingress>|networking::v1::Ingress' \
  "$PUBLICATION_DIR/kubernetes.rs" >/dev/null; then
  fail "PUB-012: G6 Kubernetes adapter must not create Published Ingress"
fi

if ! grep -Eq 'struct PublicationCommit' "$PUBLICATION_DIR/store.rs" \
  || ! grep -Eq 'operation: PublishOperation' "$PUBLICATION_DIR/store.rs" \
  || ! grep -Eq 'runtime: WorkRuntimeState' "$PUBLICATION_DIR/store.rs" \
  || ! grep -Eq 'outbox: PublicationOutboxEvent' "$PUBLICATION_DIR/store.rs"; then
  fail "PUB-005: operation, desired state, and outbox require one durable commit record"
fi

if ! grep -Eq 'controller/work-runtime' "$PUBLICATION_DIR/controller.rs" \
  || ! grep -Eq 'spawn_with_shutdown' "$PUBLICATION_DIR/controller.rs"; then
  fail "PUB-006: WorkRuntimeController must have one Supervisor-owned task"
fi

if ! grep -Eq 'idempotency_key_hash' "$PUBLICATION_DIR/model.rs" \
  || ! grep -Eq 'request_hash' "$PUBLICATION_DIR/model.rs"; then
  fail "PUB-007: persisted operations must hash Idempotency-Key and bind the canonical request"
fi

if rg -n '/projects/\{project_id\}/(publish|unpublish|rollback)|/operations/\{operation_id\}' \
  "$ROOT/services/runtime/src/http_api" -g '*.rs' -g '!**/routes/publication.rs' >/dev/null; then
  fail "PUB-008: publication routes leaked outside routes/publication.rs"
fi

if [[ "$status" -ne 0 ]]; then
  exit "$status"
fi

echo "publication control-plane architecture check passed"
