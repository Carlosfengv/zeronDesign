#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
lock_file="${ROOT_DIR}/infra/agent-sandbox/images.lock.json"
cd "${ROOT_DIR}"

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

node - "${lock_file}" "${PREFLIGHT_PREFETCH_IMAGES:-1}" <<'NODE'
const fs = require("fs");
const { spawnSync } = require("child_process");
const lock = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
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
if (lock.schemaVersion !== "anydesign-runtime-images@1") throw new Error("invalid image lock schema");
for (const [name, image] of Object.entries(lock.images || {})) {
  if (typeof image.ref !== "string" || !/^sha256:[a-f0-9]{64}$/.test(image.digest || "")) {
    throw new Error(`invalid image lock entry: ${name}`);
  }
  const inspect = runDockerWithRetry(
    ["buildx", "imagetools", "inspect", image.ref, "--format", "{{json .Manifest.Digest}}"],
    45000,
  );
  if (inspect.error?.code === "ETIMEDOUT") throw new Error(`infrastructure.registry_unavailable: ${name} inspect timeout`);
  if (inspect.status !== 0) throw new Error(`infrastructure.registry_unavailable: ${name} ${inspect.stderr.trim()}`);
  const actual = inspect.stdout.trim().replaceAll('"', "");
  if (actual !== image.digest) throw new Error(`image digest drift: ${name} expected=${image.digest} actual=${actual}`);
  if (process.argv[3] === "1") {
    const pull = runDockerWithRetry(["pull", `${image.ref}@${image.digest}`], 120000);
    if (pull.error?.code === "ETIMEDOUT" || pull.status !== 0) {
      throw new Error(`infrastructure.registry_unavailable: ${name} pull failed`);
    }
  }
}
NODE

printf 'Runtime RC preflight passed: %s\n' "${lock_file}"
