#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SESSION_DIR="${1:-}"
BATCH_PREFIX="${2:-efficiency-benchmark}"
CASES_FILE="${GENERATION_EFFICIENCY_CASES_FILE:-${ROOT_DIR}/infra/generation-reliability/runtime-efficiency-benchmark-cases.json}"
PAIR_RUNNER="${GENERATION_EFFICIENCY_PAIR_RUNNER:-${ROOT_DIR}/infra/generation-reliability/run-generation-context-paired-pair.sh}"
REPETITIONS="${GENERATION_EFFICIENCY_REPETITIONS:-3}"
DRY_RUN="${GENERATION_EFFICIENCY_DRY_RUN:-0}"
BENCHMARK_DIR="${GENERATION_EFFICIENCY_BENCHMARK_DIR:-${SESSION_DIR}/efficiency-benchmark}"

[[ -n "${SESSION_DIR}" ]] || {
  printf 'usage: %s <prepared-session-dir> [batch-prefix]\n' "$0" >&2
  exit 2
}
[[ "${BATCH_PREFIX}" =~ ^[a-z0-9][a-z0-9-]{0,30}$ ]] || {
  printf 'runtime_efficiency.invalid_batch_prefix: %s\n' "${BATCH_PREFIX}" >&2
  exit 2
}
[[ "${REPETITIONS}" =~ ^[3-9]$|^10$ ]] || {
  printf 'runtime_efficiency.repetitions_must_be_3_to_10: %s\n' "${REPETITIONS}" >&2
  exit 2
}
[[ "${DRY_RUN}" =~ ^[01]$ ]] || {
  printf 'runtime_efficiency.invalid_dry_run: %s\n' "${DRY_RUN}" >&2
  exit 2
}

SESSION_DIR="$(cd "${SESSION_DIR}" 2>/dev/null && pwd)" || {
  printf 'runtime_efficiency.session_missing: %s\n' "${SESSION_DIR}" >&2
  exit 2
}
SESSION_FILE="${SESSION_DIR}/session.json"
PAIRED_LEDGER="${SESSION_DIR}/cohort.ndjson"
for file in "${SESSION_FILE}" "${PAIRED_LEDGER}" "${CASES_FILE}" "${PAIR_RUNNER}"; do
  [[ -s "${file}" ]] || {
    printf 'runtime_efficiency.required_file_missing: %s\n' "${file}" >&2
    exit 2
  }
done

node "${ROOT_DIR}/services/runtime/scripts/generation-context-paired-cohort-ledger.mjs" \
  verify "${PAIRED_LEDGER}" >/dev/null || {
  printf 'runtime_efficiency.paired_ledger_preflight_failed\n' >&2
  exit 2
}

plan="$({
  node - "${SESSION_FILE}" "${PAIRED_LEDGER}" "${CASES_FILE}" "${BATCH_PREFIX}" "${REPETITIONS}" <<'NODE'
const crypto = require("node:crypto");
const fs = require("node:fs");
const [sessionFile, ledgerFile, casesFile, batchPrefix, repetitionsRaw] = process.argv.slice(2);
const session = JSON.parse(fs.readFileSync(sessionFile, "utf8"));
const casesRaw = fs.readFileSync(casesFile);
const manifest = JSON.parse(casesRaw);
const repetitions = Number(repetitionsRaw);
const sha256 = value => crypto.createHash("sha256").update(value).digest("hex");
if (session.schemaVersion !== "generation-context-paired-cohort-session@1") {
  throw new Error("unsupported paired Session schema");
}
if (session.source?.dirty !== false || typeof session.source?.commit !== "string" || !session.source.commit) {
  throw new Error("efficiency Benchmark requires a clean frozen Paired Session source");
}
if (session.fixtureManifestSha256 !== sha256(casesRaw)) {
  throw new Error("efficiency Benchmark case manifest differs from the prepared Paired Session");
}
const cases = manifest.cases;
if (manifest.schemaVersion !== "generation-real-provider-suite@1"
  || manifest.suiteKind !== "efficiency_benchmark" || cases?.length !== 10
  || cases.some(item => typeof item.id !== "string" || !item.id
    || item.kind !== "website" || item.expectedRoute !== "/"
    || typeof item.expectedText !== "string" || !item.expectedText
    || typeof item.prompt !== "string" || !/design system/i.test(item.prompt))
  || new Set(cases.map(item => item.id)).size !== 10
  || new Set(cases.map(item => item.expectedText)).size !== 10
  || new Set(cases.map(item => item.prompt)).size !== 10) {
  throw new Error("efficiency Benchmark requires ten unique Design System Website prompts at route /");
}
const records = fs.readFileSync(ledgerFile, "utf8").split(/\r?\n/).filter(Boolean).map(JSON.parse);
const sidesByPair = new Map();
for (const record of records.filter(item => item.kind === "sample")) {
  const sides = sidesByPair.get(record.payload.pairId) || new Set();
  sides.add(record.payload.side);
  sidesByPair.set(record.payload.pairId, sides);
}
const pairs = [];
for (const bucket of ["greenfield", "warm_copy_css"]) {
  for (let repetition = 1; repetition <= repetitions; repetition += 1) {
    const batchId = `${batchPrefix}-${bucket.replaceAll("_", "-")}-r${repetition}`;
    for (const fixture of cases) {
      const pairId = `${batchId}-${fixture.id}-${bucket.replaceAll("_", "-")}`;
      const sides = sidesByPair.get(pairId) || new Set();
      pairs.push({
        pairId,
        batchId,
        caseId: fixture.id,
        bucket,
        repetition,
        status: sides.size === 2 ? "complete" : sides.size === 0 ? "pending" : "incomplete",
      });
    }
  }
}
process.stdout.write(JSON.stringify({
  schemaVersion: "runtime-efficiency-benchmark-execution-plan@1",
  source: session.source,
  fixtureManifestSha256: session.fixtureManifestSha256,
  repetitions,
  pairCount: pairs.length,
  attemptCount: pairs.length * 2,
  pairs,
}));
NODE
})" || {
  printf 'runtime_efficiency.plan_failed\n' >&2
  exit 2
}

if [[ "${DRY_RUN}" == "1" ]]; then
  node -e 'process.stdout.write(`${JSON.stringify(JSON.parse(process.argv[1]), null, 2)}\n`)' "${plan}"
  exit 0
fi

while IFS=$'\t' read -r status batch_id case_id bucket pair_id; do
  if [[ "${status}" == "complete" ]]; then
    printf 'Runtime efficiency Pair already complete: %s\n' "${pair_id}"
    continue
  fi
  if [[ "${status}" == "incomplete" || -e "${SESSION_DIR}/pairs/${pair_id}" ]]; then
    printf 'runtime_efficiency.incomplete_pair_requires_audit: %s\n' "${pair_id}" >&2
    exit 3
  fi
  GENERATION_COHORT_CASES_FILE="${CASES_FILE}" \
    GENERATION_COHORT_MAX_CASE_ATTEMPTS=1 \
    bash "${PAIR_RUNNER}" "${SESSION_DIR}" "${batch_id}" "${case_id}" "${bucket}"
done < <(
  node -e 'for (const item of JSON.parse(process.argv[1]).pairs) process.stdout.write(`${item.status}\t${item.batchId}\t${item.caseId}\t${item.bucket}\t${item.pairId}\n`)' "${plan}"
)

if [[ ! -e "${BENCHMARK_DIR}" ]]; then
  node "${ROOT_DIR}/infra/generation-reliability/prepare-runtime-efficiency-benchmark.mjs" \
    "${PAIRED_LEDGER}" "${BENCHMARK_DIR}"
fi
node "${ROOT_DIR}/infra/generation-reliability/collect-runtime-efficiency-benchmark.mjs" \
  sync \
  "${PAIRED_LEDGER}" \
  "${BENCHMARK_DIR}/benchmark.ndjson" \
  "${BENCHMARK_DIR}/import-mapping.json"
node "${ROOT_DIR}/infra/generation-reliability/runtime-efficiency-benchmark-ledger.mjs" \
  evaluate "${BENCHMARK_DIR}/benchmark.ndjson"
