#!/usr/bin/env node
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

const TIMEOUT_MS = 45_000;
let browser;
let profileDir;
let browserStartError;
let browserStderr = "";

try {
  const input = JSON.parse(await readStdin());
  validateInput(input);
  const executable = browserExecutable(input.browserExecutable);
  if (!executable) throw new Error("Chrome or Chromium is required for the browser smoke fixture");

  profileDir = await mkdtemp(path.join(tmpdir(), "zerondesign-browser-dcp-"));
  browser = spawn(executable, [
    "--headless=new",
    "--disable-gpu",
    "--no-sandbox",
    "--no-first-run",
    "--no-default-browser-check",
    "--remote-debugging-port=0",
    `--user-data-dir=${profileDir}`,
    "about:blank",
  ], { stdio: ["ignore", "ignore", "pipe"] });
  browser.stderr?.on("data", (chunk) => {
    browserStderr = `${browserStderr}${String(chunk)}`.slice(-4096);
  });
  browser.once("error", (error) => {
    browserStartError = error;
  });
  browser.once("exit", (code, signal) => {
    if (!browserStartError) {
      browserStartError = new Error(
        `browser exited before DevTools became available (code=${code ?? "unknown"}, signal=${signal ?? "none"})${browserStderr.trim() ? `: ${browserStderr.trim().replace(/\s+/g, " ").slice(0, 1000)}` : ""}`,
      );
    }
  });

  const port = await waitForDevToolsPort(profileDir, () => browserStartError);
  const target = await createTarget(port, "about:blank");
  const cdp = await connectCdp(target.webSocketDebuggerUrl);
  try {
    await cdp.send("Page.enable");
    await cdp.send("Runtime.enable");
    const loaded = cdp.waitFor("Page.loadEventFired", TIMEOUT_MS);
    const navigation = await cdp.send("Page.navigate", { url: input.baseUrl });
    if (navigation.errorText) throw new Error(`navigation failed: ${navigation.errorText}`);
    await loaded;

    await waitForValue(cdp, `(() => Boolean(Array.from(document.querySelectorAll("button[data-project-id]")).find((element) => element.dataset.projectId === ${JSON.stringify(input.projectId)})))()`, "seeded project");
    await evaluate(cdp, `(() => { const matches = Array.from(document.querySelectorAll("button[data-project-id]")).filter((element) => element.dataset.projectId === ${JSON.stringify(input.projectId)}); if (matches.length !== 1) return { count: matches.length }; matches[0].click(); return { count: 1, clicked: true }; })()`);

    await waitForValue(cdp, readyCardExpression(), "ready DCP card and enabled Profile Sync entry");
    const before = await stateSnapshot(cdp);
    assert(before.card.includes("DESIGN CONTEXT · READY"), "browser did not render the ready DCP gate");
    assert(before.card.includes("materialized"), "browser did not render DCP materialization");
    assert(before.card.includes("verified"), "browser did not render style-contract verification");
    assert(before.fidelity.includes("FIDELITY · FAILED"), "browser did not render the failed fidelity outcome");
    assert(before.fidelity.includes("craft:accessibility-baseline:image-alt"), "browser did not link the accessibility rule");
    assert(before.fidelity.includes("375px"), "browser did not render the failed viewport");
    assert(before.fidelity.includes("420px") && before.fidelity.includes("375px"), "browser did not render current and target fidelity states");
    assert(before.fidelity.includes("project/src/styles/global.css"), "browser did not render bounded repair context");
    await waitForValue(cdp, `(() => { const section = document.querySelector('[data-testid="fidelity-outcome"][data-status="failed"]'); const link = section?.querySelector('a[href^="#fidelity-rule-"]'); return Boolean(link && document.querySelector(link.getAttribute("href"))); })()`, "linked fidelity rule detail");

    await clickUniqueTestId(cdp, "profile-sync-plan-trigger");
    await waitForValue(cdp, `(() => { const plan = document.querySelector('[data-testid="profile-sync-plan"]'); return Boolean(plan && plan.textContent.includes("1 项需要决议")); })()`, "Profile Sync conflict plan");
    const planned = await stateSnapshot(cdp);
    assert(planned.plan.includes("PROFILE SYNC · planned"), "browser did not render a planned Profile Sync operation");

    await clickUniqueTestId(cdp, "profile-sync-confirm");
    await waitForValue(cdp, `(() => document.querySelector('[data-testid="runtime-status"]')?.textContent?.includes("请为每个冲突 token 选择") || false)()`, "missing conflict decision guard");

    await clickUniqueTestId(cdp, "profile-sync-apply-target");
    await waitForValue(cdp, `(() => document.querySelector('[data-testid="profile-sync-apply-target"]')?.checked === true)()`, "explicit apply-target decision");
    await clickUniqueTestId(cdp, "profile-sync-confirm");
    await waitForValue(cdp, `(() => { const aligned = document.querySelector('[data-testid="profile-sync-aligned"]'); const card = document.querySelector('[data-testid="design-context-card"]'); return Boolean(aligned && card?.textContent.includes("DESIGN CONTEXT · READ REQUIRED") && card.textContent.includes("materialized") && card.textContent.includes("pending")); })()`, "Profile Sync child Run DCP ownership");

    const after = await stateSnapshot(cdp);
    const screenshot = await cdp.send("Page.captureScreenshot", { format: "png" });
    const screenshotSha256 = createHash("sha256")
      .update(Buffer.from(String(screenshot.data || ""), "base64"))
      .digest("hex");
    process.stdout.write(JSON.stringify({
      ok: true,
      projectId: input.projectId,
      checks: [
        "dcp-ready",
        "dcp-materialized",
        "style-contract-verified",
        "fidelity-rule-detail",
        "fidelity-current-target",
        "fidelity-repair-context",
        "profile-sync-conflict-plan",
        "missing-decision-blocked",
        "explicit-apply-target",
        "child-run-dcp-owned-and-aligned",
      ],
      before,
      after,
      screenshotSha256,
    }));
  } finally {
    cdp.close();
  }
} catch (error) {
  process.stdout.write(JSON.stringify({ ok: false, error: error.message }));
  process.exitCode = 1;
} finally {
  await stopBrowser(browser);
  if (profileDir) await rm(profileDir, { recursive: true, force: true }).catch(() => {});
}

function validateInput(input) {
  if (!input || typeof input !== "object") throw new Error("input must be an object");
  const baseUrl = new URL(input.baseUrl);
  if (baseUrl.protocol !== "http:" || !["127.0.0.1", "localhost", "[::1]"].includes(baseUrl.hostname)) {
    throw new Error("baseUrl must be a loopback HTTP fixture URL");
  }
  if (baseUrl.pathname !== "/" || baseUrl.search || baseUrl.hash) {
    throw new Error("baseUrl must point to the fixture root");
  }
  if (typeof input.projectId !== "string" || !/^[a-zA-Z0-9_-]{1,160}$/.test(input.projectId)) {
    throw new Error("projectId must be a bounded identifier");
  }
  if (input.browserExecutable !== undefined && (typeof input.browserExecutable !== "string" || input.browserExecutable.trim() === "")) {
    throw new Error("browserExecutable must be a non-empty path");
  }
}

function readyCardExpression() {
  return `(() => { const card = document.querySelector('[data-testid="design-context-card"]'); const trigger = document.querySelector('[data-testid="profile-sync-plan-trigger"]'); return Boolean(card?.textContent.includes("DESIGN CONTEXT · READY") && card.textContent.includes("materialized") && card.textContent.includes("verified") && trigger && !trigger.disabled); })()`;
}

async function stateSnapshot(cdp) {
  return evaluate(cdp, `(() => ({
    status: document.querySelector('[data-testid="runtime-status"]')?.textContent?.trim() || "",
    card: document.querySelector('[data-testid="design-context-card"]')?.textContent?.replace(/\\s+/g, " ").trim() || "",
    plan: document.querySelector('[data-testid="profile-sync-plan"]')?.textContent?.replace(/\\s+/g, " ").trim() || "",
    aligned: document.querySelector('[data-testid="profile-sync-aligned"]')?.textContent?.trim() || "",
    fidelity: document.querySelector('[data-testid="fidelity-outcome"]')?.textContent?.replace(/\\s+/g, " ").trim() || "",
  }))()`);
}

async function clickUniqueTestId(cdp, testId) {
  const result = await evaluate(cdp, `(() => { const matches = Array.from(document.querySelectorAll('[data-testid=${JSON.stringify(testId)}]')); if (matches.length !== 1) return { count: matches.length }; if (matches[0].disabled) return { count: 1, disabled: true }; matches[0].click(); return { count: 1, clicked: true }; })()`);
  if (!result?.clicked) throw new Error(`expected one enabled ${testId} control, got ${JSON.stringify(result)}`);
}

async function waitForValue(cdp, expression, label) {
  const deadline = Date.now() + TIMEOUT_MS;
  let lastError;
  while (Date.now() < deadline) {
    try {
      if (await evaluate(cdp, expression)) return;
    } catch (error) {
      lastError = error;
    }
    await sleep(100);
  }
  const state = await stateSnapshot(cdp).catch(() => null);
  throw new Error(`timed out waiting for ${label}${lastError ? `: ${lastError.message}` : ""}${state ? `; state=${JSON.stringify(state)}` : ""}`);
}

async function evaluate(cdp, expression) {
  const evaluation = await cdp.send("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (evaluation.exceptionDetails) throw new Error(`browser evaluation failed: ${evaluation.exceptionDetails.text || "unknown error"}`);
  return evaluation.result?.value;
}

function browserExecutable(boundExecutable) {
  if (boundExecutable) return boundExecutable;
  return [
    process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE,
    process.env.CHROME_PATH,
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
  ].filter(Boolean).find(existsSync);
}

async function waitForDevToolsPort(directory, startError = () => undefined) {
  const activePortPath = path.join(directory, "DevToolsActivePort");
  const deadline = Date.now() + TIMEOUT_MS;
  while (Date.now() < deadline) {
    const error = startError();
    if (error) throw new Error(`failed to start browser: ${error.message}`);
    try {
      const [port] = (await readFile(activePortPath, "utf8")).trim().split("\n");
      if (/^\d+$/.test(port)) return Number(port);
    } catch {
      // Chrome creates the file after its browser process is ready.
    }
    await sleep(50);
  }
  throw new Error("timed out waiting for Chrome DevTools port");
}

async function createTarget(port, url) {
  const response = await fetch(`http://127.0.0.1:${port}/json/new?${encodeURIComponent(url)}`, { method: "PUT" });
  if (!response.ok) throw new Error(`DevTools target creation failed with ${response.status}`);
  return response.json();
}

async function connectCdp(url) {
  const socket = new WebSocket(url);
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error("timed out connecting to DevTools")), TIMEOUT_MS);
    socket.addEventListener("open", () => { clearTimeout(timeout); resolve(); }, { once: true });
    socket.addEventListener("error", () => { clearTimeout(timeout); reject(new Error("failed to connect to DevTools")); }, { once: true });
  });
  let nextId = 1;
  const pending = new Map();
  const eventWaiters = new Map();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(String(event.data));
    if (message.id) {
      const waiter = pending.get(message.id);
      if (!waiter) return;
      pending.delete(message.id);
      if (message.error) waiter.reject(new Error(message.error.message));
      else waiter.resolve(message.result || {});
      return;
    }
    const waiters = eventWaiters.get(message.method) || [];
    eventWaiters.delete(message.method);
    for (const waiter of waiters) waiter.resolve(message.params || {});
  });
  return {
    send(method, params = {}) {
      return new Promise((resolve, reject) => {
        const id = nextId++;
        pending.set(id, { resolve, reject });
        socket.send(JSON.stringify({ id, method, params }));
      });
    },
    waitFor(method, timeoutMs) {
      return new Promise((resolve, reject) => {
        const waiters = eventWaiters.get(method) || [];
        const waiter = { resolve(value) { clearTimeout(timeout); resolve(value); } };
        const timeout = setTimeout(() => {
          eventWaiters.set(method, (eventWaiters.get(method) || []).filter((item) => item !== waiter));
          reject(new Error(`timed out waiting for ${method}`));
        }, timeoutMs);
        waiters.push(waiter);
        eventWaiters.set(method, waiters);
      });
    },
    close() { socket.close(); },
  };
}

async function readStdin() {
  let value = "";
  for await (const chunk of process.stdin) value += chunk;
  return value;
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function stopBrowser(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise((resolve) => child.once("exit", resolve));
  child.kill("SIGTERM");
  await Promise.race([exited, sleep(2_000)]);
  if (child.exitCode === null && child.signalCode === null) {
    child.kill("SIGKILL");
    await Promise.race([exited, sleep(1_000)]);
  }
}
