#!/usr/bin/env node
"use strict";

const crypto = require("crypto");
const fs = require("fs");
const http = require("http");
const path = require("path");

const WORKSPACE_ROOT = path.resolve(process.env.WORKSPACE_ROOT || "/workspace");
const CANDIDATES_ROOT = path.join(WORKSPACE_ROOT, "outputs", "candidates");
const HOST = process.env.CANDIDATE_PREVIEW_HOST || "0.0.0.0";
const PORT = Number(process.env.CANDIDATE_PREVIEW_PORT || 4321);

const CONTENT_TYPES = {
  ".css": "text/css; charset=utf-8",
  ".gif": "image/gif",
  ".html": "text/html; charset=utf-8",
  ".ico": "image/x-icon",
  ".jpeg": "image/jpeg",
  ".jpg": "image/jpeg",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".map": "application/json; charset=utf-8",
  ".png": "image/png",
  ".svg": "image/svg+xml; charset=utf-8",
  ".txt": "text/plain; charset=utf-8",
  ".webp": "image/webp",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
};

function safeCandidateRoot(buildId) {
  if (!/^build-[a-zA-Z0-9._-]+$/.test(buildId)) return null;
  return path.join(CANDIDATES_ROOT, buildId);
}

function safeFile(root, relative) {
  let decoded;
  try {
    decoded = decodeURIComponent(relative || "");
  } catch (_error) {
    return null;
  }
  const requested = path.resolve(root, decoded.replace(/^\/+/, ""));
  if (requested !== root && !requested.startsWith(`${root}${path.sep}`)) return null;
  const candidates = [requested];
  if (decoded === "" || decoded.endsWith("/")) candidates.push(path.join(requested, "index.html"));
  if (!path.extname(requested)) {
    candidates.push(`${requested}.html`, path.join(requested, "index.html"));
  }
  return candidates.find((candidate) => {
    try {
      return fs.statSync(candidate).isFile();
    } catch (_error) {
      return false;
    }
  });
}

function manifestHash(root) {
  const manifest = fs.readFileSync(
    path.join(root, ".anydesign-candidate-manifest.json"),
  );
  return crypto.createHash("sha256").update(manifest).digest("hex");
}

const server = http.createServer((request, response) => {
  if (request.url === "/healthz") {
    response.writeHead(200, { "content-type": "application/json" });
    response.end('{"ok":true}\n');
    return;
  }
  if (!request.method || !["GET", "HEAD"].includes(request.method)) {
    response.writeHead(405, { allow: "GET, HEAD" });
    response.end();
    return;
  }
  const url = new URL(request.url || "/", "http://candidate-preview.local");
  const match = url.pathname.match(/^\/candidates\/([^/]+)\/?(.*)$/);
  if (!match) {
    response.writeHead(404);
    response.end();
    return;
  }
  const root = safeCandidateRoot(match[1]);
  const file = root && safeFile(root, match[2]);
  if (!root || !file) {
    response.writeHead(404);
    response.end();
    return;
  }
  try {
    const bytes = fs.readFileSync(file);
    const contentType = CONTENT_TYPES[path.extname(file).toLowerCase()] || "application/octet-stream";
    response.writeHead(200, {
      "cache-control": contentType.startsWith("text/html")
        ? "no-store"
        : "public, max-age=31536000, immutable",
      "content-length": bytes.length,
      "content-type": contentType,
      "x-anydesign-candidate-manifest-hash": manifestHash(root),
      "x-content-type-options": "nosniff",
    });
    response.end(request.method === "HEAD" ? undefined : bytes);
  } catch (_error) {
    response.writeHead(404);
    response.end();
  }
});

server.listen(PORT, HOST, () => {
  console.log(`candidate preview listening on http://${HOST}:${PORT}`);
});
