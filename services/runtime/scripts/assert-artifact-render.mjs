#!/usr/bin/env node

import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const COLLECTOR = fileURLToPath(new URL("./collect-computed-styles.mjs", import.meta.url));

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function firstValue(result, ruleId) {
  return result?.results?.[ruleId]?.values?.[0];
}

export function validateComputedStyleResult(result) {
  if (result?.ok !== true) throw new Error(result?.error || "computed-style collector failed");
  const display = firstValue(result, "artifact-body-display");
  const color = firstValue(result, "artifact-body-color");
  const fontFamily = firstValue(result, "artifact-body-font");
  if (!display || display === "none") throw new Error("artifact body is not rendered");
  if (!color || /^(?:transparent|rgba\([^)]*,\s*0\))$/i.test(color.trim())) {
    throw new Error("artifact body has no visible computed color");
  }
  if (!fontFamily?.trim()) throw new Error("artifact body has no computed font-family");
  return {
    selector: "body",
    display,
    color,
    fontFamily,
    passed: true,
  };
}

async function collectComputedStyles(url, headers) {
  const input = JSON.stringify({
    url,
    ...(headers ? { headers } : {}),
    assertions: [
      { ruleId: "artifact-body-display", route: "/", selector: "body", property: "display" },
      { ruleId: "artifact-body-color", route: "/", selector: "body", property: "color" },
      { ruleId: "artifact-body-font", route: "/", selector: "body", property: "font-family" },
    ],
  });
  const child = spawn(process.execPath, [COLLECTOR], {
    env: process.env,
    stdio: ["pipe", "pipe", "pipe"],
  });
  child.stdin.end(input);
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8").on("data", chunk => { stdout += chunk; });
  child.stderr.setEncoding("utf8").on("data", chunk => { stderr += chunk; });
  const exitCode = await new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("exit", resolve);
  });
  let result;
  try {
    result = JSON.parse(stdout);
  } catch {
    throw new Error(`invalid computed-style output: ${stderr || stdout}`);
  }
  if (exitCode !== 0 && result?.ok !== false) {
    throw new Error(`computed-style collector exited ${exitCode}: ${stderr}`);
  }
  return result;
}

export async function assertArtifact({ url, expectedText, headers }) {
  if (typeof url !== "string" || !/^https?:\/\//.test(url)) throw new Error("url is required");
  if (typeof expectedText !== "string" || expectedText.length === 0) {
    throw new Error("expectedText is required");
  }
  if (headers !== undefined && (!headers || typeof headers !== "object" || Array.isArray(headers))) {
    throw new Error("headers must be an object");
  }
  const response = await fetch(url, headers ? { headers } : undefined);
  if (!response.ok) throw new Error(`artifact returned HTTP ${response.status}`);
  const document = await response.text();
  if (!document.includes(expectedText)) throw new Error("artifact content assertion failed");
  const computed = validateComputedStyleResult(await collectComputedStyles(url, headers));
  return {
    content: {
      expectedTextSha256: sha256(expectedText),
      documentSha256: sha256(document),
      matched: true,
    },
    computedStyle: computed,
  };
}

async function readStdin() {
  let value = "";
  for await (const chunk of process.stdin) value += chunk;
  return value;
}

async function main() {
  const input = JSON.parse(await readStdin());
  process.stdout.write(`${JSON.stringify(await assertArtifact(input))}\n`);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  await main();
}
