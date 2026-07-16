#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
lock_file="${ROOT_DIR}/infra/agent-sandbox/images.lock.json"
evidence_path="${PREFLIGHT_EVIDENCE_PATH:-}"
cd "${ROOT_DIR}"

if [[ -n "${evidence_path}" ]]; then
  mkdir -p "$(dirname "${evidence_path}")"
fi

for command in docker k3d kubectl openssl node cargo; do
  command -v "${command}" >/dev/null || {
    printf 'preflight.missing_command: %s\n' "${command}" >&2
    exit 2
  }
done
k3d_version="$(k3d version | awk '/^k3d version / {sub(/^v/, "", $3); print $3}')"
[[ "${k3d_version}" == "5.8.3" ]] || {
  printf 'preflight.k3d_version_mismatch: expected=5.8.3 actual=%s\n' \
    "${k3d_version:-unknown}" >&2
  exit 2
}
docker info >/dev/null 2>&1 || {
  printf 'preflight.docker_unavailable\n' >&2
  exit 3
}
docker buildx version >/dev/null 2>&1 || {
  printf 'preflight.buildx_unavailable\n' >&2
  exit 3
}
browser="${RUNTIME_BROWSER_EXECUTABLE:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
[[ -x "${browser}" ]] || {
  printf 'preflight.browser_unavailable: %s\n' "${browser}" >&2
  exit 4
}

node - "${lock_file}" "${PREFLIGHT_PREFETCH_IMAGES:-1}" "${evidence_path}" <<'NODE'
const crypto = require("crypto");
const fs = require("fs");
const { spawnSync } = require("child_process");
const { registryFallbackReason } = require("./infra/agent-sandbox/preflight-registry-policy.cjs");
const lockRaw = fs.readFileSync(process.argv[2], "utf8");
const lock = JSON.parse(lockRaw);
const prefetchImages = process.argv[3] === "1";
const evidencePath = process.argv[4] || "";
const registryAttempts = Math.max(1, Number(process.env.PREFLIGHT_REGISTRY_ATTEMPTS || 5));
const baseDelayMs = Math.max(0, Number(process.env.PREFLIGHT_REGISTRY_RETRY_BASE_MS || 1000));
const sleep = ms => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
const runDockerWithRetry = (args, timeout) => {
  let result;
  for (let attempt = 1; attempt <= registryAttempts; attempt += 1) {
    result = spawnSync("docker", args, { encoding: "utf8", timeout });
    if (!result.error && result.status === 0) return result;
    if (attempt < registryAttempts) sleep(baseDelayMs * 2 ** (attempt - 1));
  }
  return result;
};
const runWithFallback = (argsForRef, canonicalRef, fallbackRef, timeout) => {
  const canonical = runDockerWithRetry(argsForRef(canonicalRef), timeout);
  if (!canonical.error && canonical.status === 0) {
    return { result: canonical, source: "canonical", selectedRef: canonicalRef, fallbackReason: null };
  }
  const reason = registryFallbackReason(canonical);
  if (!fallbackRef || !reason) {
    return { result: canonical, source: "canonical", selectedRef: canonicalRef, fallbackReason: null };
  }
  const fallback = runDockerWithRetry(argsForRef(fallbackRef), timeout);
  if (!fallback.error && fallback.status === 0) {
    return { result: fallback, source: "approved-fallback", selectedRef: fallbackRef, fallbackReason: reason };
  }
  return { result: fallback, source: "approved-fallback", selectedRef: fallbackRef, fallbackReason: reason };
};
if (lock.schemaVersion !== "anydesign-runtime-images@1") throw new Error("invalid image lock schema");
const failures = [];
const drifts = [];
const entries = [];
for (const [name, image] of Object.entries(lock.images || {})) {
  if (typeof image.ref !== "string" || !/^sha256:[a-f0-9]{64}$/.test(image.digest || "")) {
    failures.push(`invalid image lock entry: ${name}`);
    continue;
  }
  if (image.fallbackRef !== undefined
    && (typeof image.fallbackRef !== "string"
      || !/^mirror\.gcr\.io\/[A-Za-z0-9._/-]+:[A-Za-z0-9._-]+$/.test(image.fallbackRef))) {
    failures.push(`invalid approved fallback image ref: ${name}`);
    continue;
  }
  const lockedRef = `${image.ref}@${image.digest}`;
  const fallbackLockedRef = image.fallbackRef ? `${image.fallbackRef}@${image.digest}` : null;
  const lockedInspect = runWithFallback(
    ref => ["buildx", "imagetools", "inspect", ref, "--format", "{{json .Manifest.Digest}}"],
    lockedRef,
    fallbackLockedRef,
    45000,
  );
  if (lockedInspect.result.error?.code === "ETIMEDOUT") {
    failures.push(`infrastructure.registry_unavailable: ${name} locked digest inspect timeout`);
    continue;
  }
  if (lockedInspect.result.status !== 0) {
    failures.push(`infrastructure.registry_unavailable: ${name} locked digest ${(lockedInspect.result.stderr || "").trim()}`);
    continue;
  }
  const lockedActual = lockedInspect.result.stdout.trim().replaceAll('"', "");
  if (lockedActual !== image.digest) {
    failures.push(`locked image identity mismatch: ${name} expected=${image.digest} actual=${lockedActual}`);
    continue;
  }

  const tagInspect = runWithFallback(
    ref => ["buildx", "imagetools", "inspect", ref, "--format", "{{json .Manifest.Digest}}"],
    image.ref,
    image.fallbackRef,
    45000,
  );
  if (tagInspect.result.error?.code === "ETIMEDOUT") {
    failures.push(`infrastructure.registry_unavailable: ${name} mutable tag inspect timeout`);
    continue;
  }
  if (tagInspect.result.status !== 0) {
    failures.push(`infrastructure.registry_unavailable: ${name} mutable tag ${(tagInspect.result.stderr || "").trim()}`);
    continue;
  }
  const tagActual = tagInspect.result.stdout.trim().replaceAll('"', "");
  if (tagActual !== image.digest) {
    drifts.push(`image digest drift: ${name} expected=${image.digest} actual=${tagActual}`);
    continue;
  }
  let pull = null;
  if (prefetchImages) {
    pull = runWithFallback(ref => ["pull", ref], lockedRef, fallbackLockedRef, 120000);
    if (pull.result.error?.code === "ETIMEDOUT" || pull.result.status !== 0) {
      failures.push(`infrastructure.registry_unavailable: ${name} pull failed`);
      continue;
    }
  }
  const sources = [lockedInspect, tagInspect, pull].filter(Boolean);
  const fallbackReasons = [...new Set(sources.map(item => item.fallbackReason).filter(Boolean))];
  entries.push({
    name,
    canonicalRef: image.ref,
    approvedFallbackRef: image.fallbackRef || null,
    lockedDigest: image.digest,
    lockedDigestVerified: true,
    mutableTagDigest: tagActual,
    mutableTagMatchesLock: true,
    inspectSource: lockedInspect.source === tagInspect.source ? lockedInspect.source : "mixed",
    pullSource: pull?.source || null,
    fallbackReasons,
    pulled: prefetchImages,
  });
  if (fallbackReasons.length) {
    process.stdout.write(`preflight.registry_fallback_verified: ${name} reasons=${fallbackReasons.join(",")} digest=${image.digest}\n`);
  }
}
const lockHash = crypto.createHash("sha256").update(lockRaw).digest("hex");
const passed = failures.length === 0 && drifts.length === 0 && entries.length === Object.keys(lock.images || {}).length;
if (evidencePath) {
  fs.writeFileSync(evidencePath, `${JSON.stringify({
    schemaVersion: "runtime-rc-preflight@1",
    recordedAt: new Date().toISOString(),
    lockHash,
    prefetchImages,
    registryAttempts,
    entries,
    passed,
    errors: [...failures, ...drifts],
  }, null, 2)}\n`);
}
if (failures.length || drifts.length) {
  throw new Error([...failures, ...drifts].join("\n"));
}
NODE

printf 'Runtime RC preflight passed: %s\n' "${lock_file}"
