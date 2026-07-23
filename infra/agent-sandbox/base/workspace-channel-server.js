#!/usr/bin/env node
"use strict";

const crypto = require("crypto");
const { spawn } = require("child_process");
const { once } = require("events");
const fs = require("fs");
const http = require("http");
const https = require("https");
const path = require("path");

const WORKSPACE_ROOT = path.resolve(process.env.WORKSPACE_ROOT || "/workspace");
const PORT = Number(process.env.WORKSPACE_CHANNEL_PORT || process.env.PORT || 80);
const HOST = process.env.WORKSPACE_CHANNEL_HOST || "0.0.0.0";
const AUTH_MODE = process.env.WORKSPACE_CHANNEL_AUTH_MODE || "disabled";
const TLS_MODE = process.env.WORKSPACE_CHANNEL_TLS_MODE || "debug-loopback";
const TLS_CA_FILE = process.env.WORKSPACE_CHANNEL_CA_FILE || "/tls/ca.crt";
const TLS_CERT_FILE = process.env.WORKSPACE_CHANNEL_CERT_FILE || "/tls/tls.crt";
const TLS_KEY_FILE = process.env.WORKSPACE_CHANNEL_KEY_FILE || "/tls/tls.key";
const EXPECTED_RUNTIME_SAN =
  process.env.WORKSPACE_CHANNEL_RUNTIME_SAN ||
  "spiffe://anydesign.local/ns/anydesign-runtime/sa/anydesign-runtime";
const PUBLIC_KEY_FILE =
  process.env.WORKSPACE_CHANNEL_PUBLIC_KEY_FILE ||
  "/var/run/anydesign/workspace-channel/public.der";
const PUBLIC_KEY_FILES = (
  process.env.WORKSPACE_CHANNEL_PUBLIC_KEY_FILES || PUBLIC_KEY_FILE
)
  .split(",")
  .map((file) => file.trim())
  .filter(Boolean);
const EXPECTED_ISSUER = "anydesign-runtime";
const EXPECTED_AUDIENCE = "workspace-channel";
const ALLOWED_OPERATIONS = new Set([
  "fs.read",
  "fs.write",
  "process.exec",
  "process.manage",
  "archive.export",
]);
const WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const SECRET_PATTERNS = [
  ".env",
  "kubeconfig",
  `${path.sep}.ssh${path.sep}`,
  "id_rsa",
  "id_ed25519",
  ".token",
  "credentials",
  "private_key",
];
const MANAGED_PREVIEW_SCRIPT = "/opt/anydesign/bootstrap/static-preview-server.js";
const PROCESS_LOG_LIMIT = 64 * 1024;
const PROCESS_EXIT_DRAIN_GRACE_MS = 250;
const TOKEN_REPLAY_LOG = path.join(WORKSPACE_ROOT, ".anydesign", "channel-jti-replay.jsonl");
const TOKEN_REPLAY_COMPACT_INTERVAL = 256;

let workspaceChannelPublicKeys;
const consumedTokenIds = new Map();
let consumedTokenWrites = 0;
const processLeases = new Map();

function loadConsumedTokenIds() {
  try {
    const now = Math.floor(Date.now() / 1000);
    for (const line of fs.readFileSync(TOKEN_REPLAY_LOG, "utf8").split("\n")) {
      if (!line.trim()) continue;
      const record = JSON.parse(line);
      if (/^[a-f0-9]{64}$/.test(record.hash) && Number.isInteger(record.exp) && record.exp > now) {
        consumedTokenIds.set(record.hash, record.exp);
      }
    }
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
}

function persistConsumedTokenId(hash, exp) {
  fs.mkdirSync(path.dirname(TOKEN_REPLAY_LOG), { recursive: true });
  fs.appendFileSync(TOKEN_REPLAY_LOG, `${JSON.stringify({ hash, exp })}\n`, {
    encoding: "utf8",
    mode: 0o600,
  });
  consumedTokenWrites += 1;
  if (consumedTokenWrites % TOKEN_REPLAY_COMPACT_INTERVAL === 0) {
    const compact = [...consumedTokenIds]
      .map(([recordHash, recordExp]) => JSON.stringify({ hash: recordHash, exp: recordExp }))
      .join("\n");
    const temporary = `${TOKEN_REPLAY_LOG}.${process.pid}.tmp`;
    fs.writeFileSync(temporary, compact ? `${compact}\n` : "", { mode: 0o600 });
    fs.renameSync(temporary, TOKEN_REPLAY_LOG);
  }
}

loadConsumedTokenIds();

function decodeBase64Url(value) {
  return Buffer.from(value, "base64url");
}

function authError(message, statusCode = 401) {
  const error = new Error(message);
  error.statusCode = statusCode;
  return error;
}

function publicKeys() {
  if (!workspaceChannelPublicKeys) {
    workspaceChannelPublicKeys = new Map();
    for (const file of PUBLIC_KEY_FILES) {
      const der = fs.readFileSync(file);
      const keyId = `ed25519-${crypto.createHash("sha256").update(der).digest("hex").slice(0, 16)}`;
      workspaceChannelPublicKeys.set(
        keyId,
        crypto.createPublicKey({ key: der, format: "der", type: "spki" }),
      );
    }
  }
  return workspaceChannelPublicKeys;
}

function publicKey(keyId) {
  if (typeof keyId !== "string" || !publicKeys().has(keyId)) {
    throw authError("unknown workspace channel signing key");
  }
  return publicKeys().get(keyId);
}

function authenticateUpgrade(request) {
  if (AUTH_MODE === "disabled") {
    return { operations: [...ALLOWED_OPERATIONS] };
  }
  if (AUTH_MODE !== "required") {
    throw authError("invalid workspace channel auth mode", 500);
  }
  const authorization = request.headers.authorization;
  if (typeof authorization !== "string" || !authorization.startsWith("Bearer ")) {
    throw authError("missing workspace channel bearer token");
  }
  const token = authorization.slice("Bearer ".length);
  const parts = token.split(".");
  if (parts.length !== 3) {
    throw authError("invalid workspace channel bearer token");
  }
  let header;
  let claims;
  try {
    header = JSON.parse(decodeBase64Url(parts[0]).toString("utf8"));
    claims = JSON.parse(decodeBase64Url(parts[1]).toString("utf8"));
  } catch (_error) {
    throw authError("invalid workspace channel bearer token");
  }
  if (
    header.alg !== "EdDSA" ||
    header.typ !== "JWT" ||
    typeof header.kid !== "string"
  ) {
    throw authError("unsupported workspace channel token algorithm");
  }
  const validSignature = crypto.verify(
    null,
    Buffer.from(`${parts[0]}.${parts[1]}`, "utf8"),
    publicKey(header.kid),
    decodeBase64Url(parts[2]),
  );
  if (!validSignature) {
    throw authError("invalid workspace channel token signature");
  }
  const now = Math.floor(Date.now() / 1000);
  if (
    claims.iss !== EXPECTED_ISSUER ||
    claims.aud !== EXPECTED_AUDIENCE ||
    !Number.isInteger(claims.exp) ||
    claims.exp <= now ||
    !Number.isInteger(claims.iat) ||
    claims.iat > now + 5
  ) {
    throw authError("invalid or expired workspace channel token claims");
  }
  if (
    typeof claims.sandboxBindingId !== "string" ||
    typeof claims.projectId !== "string" ||
    typeof claims.runId !== "string" ||
    claims.sandboxName !== process.env.POD_NAME ||
    claims.podUid !== process.env.POD_UID ||
    typeof claims.jti !== "string" ||
    claims.jti.length < 16 ||
    !Array.isArray(claims.operations) ||
    claims.operations.length === 0 ||
    claims.operations.some((operation) => !ALLOWED_OPERATIONS.has(operation))
  ) {
    throw authError("workspace channel token is not bound to this sandbox", 403);
  }
  const requestedOperation = request.headers["x-anydesign-workspace-operation"];
  if (
    typeof requestedOperation !== "string" ||
    !ALLOWED_OPERATIONS.has(requestedOperation) ||
    !claims.operations.includes(requestedOperation)
  ) {
    throw authError("workspace channel operation is not authorized", 403);
  }
  for (const [tokenIdHash, expiresAt] of consumedTokenIds) {
    if (expiresAt <= now) consumedTokenIds.delete(tokenIdHash);
  }
  const tokenIdHash = crypto.createHash("sha256").update(claims.jti).digest("hex");
  if (consumedTokenIds.has(tokenIdHash)) {
    throw authError("workspace channel token has already been consumed");
  }
  consumedTokenIds.set(tokenIdHash, claims.exp);
  persistConsumedTokenId(tokenIdHash, claims.exp);
  return claims;
}

function authenticateTlsPeer(request) {
  if (TLS_MODE === "debug-loopback") return null;
  if (TLS_MODE !== "required") {
    throw authError("invalid workspace channel TLS mode", 500);
  }
  if (!request.socket.authorized) {
    throw authError("workspace channel client certificate is not authorized");
  }
  const certificate = request.socket.getPeerCertificate(false);
  const alternativeNames = String(certificate.subjectaltname || "")
    .split(",")
    .map((value) => value.trim());
  if (!alternativeNames.includes(`URI:${EXPECTED_RUNTIME_SAN}`)) {
    throw authError("workspace channel client SPIFFE SAN mismatch", 403);
  }
  return String(certificate.fingerprint256 || "")
    .replaceAll(":", "")
    .toLowerCase()
    .slice(0, 12);
}

function requiredOperation(op) {
  switch (op) {
    case "fs.read":
    case "fs.readBytes":
    case "fs.list":
    case "fs.stat":
      return "fs.read";
    case "fs.write":
    case "fs.writeBytes":
    case "fs.removeFile":
    case "fs.removeDirAll":
    case "fs.copyDir":
    case "fs.rename":
      return "fs.write";
    case "process.exec":
      return "process.exec";
    case "process.start":
    case "process.status":
    case "process.stop":
      return "process.manage";
    case "archive.export":
      return "archive.export";
    default:
      return null;
  }
}

function authorizeOperation(claims, op) {
  const operation = requiredOperation(op);
  if (!operation || !claims.operations.includes(operation)) {
    throw authError("workspace channel operation is not authorized", 403);
  }
}

function workspaceRootReal() {
  return fs.realpathSync.native(WORKSPACE_ROOT);
}

function assertInsideWorkspace(realPath) {
  const root = workspaceRootReal();
  const relative = path.relative(root, realPath);
  if (relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative))) {
    return realPath;
  }
  throw new Error("path outside workspace");
}

function assertNotSecret(realPath) {
  if (SECRET_PATTERNS.some((pattern) => realPath.includes(pattern))) {
    throw new Error("secret path denied");
  }
  return realPath;
}

function resolveWorkspacePath(inputPath, mode) {
  if (typeof inputPath !== "string" || inputPath.length === 0) {
    throw new Error("path must be a non-empty string");
  }

  const normalized = path.posix.normalize(inputPath);
  if (normalized !== "/workspace" && !normalized.startsWith("/workspace/")) {
    throw new Error("path must be under /workspace");
  }

  const relative = path.posix.relative("/workspace", normalized);
  const candidate = path.resolve(WORKSPACE_ROOT, relative);

  if (mode === "create") {
    let existingAncestor = path.dirname(candidate);
    let ancestorReal;
    while (!ancestorReal) {
      try {
        ancestorReal = fs.realpathSync.native(existingAncestor);
      } catch (error) {
        if (!error || typeof error !== "object" || error.code !== "ENOENT") {
          throw error;
        }
        const parent = path.dirname(existingAncestor);
        if (parent === existingAncestor) {
          throw error;
        }
        existingAncestor = parent;
      }
    }
    assertInsideWorkspace(ancestorReal);
    const suffix = path.relative(existingAncestor, candidate);
    return assertNotSecret(assertInsideWorkspace(path.resolve(ancestorReal, suffix)));
  }

  const real = fs.realpathSync.native(candidate);
  assertInsideWorkspace(real);
  return assertNotSecret(real);
}

async function handleRequest(request) {
  const payload = request.payload || {};

  switch (request.op) {
    case "fs.read": {
      const file = resolveWorkspacePath(request.path, "existing");
      return { text: await fs.promises.readFile(file, "utf8") };
    }
    case "fs.readBytes": {
      const file = resolveWorkspacePath(request.path, "existing");
      return { base64: (await fs.promises.readFile(file)).toString("base64") };
    }
    case "fs.write": {
      if (typeof payload.text !== "string") {
        throw new Error("fs.write requires payload.text");
      }
      const file = resolveWorkspacePath(request.path, "create");
      await fs.promises.mkdir(path.dirname(file), { recursive: true });
      await fs.promises.writeFile(file, payload.text, "utf8");
      return { bytes: Buffer.byteLength(payload.text) };
    }
    case "fs.writeBytes": {
      if (typeof payload.base64 !== "string") {
        throw new Error("fs.writeBytes requires payload.base64");
      }
      const file = resolveWorkspacePath(request.path, "create");
      const bytes = Buffer.from(payload.base64, "base64");
      await fs.promises.mkdir(path.dirname(file), { recursive: true });
      await fs.promises.writeFile(file, bytes);
      return { bytes: bytes.length };
    }
    case "fs.list": {
      const dir = resolveWorkspacePath(request.path, "existing");
      const entries = await fs.promises.readdir(dir, { withFileTypes: true });
      return {
        entries: entries.map((entry) => ({
          name: entry.name,
          kind: entry.isDirectory() ? "dir" : "file",
        })),
      };
    }
    case "fs.stat": {
      const target = resolveWorkspacePath(request.path, "existing");
      const stat = await fs.promises.stat(target);
      return { kind: stat.isDirectory() ? "dir" : "file" };
    }
    case "fs.removeFile": {
      const file = resolveWorkspacePath(request.path, "existing");
      if (file === workspaceRootReal()) {
        throw new Error("cannot remove workspace root");
      }
      await fs.promises.unlink(file);
      return { deleted: true };
    }
    case "fs.removeDirAll": {
      const dir = resolveWorkspacePath(request.path, "existing");
      if (dir === workspaceRootReal()) {
        throw new Error("cannot remove workspace root");
      }
      await fs.promises.rm(dir, { recursive: true, force: false });
      return { deleted: true };
    }
    case "fs.copyDir": {
      if (typeof payload.to !== "string") {
        throw new Error("fs.copyDir requires payload.to");
      }
      const source = resolveWorkspacePath(request.path, "existing");
      const target = resolveWorkspacePath(payload.to, "create");
      const skipDirNames = Array.isArray(payload.skipDirNames)
        ? payload.skipDirNames.filter((item) => typeof item === "string")
        : [];
      await copyDir(source, target, skipDirNames);
      return { copied: true };
    }
    case "fs.rename": {
      if (typeof payload.to !== "string") {
        throw new Error("fs.rename requires payload.to");
      }
      const source = resolveWorkspacePath(request.path, "existing");
      const target = resolveWorkspacePath(payload.to, "create");
      await fs.promises.mkdir(path.dirname(target), { recursive: true });
      await fs.promises.rename(source, target);
      return { renamed: true };
    }
    case "process.exec": {
      if (!Array.isArray(payload.argv) || payload.argv.length === 0) {
        throw new Error("process.exec requires payload.argv");
      }
      if (!payload.argv.every((item) => typeof item === "string")) {
        throw new Error("process.exec argv must be a string array");
      }
      const cwd = resolveWorkspacePath(
        request.path || "/workspace/project",
        "existing",
      );
      const timeoutMs = Number(payload.timeoutMs || 60000);
      return await runProcess(payload.argv, cwd, timeoutMs);
    }
    case "process.start": {
      return await startManagedProcess(request, payload);
    }
    case "process.status": {
      return managedProcessStatus(payload.leaseId);
    }
    case "process.stop": {
      return await stopManagedProcess(payload.leaseId);
    }
    default:
      throw new Error(`unsupported workspace op: ${request.op}`);
  }
}

function validateProcessLeaseId(leaseId) {
  if (
    typeof leaseId !== "string" ||
    leaseId.length < 16 ||
    leaseId.length > 128 ||
    !/^[A-Za-z0-9_-]+$/.test(leaseId)
  ) {
    throw new Error("process leaseId is invalid");
  }
  return leaseId;
}

function boundedLog(previous, chunk) {
  const next = `${previous}${chunk.toString("utf8")}`;
  return next.length <= PROCESS_LOG_LIMIT
    ? next
    : next.slice(next.length - PROCESS_LOG_LIMIT);
}

function managedProcessStatus(leaseId) {
  const lease = processLeases.get(validateProcessLeaseId(leaseId));
  if (!lease) throw new Error("process lease not found");
  return {
    leaseId: lease.id,
    status: lease.status,
    pid: lease.child.pid || null,
    exitCode: lease.exitCode,
    stdout: lease.stdout,
    stderr: lease.stderr,
    startedAt: lease.startedAt,
    exitedAt: lease.exitedAt,
  };
}

async function startManagedProcess(request, payload) {
  const leaseId = validateProcessLeaseId(payload.leaseId);
  const existing = processLeases.get(leaseId);
  if (existing && !["stopped", "exited", "failed"].includes(existing.status)) {
    return managedProcessStatus(leaseId);
  }
  if (existing) {
    processLeases.delete(leaseId);
  }
  if (!Array.isArray(payload.argv) || !payload.argv.every((item) => typeof item === "string")) {
    throw new Error("process.start argv must be a string array");
  }
  const testProcessAllowed = process.env.WORKSPACE_CHANNEL_ALLOW_TEST_PROCESS === "1";
  const isManagedPreview =
    payload.argv.length === 2 &&
    payload.argv[0] === "node" &&
    payload.argv[1] === MANAGED_PREVIEW_SCRIPT;
  const isManagedDevPreview =
    payload.argv.length === 8 &&
    payload.argv[0] === "env" &&
    payload.argv[1] === `ANYDESIGN_PREVIEW_BASE_PATH=/previews/${leaseId}` &&
    payload.argv[2] === "npm" &&
    payload.argv[3] === "run" &&
    payload.argv[4] === "dev" &&
    payload.argv[5] === "--" &&
    payload.argv[6] === "--port" &&
    payload.argv[7] === "3000";
  const isTestProcess = testProcessAllowed && payload.argv[0] === "node";
  if (!isManagedPreview && !isManagedDevPreview && !isTestProcess) {
    throw new Error("process.start only allows the runtime managed preview server");
  }
  const cwd = resolveWorkspacePath(request.path || "/workspace", "existing");
  const child = spawn(payload.argv[0], payload.argv.slice(1), {
    cwd,
    // Static PreviewLease routing has a dedicated, Runtime-owned port. The
    // next-app sandbox keeps CANDIDATE_PREVIEW_PORT=3000 for managed Dev, so
    // inheriting that value here would make the static proxy's 4321 target a
    // permanent false-positive.
    env: isManagedPreview
      ? { ...process.env, CANDIDATE_PREVIEW_PORT: "4321" }
      : process.env,
    stdio: ["ignore", "pipe", "pipe"],
    // npm launches the framework Dev server as a child process. A dedicated
    // process group lets process.stop terminate the full tree so a restarted
    // lease cannot inherit an orphan that still owns the preview port.
    detached: true,
  });
  const lease = {
    id: leaseId,
    child,
    status: "running",
    exitCode: null,
    stdout: "",
    stderr: "",
    startedAt: new Date().toISOString(),
    exitedAt: null,
  };
  processLeases.set(leaseId, lease);
  child.stdout.on("data", (chunk) => {
    lease.stdout = boundedLog(lease.stdout, chunk);
  });
  child.stderr.on("data", (chunk) => {
    lease.stderr = boundedLog(lease.stderr, chunk);
  });
  child.on("error", (error) => {
    lease.status = "failed";
    lease.stderr = boundedLog(lease.stderr, Buffer.from(error.message));
    lease.exitedAt = new Date().toISOString();
  });
  child.on("close", (code) => {
    lease.status = lease.status === "stopping" ? "stopped" : code === 0 ? "exited" : "failed";
    lease.exitCode = code;
    lease.exitedAt = new Date().toISOString();
  });
  return managedProcessStatus(leaseId);
}

async function stopManagedProcess(leaseId) {
  const lease = processLeases.get(validateProcessLeaseId(leaseId));
  if (!lease) throw new Error("process lease not found");
  if (["stopped", "exited", "failed"].includes(lease.status)) {
    return managedProcessStatus(leaseId);
  }
  lease.status = "stopping";
  signalManagedProcessGroup(lease.child, "SIGTERM");
  await Promise.race([
    once(lease.child, "close"),
    new Promise((resolve) => setTimeout(resolve, 2000)),
  ]);
  if (lease.status === "stopping") {
    signalManagedProcessGroup(lease.child, "SIGKILL");
    lease.status = "stopped";
    lease.exitedAt = new Date().toISOString();
  }
  return managedProcessStatus(leaseId);
}

function signalManagedProcessGroup(child, signal) {
  try {
    process.kill(-child.pid, signal);
  } catch (error) {
    if (error.code !== "ESRCH") {
      child.kill(signal);
    }
  }
}

async function descendantProcessIds(rootPid) {
  return await new Promise((resolve) => {
    let output = "";
    const ps = spawn("ps", ["-Ao", "pid=,ppid="], {
      stdio: ["ignore", "pipe", "ignore"],
    });
    ps.stdout.on("data", (chunk) => {
      output += chunk.toString("utf8");
    });
    ps.on("error", () => resolve([]));
    ps.on("close", () => {
      const children = new Map();
      for (const line of output.split("\n")) {
        const [pidText, parentText] = line.trim().split(/\s+/);
        const pid = Number(pidText);
        const parent = Number(parentText);
        if (!Number.isInteger(pid) || !Number.isInteger(parent)) continue;
        const siblings = children.get(parent) || [];
        siblings.push(pid);
        children.set(parent, siblings);
      }
      const descendants = [];
      const pending = [...(children.get(rootPid) || [])];
      while (pending.length > 0) {
        const pid = pending.pop();
        descendants.push(pid);
        pending.push(...(children.get(pid) || []));
      }
      resolve(descendants);
    });
  });
}

function signalProcessIds(pids, signal) {
  for (const pid of pids.reverse()) {
    try {
      process.kill(pid, signal);
    } catch (error) {
      if (error.code !== "ESRCH") continue;
    }
  }
}

async function copyDir(source, target, skipDirNames) {
  const stat = await fs.promises.stat(source);
  if (!stat.isDirectory()) {
    throw new Error("fs.copyDir source must be a directory");
  }
  await fs.promises.mkdir(target, { recursive: true });
  const entries = await fs.promises.readdir(source, { withFileTypes: true });
  for (const entry of entries) {
    if (entry.isDirectory() && skipDirNames.includes(entry.name)) {
      continue;
    }
    const from = path.join(source, entry.name);
    const to = path.join(target, entry.name);
    if (entry.isDirectory()) {
      await copyDir(from, to, skipDirNames);
    } else if (entry.isFile()) {
      await fs.promises.copyFile(from, to);
    }
  }
}

function runProcess(argv, cwd, timeoutMs) {
  return new Promise((resolve, reject) => {
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    let exitCode = null;
    let settled = false;
    let exitDrainTimeout;
    let exitPoll;
    const child = spawn(argv[0], argv.slice(1), {
      cwd,
      env: process.env,
      stdio: ["ignore", "pipe", "pipe"],
      detached: true,
    });

    const finish = (callback) => {
      if (settled) return;
      settled = true;
      clearInterval(exitPoll);
      clearTimeout(exitDrainTimeout);
      callback();
    };
    const signalProcessGroup = (signal) => {
      try {
        process.kill(-child.pid, signal);
      } catch (error) {
        if (error.code !== "ESRCH") {
          child.kill(signal);
        }
      }
    };
    const result = () => ({
      status: exitCode,
      success: !timedOut && exitCode === 0,
      stdout,
      stderr: timedOut
        ? `${stderr}${stderr.endsWith("\n") || stderr.length === 0 ? "" : "\n"}process.exec timed out`
        : stderr,
    });

    const timeout = setTimeout(async () => {
      timedOut = true;
      const descendants = await descendantProcessIds(child.pid);
      const processTree = [child.pid, ...descendants];
      signalProcessIds([...processTree], "SIGTERM");
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 1000));
      signalProcessIds([...processTree], "SIGKILL");
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 50));
      finish(() => resolve(result()));
    }, timeoutMs);
    timeout.unref();

    const scheduleExitedCompletion = (code) => {
      exitCode = code;
      clearTimeout(timeout);
      if (exitDrainTimeout) return;
      exitDrainTimeout = setTimeout(() => {
        signalProcessGroup("SIGTERM");
        finish(() => resolve(result()));
      }, PROCESS_EXIT_DRAIN_GRACE_MS);
      exitDrainTimeout.unref();
    };

    // ChildProcess normally emits `exit` and then `close`, but npm lifecycle
    // process trees have shown an exited/reaped parent without either callback
    // completing this Promise. `exitCode` is maintained by Node independently
    // of those listeners, so polling it provides a bounded final fallback.
    exitPoll = setInterval(() => {
      if (!timedOut && child.exitCode !== null) {
        scheduleExitedCompletion(child.exitCode);
      }
    }, 50);
    exitPoll.unref();

    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString("utf8");
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString("utf8");
    });
    child.on("error", (error) => {
      clearTimeout(timeout);
      clearTimeout(exitDrainTimeout);
      finish(() => reject(error));
    });
    child.on("exit", (code) => {
      // `close` waits for every inherited stdio descriptor to close. npm
      // lifecycle descendants can keep those descriptors open after the npm
      // parent has already exited successfully, which otherwise leaves the
      // Runtime tool execution in-doubt until its transport deadline. Give
      // buffered output a short grace period, then return the authoritative
      // parent exit status and terminate any leaked process-group members.
      if (!timedOut) scheduleExitedCompletion(code);
    });
    child.on("close", (code) => {
      exitCode = code;
      if (!timedOut) {
        clearTimeout(timeout);
        clearTimeout(exitDrainTimeout);
        finish(() => resolve(result()));
      }
    });
  });
}

function websocketAcceptKey(key) {
  return crypto.createHash("sha1").update(`${key}${WS_GUID}`).digest("base64");
}

function encodeFramePayload(payload, opcode) {
  if (payload.length < 126) {
    return Buffer.concat([Buffer.from([0x80 | opcode, payload.length]), payload]);
  }
  if (payload.length <= 0xffff) {
    const header = Buffer.alloc(4);
    header[0] = 0x80 | opcode;
    header[1] = 126;
    header.writeUInt16BE(payload.length, 2);
    return Buffer.concat([header, payload]);
  }
  const header = Buffer.alloc(10);
  header[0] = 0x80 | opcode;
  header[1] = 127;
  header.writeBigUInt64BE(BigInt(payload.length), 2);
  return Buffer.concat([header, payload]);
}

function encodeFrame(text) {
  return encodeFramePayload(Buffer.from(text, "utf8"), 0x1);
}

function encodeBinaryFrame(bytes) {
  return encodeFramePayload(Buffer.from(bytes), 0x2);
}

async function writeSocket(socket, bytes) {
  if (!socket.write(bytes)) await once(socket, "drain");
}

function tryDecodeFrame(buffer) {
  if (buffer.length < 2) {
    return null;
  }

  const first = buffer[0];
  const second = buffer[1];
  const opcode = first & 0x0f;
  const masked = (second & 0x80) !== 0;
  let length = second & 0x7f;
  let offset = 2;

  if (length === 126) {
    if (buffer.length < offset + 2) return null;
    length = buffer.readUInt16BE(offset);
    offset += 2;
  } else if (length === 127) {
    if (buffer.length < offset + 8) return null;
    const bigLength = buffer.readBigUInt64BE(offset);
    if (bigLength > BigInt(Number.MAX_SAFE_INTEGER)) {
      throw new Error("websocket frame too large");
    }
    length = Number(bigLength);
    offset += 8;
  }

  const maskOffset = offset;
  if (masked) {
    offset += 4;
  }
  if (buffer.length < offset + length) {
    return null;
  }

  const payload = Buffer.from(buffer.subarray(offset, offset + length));
  if (masked) {
    const mask = buffer.subarray(maskOffset, maskOffset + 4);
    for (let i = 0; i < payload.length; i += 1) {
      payload[i] ^= mask[i % 4];
    }
  }

  return {
    opcode,
    text: payload.toString("utf8"),
    rest: buffer.subarray(offset + length),
  };
}

async function processFrame(socket, text, claims) {
  let response;
  try {
    const request = JSON.parse(text);
    authorizeOperation(claims, request.op);
    if (request.op === "archive.export") {
      await streamArchive(socket, request);
      return;
    }
    response = { ok: true, result: await handleRequest(request) };
  } catch (error) {
    response = {
      ok: false,
      error: error instanceof Error ? error.message : String(error),
      code:
        error && typeof error === "object" && typeof error.code === "string"
          ? error.code
          : undefined,
    };
  }
  await writeSocket(socket, encodeFrame(JSON.stringify(response)));
}

const EXPORT_CHUNK_BYTES = 64 * 1024;
const EXPORT_MAX_BYTES = 256 * 1024 * 1024;
const EXPORT_MAX_FILE_BYTES = 64 * 1024 * 1024;
const EXPORT_MAX_FILES = 20000;

async function fileSha256(file) {
  const digest = crypto.createHash("sha256");
  const stream = fs.createReadStream(file);
  for await (const chunk of stream) digest.update(chunk);
  return digest.digest("hex");
}

async function collectArchiveFiles(root, excludedFiles) {
  const files = [];
  const stack = [root];
  let totalBytes = 0;
  while (stack.length > 0) {
    const directory = stack.pop();
    const entries = await fs.promises.readdir(directory, { withFileTypes: true });
    entries.sort((left, right) =>
      Buffer.compare(Buffer.from(left.name), Buffer.from(right.name)),
    );
    for (const entry of entries.reverse()) {
      const file = path.join(directory, entry.name);
      const relative = path.relative(root, file).split(path.sep).join("/");
      if (entry.isSymbolicLink()) throw new Error("archive export rejects symbolic links");
      if (entry.isDirectory()) {
        stack.push(file);
        continue;
      }
      if (!entry.isFile() || excludedFiles.has(relative)) continue;
      const stat = await fs.promises.stat(file);
      if (
        stat.size > EXPORT_MAX_FILE_BYTES ||
        files.length >= EXPORT_MAX_FILES ||
        totalBytes + stat.size > EXPORT_MAX_BYTES
      ) {
        throw new Error("archive export exceeds configured limits");
      }
      totalBytes += stat.size;
      files.push({
        absolute: file,
        bytes: stat.size,
        path: relative,
        sha256: await fileSha256(file),
      });
    }
  }
  files.sort((left, right) =>
    Buffer.compare(Buffer.from(left.path), Buffer.from(right.path)),
  );
  return { files, totalBytes };
}

async function streamArchive(socket, request) {
  try {
    const root = resolveWorkspacePath(request.path, "existing");
    const stat = await fs.promises.stat(root);
    if (!stat.isDirectory()) throw new Error("archive.export source must be a directory");
    const excludedFiles = new Set(
      Array.isArray(request.payload?.excludedFiles)
        ? request.payload.excludedFiles.filter((item) => typeof item === "string")
        : [],
    );
    const { files, totalBytes } = await collectArchiveFiles(root, excludedFiles);
    const manifest = files.map((file) => ({
      bytes: file.bytes,
      path: file.path,
      sha256: file.sha256,
    }));
    const manifestHash = crypto
      .createHash("sha256")
      .update(JSON.stringify(manifest))
      .digest("hex");
    await writeSocket(
      socket,
      encodeFrame(
        JSON.stringify({ type: "archive.start", format: "anydesign-tree-stream@1" }),
      ),
    );
    for (const file of files) {
      await writeSocket(
        socket,
        encodeFrame(
          JSON.stringify({
            type: "archive.file",
            path: file.path,
            bytes: file.bytes,
            sha256: file.sha256,
          }),
        ),
      );
      const stream = fs.createReadStream(file.absolute, {
        highWaterMark: EXPORT_CHUNK_BYTES,
      });
      for await (const chunk of stream) {
        await writeSocket(socket, encodeBinaryFrame(chunk));
      }
    }
    await writeSocket(
      socket,
      encodeFrame(
        JSON.stringify({
          type: "archive.end",
          fileCount: files.length,
          totalBytes,
          manifestHash,
        }),
      ),
    );
  } catch (error) {
    await writeSocket(
      socket,
      encodeFrame(
        JSON.stringify({
          type: "archive.error",
          error: error instanceof Error ? error.message : String(error),
        }),
      ),
    );
  }
}

const requestHandler = (_request, response) => {
  response.writeHead(404);
  response.end("workspace channel websocket endpoint is /workspace\n");
};
const server =
  TLS_MODE === "required"
    ? https.createServer(
        {
          ca: fs.readFileSync(TLS_CA_FILE),
          cert: fs.readFileSync(TLS_CERT_FILE),
          key: fs.readFileSync(TLS_KEY_FILE),
          requestCert: true,
          rejectUnauthorized: true,
          minVersion: "TLSv1.3",
        },
        requestHandler,
      )
    : TLS_MODE === "debug-loopback"
      ? http.createServer(requestHandler)
      : (() => {
          throw new Error("invalid WORKSPACE_CHANNEL_TLS_MODE");
        })();

server.on("connection", (socket) => {
  socket.on("error", () => socket.destroy());
});
server.on("secureConnection", (socket) => {
  socket.on("error", () => socket.destroy());
});
server.on("tlsClientError", (_error, socket) => {
  if (!socket.destroyed) socket.destroy();
});

server.on("upgrade", (request, socket) => {
  if (!request.url || !request.url.startsWith("/workspace")) {
    socket.destroy();
    return;
  }

  const key = request.headers["sec-websocket-key"];
  if (typeof key !== "string") {
    socket.destroy();
    return;
  }

  let claims;
  try {
    const fingerprint = authenticateTlsPeer(request);
    claims = authenticateUpgrade(request);
    if (fingerprint) {
      console.log(`workspace channel mTLS client fingerprint=${fingerprint}`);
    }
  } catch (error) {
    const statusCode =
      error && typeof error === "object" && Number.isInteger(error.statusCode)
        ? error.statusCode
        : 401;
    socket.end(
      `HTTP/1.1 ${statusCode} ${statusCode === 403 ? "Forbidden" : statusCode === 500 ? "Internal Server Error" : "Unauthorized"}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n`,
    );
    return;
  }

  socket.write(
    [
      "HTTP/1.1 101 Switching Protocols",
      "Upgrade: websocket",
      "Connection: Upgrade",
      `Sec-WebSocket-Accept: ${websocketAcceptKey(key)}`,
      "",
      "",
    ].join("\r\n"),
  );

  let pending = Buffer.alloc(0);
  socket.on("data", (chunk) => {
    pending = Buffer.concat([pending, chunk]);
    try {
      let frame;
      while ((frame = tryDecodeFrame(pending))) {
        pending = frame.rest;
        if (frame.opcode === 0x8) {
          socket.end(Buffer.from([0x88, 0x00]));
          return;
        }
        if (frame.opcode === 0x1 || frame.opcode === 0x2) {
          processFrame(socket, frame.text, claims).catch(() => socket.destroy());
        }
      }
    } catch (error) {
      socket.write(
        encodeFrame(
          JSON.stringify({
            ok: false,
            error: error instanceof Error ? error.message : String(error),
          }),
        ),
      );
    }
  });
});

server.listen(PORT, HOST, () => {
  const address = server.address();
  const actualPort = typeof address === "object" && address ? address.port : PORT;
  const scheme = TLS_MODE === "required" ? "wss" : "ws";
  console.log(`workspace channel listening on ${scheme}://${HOST}:${actualPort}/workspace`);
});
