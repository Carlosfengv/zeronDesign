#!/usr/bin/env node

import assert from "node:assert/strict";
import policy from "./preflight-registry-policy.cjs";

const { registryFallbackReason } = policy;

assert.equal(registryFallbackReason({ error: { code: "ETIMEDOUT" } }), "timeout");
assert.equal(registryFallbackReason({ status: 1, stderr: "429 Too Many Requests" }), "rate_limited");
assert.equal(registryFallbackReason({ status: 1, stderr: "failed to fetch anonymous token: EOF" }), "transport_interrupted");
assert.equal(registryFallbackReason({ status: 1, stderr: "unexpected EOF" }), "transport_interrupted");
assert.equal(registryFallbackReason({ status: 1, stderr: "connection reset by peer" }), "transport_interrupted");
assert.equal(registryFallbackReason({ status: 1, stderr: "TLS handshake timeout" }), "transport_interrupted");
assert.equal(registryFallbackReason({ status: 1, stderr: "manifest unknown" }), null);
assert.equal(registryFallbackReason({ status: 1, stderr: "digest mismatch" }), null);
assert.equal(registryFallbackReason({ status: 1, stderr: "denied" }), null);

process.stdout.write("runtime RC preflight registry policy tests passed\n");
