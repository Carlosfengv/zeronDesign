#!/usr/bin/env node
"use strict";

const crypto = require("crypto");
const fs = require("fs");
const http = require("http");
const path = require("path");

const WORKSPACE_ROOT = path.resolve(process.env.WORKSPACE_ROOT || "/workspace");
const CANDIDATES_ROOT = path.join(WORKSPACE_ROOT, "outputs", "candidates");
const FIXED_BUILD_ID = process.env.CANDIDATE_BUILD_ID || null;
const HOST = process.env.CANDIDATE_PREVIEW_HOST || "0.0.0.0";
const PORT = Number(process.env.CANDIDATE_PREVIEW_PORT || 4321);
const CANDIDATE_MANIFEST_FILE = ".anydesign-candidate-manifest.json";
const ROUTE_MANIFEST_FILE = ".anydesign-artifact-routes.json";
const CONTENT_TYPES = {
  ".css": "text/css; charset=utf-8",
  ".gif": "image/gif",
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

function sha256(bytes) {
  return crypto.createHash("sha256").update(bytes).digest("hex");
}

function safeCandidateRoot(buildId) {
  if (!/^build-[a-zA-Z0-9._-]+$/.test(buildId)) return null;
  const root = path.join(CANDIDATES_ROOT, buildId);
  try {
    const canonicalCandidates = fs.realpathSync(CANDIDATES_ROOT);
    const canonicalRoot = fs.realpathSync(root);
    return canonicalRoot.startsWith(`${canonicalCandidates}${path.sep}`) ? canonicalRoot : null;
  } catch (_error) {
    return null;
  }
}

function parseRequest(requestUrl) {
  const url = new URL(requestUrl || "/", "http://candidate-preview.local");
  if (FIXED_BUILD_ID) return { buildId: FIXED_BUILD_ID, route: url.pathname };
  const match = url.pathname.match(/^\/candidates\/([^/]+)(\/.*)?$/);
  if (!match) return null;
  return { buildId: match[1], route: match[2] || "/" };
}

function readRegularFileNoSymlink(root, relative) {
  let cursor = root;
  for (const segment of relative.split("/")) {
    cursor = path.join(cursor, segment);
    const stat = fs.lstatSync(cursor);
    if (stat.isSymbolicLink()) throw new Error("candidate symbolic links are forbidden");
  }
  if (!fs.statSync(cursor).isFile()) throw new Error("candidate target is not a file");
  return fs.readFileSync(cursor);
}

function readBoundManifests(root) {
  const candidateBytes = readRegularFileNoSymlink(root, CANDIDATE_MANIFEST_FILE);
  const candidate = JSON.parse(candidateBytes.toString("utf8"));
  if (candidate.schemaVersion !== "candidate-manifest@1") throw new Error("candidate manifest schema mismatch");
  if (candidate.artifactRouteManifestPath !== ROUTE_MANIFEST_FILE) throw new Error("route manifest path mismatch");
  const files = new Map(candidate.files.map((entry) => [entry.path, entry]));
  const routeEntry = files.get(ROUTE_MANIFEST_FILE);
  if (!routeEntry || routeEntry.sha256 !== candidate.artifactRouteManifestHash) throw new Error("route manifest candidate binding mismatch");
  const routeBytes = readRegularFileNoSymlink(root, ROUTE_MANIFEST_FILE);
  if (sha256(routeBytes) !== candidate.artifactRouteManifestHash) throw new Error("route manifest integrity mismatch");
  const routes = JSON.parse(routeBytes.toString("utf8"));
  if (routes.schemaVersion !== "artifact-route-manifest@1" || routes.buildId !== candidate.buildId) throw new Error("route manifest identity mismatch");
  if (!routes.routes?.[routes.entryRoute]) throw new Error("route manifest entry is missing");
  return { candidateBytes, files, routes };
}

function normalizeAssetPath(route) {
  let decoded;
  try {
    decoded = decodeURIComponent(route);
  } catch (_error) {
    return null;
  }
  if (decoded.includes("\\") || decoded.includes("\0") || decoded.includes("//")) return null;
  const relative = decoded.replace(/^\/+/, "");
  if (!relative || relative.split("/").some((segment) => !segment || segment === "." || segment === "..")) return null;
  return relative;
}

function resolveTarget(route, manifests) {
  const canonical = manifests.routes.aliases?.[route] || route;
  const routeTarget = manifests.routes.routes?.[canonical];
  if (routeTarget) return routeTarget;
  const assetPath = normalizeAssetPath(route);
  const asset = assetPath && manifests.files.get(assetPath);
  if (!asset || assetPath === ROUTE_MANIFEST_FILE || assetPath === CANDIDATE_MANIFEST_FILE) return null;
  if (asset.contentType?.startsWith("text/html") || assetPath.endsWith(".html")) return null;
  return {
    file: assetPath,
    sha256: asset.sha256,
    contentType: CONTENT_TYPES[path.extname(assetPath).toLowerCase()] || "application/octet-stream",
  };
}

function readVerifiedFile(root, target, candidateEntry) {
  if (!candidateEntry || candidateEntry.sha256 !== target.sha256) throw new Error("route target candidate binding mismatch");
  const requested = path.join(root, target.file);
  let cursor = root;
  for (const segment of target.file.split("/")) {
    cursor = path.join(cursor, segment);
    if (fs.lstatSync(cursor).isSymbolicLink()) throw new Error("candidate symbolic links are forbidden");
  }
  const canonical = fs.realpathSync(requested);
  if (!canonical.startsWith(`${root}${path.sep}`) || !fs.statSync(canonical).isFile()) throw new Error("artifact target escapes candidate root");
  const bytes = fs.readFileSync(canonical);
  if (bytes.length !== candidateEntry.bytes || sha256(bytes) !== target.sha256) throw new Error("artifact target integrity mismatch");
  return bytes;
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
  const parsed = parseRequest(request.url);
  const root = parsed && safeCandidateRoot(parsed.buildId);
  if (!root) {
    response.writeHead(404);
    response.end();
    return;
  }
  try {
    const manifests = readBoundManifests(root);
    const target = resolveTarget(parsed.route, manifests);
    if (!target) {
      response.writeHead(404, { "x-content-type-options": "nosniff" });
      response.end();
      return;
    }
    const bytes = readVerifiedFile(root, target, manifests.files.get(target.file));
    const contentType = target.contentType || "application/octet-stream";
    response.writeHead(200, {
      "cache-control": contentType.startsWith("text/html") ? "no-store" : "public, max-age=31536000, immutable",
      "content-length": bytes.length,
      "content-type": contentType,
      "x-anydesign-artifact-path": target.file,
      "x-anydesign-artifact-sha256": target.sha256,
      "x-anydesign-candidate-manifest-hash": sha256(manifests.candidateBytes),
      "x-content-type-options": "nosniff",
    });
    response.end(request.method === "HEAD" ? undefined : bytes);
  } catch (_error) {
    response.writeHead(409, { "content-type": "application/json", "x-content-type-options": "nosniff" });
    response.end('{"error":"candidate_manifest_conflict"}\n');
  }
});

server.listen(PORT, HOST, () => {
  console.log(`candidate preview listening on http://${HOST}:${PORT}`);
});
