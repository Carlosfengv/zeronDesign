"use strict";

function resultText(result) {
  return `${result?.stderr || ""}\n${result?.stdout || ""}`;
}

function registryFallbackReason(result) {
  if (result?.error?.code === "ETIMEDOUT") return "timeout";
  const text = resultText(result);
  if (/429|Too Many Requests|pull rate limit/i.test(text)) return "rate_limited";
  if (/\bEOF\b|connection reset|TLS handshake timeout/i.test(text)) return "transport_interrupted";
  return null;
}

module.exports = { registryFallbackReason };
