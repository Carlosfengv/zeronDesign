#!/usr/bin/env node
"use strict";

const crypto = require("crypto");
const { spawn } = require("child_process");
const fs = require("fs");
const http = require("http");
const path = require("path");

const WORKSPACE_ROOT = path.resolve(process.env.WORKSPACE_ROOT || "/workspace");
const PORT = Number(process.env.WORKSPACE_CHANNEL_PORT || process.env.PORT || 80);
const HOST = process.env.WORKSPACE_CHANNEL_HOST || "0.0.0.0";
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
    const parentReal = fs.realpathSync.native(path.dirname(candidate));
    assertInsideWorkspace(parentReal);
    return assertNotSecret(path.join(parentReal, path.basename(candidate)));
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
    case "fs.write": {
      if (typeof payload.text !== "string") {
        throw new Error("fs.write requires payload.text");
      }
      const file = resolveWorkspacePath(request.path, "create");
      await fs.promises.writeFile(file, payload.text, "utf8");
      return { bytes: Buffer.byteLength(payload.text) };
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
    default:
      throw new Error(`unsupported workspace op: ${request.op}`);
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
    const child = spawn(argv[0], argv.slice(1), {
      cwd,
      env: process.env,
      stdio: ["ignore", "pipe", "pipe"],
    });

    const timeout = setTimeout(() => {
      timedOut = true;
      child.kill("SIGTERM");
      setTimeout(() => {
        if (!child.killed) {
          child.kill("SIGKILL");
        }
      }, 1000).unref();
    }, timeoutMs);
    timeout.unref();

    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString("utf8");
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString("utf8");
    });
    child.on("error", (error) => {
      clearTimeout(timeout);
      reject(error);
    });
    child.on("close", (code) => {
      clearTimeout(timeout);
      resolve({
        status: code,
        success: !timedOut && code === 0,
        stdout,
        stderr: timedOut
          ? `${stderr}${stderr.endsWith("\n") || stderr.length === 0 ? "" : "\n"}process.exec timed out`
          : stderr,
      });
    });
  });
}

function websocketAcceptKey(key) {
  return crypto.createHash("sha1").update(`${key}${WS_GUID}`).digest("base64");
}

function encodeFrame(text) {
  const payload = Buffer.from(text, "utf8");
  if (payload.length < 126) {
    return Buffer.concat([Buffer.from([0x81, payload.length]), payload]);
  }
  if (payload.length <= 0xffff) {
    const header = Buffer.alloc(4);
    header[0] = 0x81;
    header[1] = 126;
    header.writeUInt16BE(payload.length, 2);
    return Buffer.concat([header, payload]);
  }
  const header = Buffer.alloc(10);
  header[0] = 0x81;
  header[1] = 127;
  header.writeBigUInt64BE(BigInt(payload.length), 2);
  return Buffer.concat([header, payload]);
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

async function processFrame(socket, text) {
  let response;
  try {
    const request = JSON.parse(text);
    response = { ok: true, result: await handleRequest(request) };
  } catch (error) {
    response = {
      ok: false,
      error: error instanceof Error ? error.message : String(error),
    };
  }
  socket.write(encodeFrame(JSON.stringify(response)));
}

const server = http.createServer((_request, response) => {
  response.writeHead(404);
  response.end("workspace channel websocket endpoint is /workspace\n");
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
          processFrame(socket, frame.text);
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
  console.log(`workspace channel listening on ws://${HOST}:${actualPort}/workspace`);
});
