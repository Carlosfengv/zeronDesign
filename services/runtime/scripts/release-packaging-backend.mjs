#!/usr/bin/env node

import { createHash, randomBytes } from "node:crypto";
import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const protocolVersion = "release-packager-process@1";
const repoRoot = resolve(dirname(new URL(import.meta.url).pathname), "../../..");
const lock = JSON.parse(readFileSync(join(repoRoot, "infra/published-runtime/images.lock.json"), "utf8"));
const locks = lock.images;
const toolLocks = lock.tools;
const workRoot = resolve(process.env.ANYDESIGN_PACKAGER_ROOT || join(process.env.HOME, ".cache/anydesign/release-packager"));
const indexPath = join(workRoot, "digest-index.json");
const toolsRoot = resolve(process.env.ANYDESIGN_PACKAGER_TOOLS || "/opt/homebrew/bin");
const builderName = "anydesign-release-g4";

function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function digestFile(path) {
  return `sha256:${sha256File(path)}`;
}

function staticWebBaseImage(digest) {
  return `${locks.staticWebBase.localAcceptanceMirror}@${digest}`;
}

function run(program, args, options = {}) {
  const result = spawnSync(program, args, {
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
    ...options,
  });
  if (result.error) throw result.error;
  if (result.status !== 0 && !options.acceptFailure) {
    throw new Error(`${program} failed (${result.status}): ${(result.stderr || result.stdout).trim()}`);
  }
  return result;
}

function tool(name) {
  const path = join(toolsRoot, name);
  if (!existsSync(path)) throw new Error(`required trusted tool is unavailable: ${name}`);
  return path;
}

function verifyToolchain() {
  const syftVersion = run(tool("syft"), ["version", "-o", "json"]);
  if (JSON.parse(syftVersion.stdout).version !== toolLocks.syft.version) throw new Error("unexpected Syft version");
  if (!new RegExp(`Version:\\s*${toolLocks.trivy.version.replaceAll(".", "\\.")}\\b`).test(run(tool("trivy"), ["--version"]).stdout)) throw new Error("unexpected Trivy version");
  const cosignVersion = run(tool("cosign"), ["version", "--json"]);
  if (JSON.parse(cosignVersion.stdout).gitVersion !== `v${toolLocks.cosign.version}`) throw new Error("unexpected Cosign version");
}

function verifyBuilder() {
  const inspected = run("docker", ["buildx", "inspect", builderName]);
  const expectedImage = `registry-1.docker.io/moby/buildkit@${locks.buildkit.digest}`;
  if (!/^Driver:\s+docker-container$/m.test(inspected.stdout)
    || !/^Status:\s+running$/m.test(inspected.stdout)
    || !inspected.stdout.includes(`image=\"${expectedImage}\"`)) {
    throw new Error("trusted isolated BuildKit identity mismatch");
  }
}

function registryUrl(repository, releaseId) {
  const separator = repository.indexOf("/");
  if (separator < 1) throw new Error("registry repository must include a host and path");
  const host = repository.slice(0, separator);
  const path = repository.slice(separator + 1);
  const scheme = /^(localhost|127\.0\.0\.1)(:|$)/.test(host) ? "http" : "https";
  return `${scheme}://${host}/v2/${path}/${safeId(releaseId)}/manifests/latest`;
}

function registryDigestUrl(repository, releaseId, digest) {
  if (!/^sha256:[0-9a-f]{64}$/.test(digest)) throw new Error("invalid garbage collection digest");
  return registryUrl(repository, releaseId).replace(/\/manifests\/latest$/, `/manifests/${digest}`);
}

function builderRepository(repository) {
  return repository.replace(/^localhost:/, "host.docker.internal:").replace(/^127\.0\.0\.1:/, "host.docker.internal:");
}

function safeId(value) {
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(value)) throw new Error("unsafe release identifier");
  return value;
}

function releaseDir(releaseId) {
  return join(workRoot, safeId(releaseId));
}

function readIndex() {
  return existsSync(indexPath) ? JSON.parse(readFileSync(indexPath, "utf8")) : {};
}

function recordDigest(digest, value) {
  mkdirSync(workRoot, { recursive: true });
  const index = readIndex();
  index[digest] = value;
  writeFileSync(indexPath, `${JSON.stringify(index, null, 2)}\n`, { mode: 0o600 });
}

function indexed(digest) {
  const value = readIndex()[digest];
  if (!value) throw new Error(`image digest is not owned by this packager: ${digest}`);
  return value;
}

function build(request) {
  verifyBuilder();
  const root = releaseDir(request.releaseId);
  const context = join(root, "context");
  const publicRoot = join(context, "public");
  const metadataRoot = join(context, "metadata");
  mkdirSync(publicRoot, { recursive: true });
  mkdirSync(metadataRoot, { recursive: true });
  const artifactManifestPath = join(request.artifactRoot, ".anydesign-artifact-manifest.json");
  const artifactManifest = JSON.parse(readFileSync(artifactManifestPath, "utf8"));
  for (const entry of artifactManifest.files) {
    const source = resolve(request.artifactRoot, entry.path);
    const target = resolve(publicRoot, entry.path);
    if (!source.startsWith(`${resolve(request.artifactRoot)}/`) || !target.startsWith(`${resolve(publicRoot)}/`)) {
      throw new Error("artifact path escaped its trusted root");
    }
    mkdirSync(dirname(target), { recursive: true });
    copyFileSync(source, target);
  }
  copyFileSync(artifactManifestPath, join(metadataRoot, "artifact-manifest.json"));
  writeFileSync(join(metadataRoot, "runtime-manifest.json"), `${JSON.stringify(request.runtimeManifest)}\n`);
  writeFileSync(join(metadataRoot, "release-provenance.json"), `${JSON.stringify({
    schemaVersion: "release-provenance@1",
    releaseId: request.releaseId,
    artifactManifestHash: request.artifactManifestHash,
    runtimeManifestHash: request.runtimeManifestHash,
    baseImageDigest: request.baseImageDigest,
    packagerVersion: request.packagerVersion,
  })}\n`);
  copyFileSync(join(repoRoot, "infra/published-runtime/static-web/Dockerfile"), join(context, "Dockerfile"));
  copyFileSync(join(repoRoot, "infra/published-runtime/static-web/nginx.conf"), join(context, "nginx.conf"));
  const archive = join(root, "release.oci.tar");
  const buildMetadata = join(root, "build-metadata.json");
  const result = run("docker", [
    "buildx", "build", "--builder", builderName, "--platform", "linux/arm64",
    "--build-arg", `BASE_IMAGE=${staticWebBaseImage(request.baseImageDigest)}`,
    "--build-arg", `RELEASE_ID=${safeId(request.releaseId)}`,
    "--output", `type=oci,dest=${archive}`,
    "--metadata-file", buildMetadata,
    "--provenance=false", "--sbom=false", context,
  ], { acceptFailure: true });
  if (result.status !== 0) throw new Error(`docker buildx failed (${result.status}): ${result.stderr.trim()}`);
  const metadata = JSON.parse(readFileSync(buildMetadata, "utf8"));
  const digest = metadata["containerimage.digest"];
  if (!/^sha256:[0-9a-f]{64}$/.test(digest)) throw new Error("buildx did not return an image digest");
  recordDigest(digest, { root, repository: request.imageRepository, releaseId: request.releaseId });
  return {
    digest,
    layoutUri: `file://${archive}`,
    artifactManifestHash: request.artifactManifestHash,
    runtimeManifestHash: request.runtimeManifestHash,
  };
}

function registryDigest({ imageRepository, releaseId }) {
  const result = run("curl", [
    "--silent", "--show-error", "--dump-header", "-", "--output", "/dev/null",
    "--header", "Accept: application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json",
    registryUrl(imageRepository, releaseId),
  ], { acceptFailure: true });
  if (result.status !== 0) {
    if (/404 Not Found/i.test(result.stdout)) return null;
    throw new Error(`registry inspect failed (${result.status}): ${result.stderr.trim()}`);
  }
  if (/404 Not Found/i.test(result.stdout)) return null;
  const match = result.stdout.match(/^docker-content-digest:\s*(sha256:[0-9a-f]{64})\s*$/im);
  const digest = match?.[1];
  if (!/^sha256:[0-9a-f]{64}$/.test(digest)) throw new Error("registry returned an invalid digest");
  return digest;
}

function push({ request, image: built }) {
  verifyBuilder();
  const root = releaseDir(request.releaseId);
  const context = join(root, "context");
  const destination = `${builderRepository(request.imageRepository)}/${safeId(request.releaseId)}`;
  const metadataPath = join(root, "push-metadata.json");
  const result = run("docker", [
    "buildx", "build", "--builder", builderName, "--platform", "linux/arm64",
    "--build-arg", `BASE_IMAGE=${staticWebBaseImage(request.baseImageDigest)}`,
    "--build-arg", `RELEASE_ID=${safeId(request.releaseId)}`,
    "--output", `type=image,name=${destination},push=true,oci-mediatypes=true`,
    "--metadata-file", metadataPath,
    "--provenance=false", "--sbom=false", context,
  ], { acceptFailure: true });
  if (result.status !== 0) throw new Error(`registry push failed (${result.status}): ${result.stderr.trim()}`);
  const pushedMetadata = JSON.parse(readFileSync(metadataPath, "utf8"));
  if (pushedMetadata["containerimage.digest"] !== built.digest) {
    throw new Error("registry exporter digest differs from the trusted OCI build digest");
  }
  const digest = registryDigest({ imageRepository: request.imageRepository, releaseId: request.releaseId });
  if (digest !== built.digest) throw new Error("registry digest differs from the trusted build digest");
  recordDigest(digest, { root, repository: request.imageRepository, releaseId: request.releaseId });
  return digest;
}

function generateEvidence({ request, imageDigest }) {
  verifyToolchain();
  const { root } = indexed(imageDigest);
  const sbom = join(root, "sbom.spdx.json");
  const result = run(tool("syft"), ["scan", `oci-archive:${join(root, "release.oci.tar")}`, "-o", `spdx-json=${sbom}`], { acceptFailure: true });
  if (result.status !== 0) throw new Error(`SBOM generation failed (${result.status}): ${result.stderr.trim()}`);
  const provenance = join(root, "context/metadata/release-provenance.json");
  return { sbomDigest: digestFile(sbom), provenanceDigest: digestFile(provenance) };
}

function scan({ imageDigest, evidence, policyVersion }) {
  verifyToolchain();
  if (policyVersion !== "trivy-critical-secret-v1") throw new Error("unsupported scan policy");
  const { root } = indexed(imageDigest);
  const trivyCache = join(workRoot, "trivy-cache");
  const databaseMetadataPath = join(trivyCache, "db/metadata.json");
  if (!existsSync(databaseMetadataPath)) throw new Error("trusted Trivy database is unavailable");
  const databaseMetadata = JSON.parse(readFileSync(databaseMetadataPath, "utf8"));
  const updatedAt = Date.parse(databaseMetadata.UpdatedAt);
  const nextUpdate = Date.parse(databaseMetadata.NextUpdate);
  if (!Number.isFinite(updatedAt) || !Number.isFinite(nextUpdate) || updatedAt > Date.now() + 300_000 || nextUpdate <= Date.now()) {
    throw new Error("trusted Trivy database is stale or invalid");
  }
  if (digestFile(join(root, "sbom.spdx.json")) !== evidence.sbomDigest) throw new Error("SBOM evidence changed before scan");
  const ociLayout = join(root, "release.oci");
  rmSync(ociLayout, { recursive: true, force: true });
  mkdirSync(ociLayout, { recursive: true });
  run("tar", ["-xf", join(root, "release.oci.tar"), "-C", ociLayout]);
  const reportPath = join(root, "trivy.json");
  const result = run(tool("trivy"), [
    "image", "--input", ociLayout, "--scanners", "vuln,secret",
    "--cache-dir", trivyCache, "--skip-db-update",
    "--format", "json", "--output", reportPath, "--exit-code", "0",
  ], { acceptFailure: true });
  if (result.status !== 0) throw new Error(`image scan failed (${result.status}): ${result.stderr.trim()}`);
  const report = JSON.parse(readFileSync(reportPath, "utf8"));
  let critical = 0;
  let high = 0;
  let secrets = 0;
  for (const item of report.Results || []) {
    for (const vulnerability of item.Vulnerabilities || []) {
      if (vulnerability.Severity === "CRITICAL") critical += 1;
      if (vulnerability.Severity === "HIGH") high += 1;
    }
    secrets += (item.Secrets || []).length;
  }
  return {
    policyVersion,
    passed: critical === 0 && secrets === 0,
    criticalVulnerabilities: critical,
    highVulnerabilities: high,
    secretFindings: secrets,
    reportDigest: digestFile(reportPath),
  };
}

function sign({ imageDigest, provenanceDigest }) {
  verifyToolchain();
  const { root, repository, releaseId } = indexed(imageDigest);
  const provenance = join(root, "context/metadata/release-provenance.json");
  if (digestFile(provenance) !== provenanceDigest) throw new Error("provenance changed before signing");
  const keys = join(root, "keys");
  mkdirSync(keys, { recursive: true, mode: 0o700 });
  const passwordPath = join(keys, "password");
  if (!existsSync(passwordPath)) {
    writeFileSync(passwordPath, randomBytes(32).toString("hex"), { mode: 0o600 });
    const password = readFileSync(passwordPath, "utf8");
    const generated = run(tool("cosign"), ["generate-key-pair", "--output-key-prefix", join(keys, "cosign")], {
      acceptFailure: true,
      env: { ...process.env, COSIGN_PASSWORD: password },
    });
    if (generated.status !== 0) throw new Error(`cosign key generation failed (${generated.status}): ${generated.stderr.trim()}`);
    chmodSync(join(keys, "cosign.key"), 0o600);
  }
  const password = readFileSync(passwordPath, "utf8");
  const reference = `${repository}/${safeId(releaseId)}@${imageDigest}`;
  const signingConfig = join(keys, "local-signing-config.json");
  const configured = run(tool("cosign"), [
    "signing-config", "create", "--no-default-fulcio", "--no-default-oidc",
    "--no-default-rekor", "--no-default-tsa", "--out", signingConfig,
  ], { acceptFailure: true });
  if (configured.status !== 0) throw new Error(`local signing configuration failed (${configured.status}): ${configured.stderr.trim()}`);
  const signed = run(tool("cosign"), [
    "sign", "--key", join(keys, "cosign.key"), "--allow-insecure-registry",
    "--signing-config", signingConfig, "--yes", reference,
  ], { acceptFailure: true, env: { ...process.env, COSIGN_PASSWORD: password } });
  if (signed.status !== 0) throw new Error(`image signing failed (${signed.status}): ${signed.stderr.trim()}`);
  const verified = run(tool("cosign"), [
    "verify", "--key", join(keys, "cosign.pub"), "--allow-insecure-registry",
    "--insecure-ignore-tlog", "--output", "json", reference,
  ], { acceptFailure: true });
  if (verified.status !== 0) throw new Error(`image signature verification failed (${verified.status}): ${verified.stderr.trim()}`);
  const verificationPath = join(root, "cosign-verification.json");
  writeFileSync(verificationPath, verified.stdout);
  return {
    identity: `local-cosign-key:sha256:${sha256File(join(keys, "cosign.pub"))}`,
    signatureDigest: digestFile(verificationPath),
  };
}

function garbageCollect({ release, packaging }) {
  if (release.status !== "garbage_collectable" || packaging.status !== "failed") {
    throw new Error("only an authorized failed release can be garbage collected");
  }
  const digest = packaging.pushedImageDigest;
  if (release.runtimeImageDigest !== digest) throw new Error("garbage collection digest mismatch");
  const result = run("curl", [
    "--silent", "--show-error", "--dump-header", "-", "--output", "/dev/null",
    "--request", "DELETE", registryDigestUrl(packaging.registryRepository, release.id, digest),
  ], { acceptFailure: true });
  if (result.status !== 0 || !/HTTP\/\S+\s+(202|404)\b/i.test(result.stdout)) {
    throw new Error(`registry manifest deletion failed (${result.status}): ${(result.stderr || result.stdout).trim()}`);
  }
  const indexedRelease = indexed(digest);
  rmSync(indexedRelease.root, { recursive: true, force: true });
  const index = readIndex();
  delete index[digest];
  writeFileSync(indexPath, `${JSON.stringify(index, null, 2)}\n`, { mode: 0o600 });
  return { registryManifestDeleted: true, packagingEvidenceDeleted: true };
}

try {
  const request = JSON.parse(readFileSync(0, "utf8"));
  if (request.protocolVersion !== protocolVersion) throw new Error("unsupported helper protocol");
  const operations = { build, registryDigest, push, generateEvidence, scan, sign, garbageCollect };
  const operation = operations[request.operation];
  if (!operation) throw new Error("unsupported helper operation");
  const output = operation(request.input);
  process.stdout.write(JSON.stringify({ protocolVersion, output }));
} catch (error) {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  process.exit(1);
}
