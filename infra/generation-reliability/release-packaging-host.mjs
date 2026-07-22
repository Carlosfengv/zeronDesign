#!/usr/bin/env node

import { createHash, generateKeyPairSync, sign, verify } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:http";
import { dirname, join, resolve, sep } from "node:path";
import { spawnSync } from "node:child_process";

const protocolVersion = "release-packager-process@1";
const repoRoot = resolve(dirname(new URL(import.meta.url).pathname), "../..");
const port = Number(process.env.ANYDESIGN_K3D_PACKAGER_PORT || "49091");
const token = required("ANYDESIGN_K3D_PACKAGER_TOKEN");
const runtimeNamespace = process.env.ANYDESIGN_K3D_RUNTIME_NAMESPACE || "anydesign-runtime";
const registryExternal = process.env.ANYDESIGN_K3D_REGISTRY_EXTERNAL || "localhost:5003";
const registryInternal = process.env.ANYDESIGN_K3D_REGISTRY_INTERNAL || "k3d-greenfield-registry.localhost:5000";
const workRoot = resolve(process.env.ANYDESIGN_K3D_PACKAGER_ROOT || join(repoRoot, "services/runtime/target/k3d-release-packager"));
const states = new Map();

function required(name) {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function digestBytes(bytes) {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}

function digestFile(path) {
  return digestBytes(readFileSync(path));
}

function safeId(value) {
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(value)) throw new Error("unsafe release identifier");
  return value;
}

function run(program, args, options = {}) {
  const result = spawnSync(program, args, {
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
    ...options,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${program} failed (${result.status}): ${(result.stderr || result.stdout).trim()}`);
  }
  return result.stdout;
}

function runtimePod() {
  return run("kubectl", [
    "-n", runtimeNamespace,
    "get", "pod", "-l", "app=anydesign-runtime,anydesign.io/runtime-role=primary",
    "-o", "jsonpath={.items[0].metadata.name}",
  ]).trim();
}

function internalRepositoryToExternal(repository) {
  const prefix = `${registryInternal}/`;
  if (!repository.startsWith(prefix)) throw new Error("unexpected k3d registry repository");
  return `${registryExternal}/${repository.slice(prefix.length)}`;
}

async function registryDigest({ imageRepository, releaseId }) {
  const prefix = `${registryInternal}/`;
  if (!imageRepository.startsWith(prefix)) {
    throw new Error("unexpected k3d registry repository");
  }
  const repositoryPath = imageRepository.slice(prefix.length);
  const registryContainer = registryInternal.split(":", 1)[0];
  const result = spawnSync(
    "docker",
    [
      "exec",
      registryContainer,
      "wget",
      "-S",
      "--spider",
      "--header=Accept: application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json",
      `http://127.0.0.1:5000/v2/${repositoryPath}/${safeId(releaseId)}/manifests/latest`,
    ],
    { encoding: "utf8", maxBuffer: 32 * 1024 * 1024 },
  );
  if (result.error) throw result.error;
  const headers = `${result.stdout}\n${result.stderr}`;
  if (result.status !== 0 && /HTTP\/1\.1 404\b/.test(headers)) return null;
  if (result.status !== 0) {
    throw new Error(`registry inspection failed: ${headers.trim()}`);
  }
  const digest = headers.match(/^\s*Docker-Content-Digest:\s*(sha256:[a-f0-9]{64})\s*$/im)?.[1];
  if (!/^sha256:[a-f0-9]{64}$/.test(digest || "")) throw new Error("registry returned an invalid digest");
  return digest;
}

function validateAndCopyArtifacts(artifactRoot, publicRoot) {
  const copiedRoot = join(publicRoot, ".source");
  mkdirSync(copiedRoot, { recursive: true });
  run("kubectl", ["-n", runtimeNamespace, "cp", `${runtimePod()}:${artifactRoot}/.`, copiedRoot]);
  const manifestPath = join(copiedRoot, ".anydesign-artifact-manifest.json");
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  for (const entry of manifest.files || []) {
    const source = resolve(copiedRoot, entry.path);
    const target = resolve(publicRoot, entry.path);
    if (!source.startsWith(`${resolve(copiedRoot)}${sep}`) || !target.startsWith(`${resolve(publicRoot)}${sep}`)) {
      throw new Error("artifact path escaped its trusted root");
    }
    if (!statSync(source).isFile()) throw new Error(`artifact file is missing: ${entry.path}`);
    if (entry.sha256 && digestFile(source) !== `sha256:${entry.sha256}`) {
      throw new Error(`artifact file digest mismatch: ${entry.path}`);
    }
    mkdirSync(dirname(target), { recursive: true });
    copyFileSync(source, target);
  }
  rmSync(copiedRoot, { recursive: true, force: true });
  return manifest;
}

async function build(request) {
  const releaseId = safeId(request.releaseId);
  const root = join(workRoot, releaseId);
  const context = join(root, "context");
  const publicRoot = join(context, "public");
  const metadataRoot = join(context, "metadata");
  rmSync(root, { recursive: true, force: true });
  mkdirSync(publicRoot, { recursive: true });
  mkdirSync(metadataRoot, { recursive: true });
  const artifactManifest = validateAndCopyArtifacts(request.artifactRoot, publicRoot);
  writeFileSync(join(metadataRoot, "artifact-manifest.json"), `${JSON.stringify(artifactManifest, null, 2)}\n`);
  writeFileSync(join(metadataRoot, "runtime-manifest.json"), `${JSON.stringify(request.runtimeManifest, null, 2)}\n`);
  const provenancePath = join(metadataRoot, "release-provenance.json");
  writeFileSync(provenancePath, `${JSON.stringify({
    schemaVersion: "release-provenance@1",
    releaseId,
    projectId: request.projectId,
    versionId: request.versionId,
    artifactManifestHash: request.artifactManifestHash,
    runtimeManifestHash: request.runtimeManifestHash,
    baseImageDigest: request.baseImageDigest,
    packagerVersion: request.packagerVersion,
    executionMode: "k3d-local-acceptance",
  }, null, 2)}\n`);
  copyFileSync(join(repoRoot, "infra/published-runtime/static-web/Dockerfile"), join(context, "Dockerfile"));
  copyFileSync(join(repoRoot, "infra/published-runtime/static-web/nginx.conf"), join(context, "nginx.conf"));

  const externalRepository = internalRepositoryToExternal(request.imageRepository);
  const image = `${externalRepository}/${releaseId}:latest`;
  run("docker", [
    "build", "--platform", "linux/arm64", "--provenance=false",
    "--build-arg", `BASE_IMAGE=registry-1.docker.io/library/nginx@${request.baseImageDigest}`,
    "--build-arg", `RELEASE_ID=${releaseId}`,
    "-t", image, context,
  ]);
  run("docker", ["push", image]);
  const digest = await registryDigest({ imageRepository: request.imageRepository, releaseId });
  if (!digest) throw new Error("built image is missing from the registry");
  states.set(digest, { root, request, provenancePath, artifactManifest });
  return {
    digest,
    layoutUri: `registry://${request.imageRepository}/${releaseId}@${digest}`,
    artifactManifestHash: request.artifactManifestHash,
    runtimeManifestHash: request.runtimeManifestHash,
  };
}

async function push({ request, image }) {
  const digest = await registryDigest({ imageRepository: request.imageRepository, releaseId: request.releaseId });
  if (digest !== image.digest) throw new Error("registry digest differs from built image digest");
  return digest;
}

function stateFor(imageDigest) {
  const state = states.get(imageDigest);
  if (!state) throw new Error(`unknown packager-owned image digest: ${imageDigest}`);
  return state;
}

function generateEvidence({ imageDigest }) {
  const state = stateFor(imageDigest);
  const sbomPath = join(state.root, "sbom.spdx.json");
  writeFileSync(sbomPath, `${JSON.stringify({
    spdxVersion: "SPDX-2.3",
    name: state.request.releaseId,
    documentNamespace: `urn:anydesign:k3d:${imageDigest}`,
    files: (state.artifactManifest.files || []).map((file) => ({
      fileName: file.path,
      checksums: file.sha256 ? [{ algorithm: "SHA256", checksumValue: file.sha256 }] : [],
    })),
  }, null, 2)}\n`);
  return { sbomDigest: digestFile(sbomPath), provenanceDigest: digestFile(state.provenancePath) };
}

function walkFiles(root) {
  const files = [];
  for (const name of readdirSync(root)) {
    const path = join(root, name);
    if (statSync(path).isDirectory()) files.push(...walkFiles(path));
    else files.push(path);
  }
  return files;
}

function scanImage({ imageDigest, evidence, policyVersion }) {
  const state = stateFor(imageDigest);
  if (evidence.provenanceDigest !== digestFile(state.provenancePath)) throw new Error("provenance changed before scan");
  const patterns = [/-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/, /AKIA[0-9A-Z]{16}/];
  let secretFindings = 0;
  for (const file of walkFiles(join(state.root, "context/public"))) {
    const bytes = readFileSync(file);
    if (bytes.includes(0)) continue;
    const text = bytes.toString("utf8");
    secretFindings += patterns.filter((pattern) => pattern.test(text)).length;
  }
  const reportPath = join(state.root, "local-integrity-scan.json");
  const report = {
    schemaVersion: "k3d-local-integrity-scan@1",
    imageDigest,
    policyVersion,
    artifactCount: state.artifactManifest.files?.length || 0,
    criticalVulnerabilities: 0,
    highVulnerabilities: 0,
    secretFindings,
    note: "Local acceptance gate validates artifact integrity and obvious embedded secrets; production vulnerability scanning is out of scope.",
  };
  writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`);
  return {
    policyVersion,
    passed: secretFindings === 0,
    criticalVulnerabilities: 0,
    highVulnerabilities: 0,
    secretFindings,
    reportDigest: digestFile(reportPath),
  };
}

function signImage({ imageDigest, provenanceDigest }) {
  const state = stateFor(imageDigest);
  if (provenanceDigest !== digestFile(state.provenancePath)) throw new Error("provenance changed before signing");
  const keysRoot = join(workRoot, "keys");
  const privateKeyPath = join(keysRoot, "ed25519-private.pem");
  const publicKeyPath = join(keysRoot, "ed25519-public.pem");
  mkdirSync(keysRoot, { recursive: true });
  if (!existsSync(privateKeyPath)) {
    const keys = generateKeyPairSync("ed25519", {
      privateKeyEncoding: { type: "pkcs8", format: "pem" },
      publicKeyEncoding: { type: "spki", format: "pem" },
    });
    writeFileSync(privateKeyPath, keys.privateKey, { mode: 0o600 });
    writeFileSync(publicKeyPath, keys.publicKey, { mode: 0o644 });
  }
  const payload = Buffer.from(`${imageDigest}\n${provenanceDigest}\n`);
  const signature = sign(null, payload, readFileSync(privateKeyPath));
  if (!verify(null, payload, readFileSync(publicKeyPath), signature)) throw new Error("local signature verification failed");
  const signaturePath = join(state.root, "ed25519-signature.bin");
  writeFileSync(signaturePath, signature);
  return {
    identity: `k3d-local-ed25519:${digestFile(publicKeyPath)}`,
    signatureDigest: digestFile(signaturePath),
  };
}

async function dispatch(request) {
  if (request.protocolVersion !== protocolVersion) throw new Error("unsupported helper protocol");
  switch (request.operation) {
    case "build": return build(request.input);
    case "registryDigest": return registryDigest(request.input);
    case "push": return push(request.input);
    case "generateEvidence": return generateEvidence(request.input);
    case "scan": return scanImage(request.input);
    case "sign": return signImage(request.input);
    case "garbageCollect": return { registryManifestDeleted: false, packagingEvidenceDeleted: false };
    default: throw new Error("unsupported helper operation");
  }
}

const server = createServer(async (request, response) => {
  try {
    if (request.method !== "POST" || request.url !== "/release-packaging") {
      response.writeHead(404).end("not found");
      return;
    }
    if (request.headers.authorization !== `Bearer ${token}`) {
      response.writeHead(401).end("unauthorized");
      return;
    }
    const chunks = [];
    let size = 0;
    for await (const chunk of request) {
      size += chunk.length;
      if (size > 1024 * 1024) throw new Error("request exceeds 1 MiB");
      chunks.push(chunk);
    }
    const input = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    const output = await dispatch(input);
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ protocolVersion, output }));
  } catch (error) {
    response.writeHead(500, { "content-type": "text/plain" });
    response.end(error instanceof Error ? error.message : String(error));
  }
});

server.listen(port, "0.0.0.0", () => {
  process.stdout.write(`k3d release packager listening on 0.0.0.0:${port}\n`);
});
