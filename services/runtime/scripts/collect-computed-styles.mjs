import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

const TIMEOUT_MS = 20_000;
let browser;
let profileDir;

try {
  const input = JSON.parse(await readStdin());
  validateInput(input);
  const executable = browserExecutable();
  if (!executable) throw new Error("Chrome or Chromium is required for computed-style evidence");

  profileDir = await mkdtemp(path.join(tmpdir(), "anydesign-fidelity-"));
  browser = spawn(executable, [
    "--headless=new",
    "--disable-gpu",
    "--no-first-run",
    "--no-default-browser-check",
    "--remote-debugging-port=0",
    `--user-data-dir=${profileDir}`,
    "about:blank",
  ], { stdio: "ignore" });

  const port = await waitForDevToolsPort(profileDir);
  const target = await createTarget(port, input.url);
  const cdp = await connectCdp(target.webSocketDebuggerUrl);
  try {
    await cdp.send("Page.enable");
    await cdp.send("Runtime.enable");
    const byRoute = Map.groupBy(input.assertions, (assertion) => assertion.route || "/");
    const results = {};
    for (const [route, assertions] of byRoute) {
      const url = new URL(route, input.url).toString();
      const loaded = cdp.waitFor("Page.loadEventFired", TIMEOUT_MS);
      const navigation = await cdp.send("Page.navigate", { url });
      if (navigation.errorText) throw new Error(`navigation failed: ${navigation.errorText}`);
      await loaded;
      for (const assertion of assertions) {
        const expression = `(() => Array.from(document.querySelectorAll(${JSON.stringify(assertion.selector)})).filter((element) => !${JSON.stringify(assertion.excludeWithin || "")} || !element.closest(${JSON.stringify(assertion.excludeWithin || ":scope:not(*)")})).map((element) => { const style = getComputedStyle(element); return { value: style.getPropertyValue(${JSON.stringify(assertion.property)}), referenceValue: ${JSON.stringify(assertion.referenceProperty || "")} ? style.getPropertyValue(${JSON.stringify(assertion.referenceProperty || "")}) : "" }; }))()`;
        const evaluation = await cdp.send("Runtime.evaluate", {
          expression,
          returnByValue: true,
          awaitPromise: true,
        });
        if (evaluation.exceptionDetails) {
          throw new Error(`computed-style evaluation failed for ${assertion.ruleId}`);
        }
        const samples = Array.isArray(evaluation.result?.value) ? evaluation.result.value : [];
        results[assertion.ruleId] = {
          route,
          selector: assertion.selector,
          property: assertion.property,
          referenceProperty: assertion.referenceProperty || null,
          excludeWithin: assertion.excludeWithin || null,
          values: samples.map((sample) => String(sample.value || "")),
          referenceValues: samples.map((sample) => String(sample.referenceValue || "")),
        };
      }
    }
    process.stdout.write(JSON.stringify({ ok: true, results }));
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
  new URL(input.url);
  if (!Array.isArray(input.assertions) || input.assertions.length === 0) {
    throw new Error("assertions must be a non-empty array");
  }
  for (const assertion of input.assertions) {
    for (const field of ["ruleId", "selector", "property"]) {
      if (typeof assertion[field] !== "string" || assertion[field].trim() === "") {
        throw new Error(`${field} must be a non-empty string`);
      }
    }
    if (assertion.route !== undefined && (!assertion.route.startsWith("/") || assertion.route.startsWith("//"))) {
      throw new Error("route must be a root-relative path");
    }
  }
}

function browserExecutable() {
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

async function waitForDevToolsPort(directory) {
  const activePortPath = path.join(directory, "DevToolsActivePort");
  const deadline = Date.now() + TIMEOUT_MS;
  while (Date.now() < deadline) {
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
  const response = await fetch(`http://127.0.0.1:${port}/json/new?${encodeURIComponent(url)}`, {
    method: "PUT",
  });
  if (!response.ok) throw new Error(`DevTools target creation failed with ${response.status}`);
  return await response.json();
}

async function connectCdp(url) {
  const socket = new WebSocket(url);
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error("timed out connecting to DevTools")), TIMEOUT_MS);
    socket.addEventListener("open", () => {
      clearTimeout(timeout);
      resolve();
    }, { once: true });
    socket.addEventListener("error", () => {
      clearTimeout(timeout);
      reject(new Error("failed to connect to DevTools"));
    }, { once: true });
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
        const timeout = setTimeout(() => {
          eventWaiters.set(method, (eventWaiters.get(method) || []).filter((item) => item !== waiter));
          reject(new Error(`timed out waiting for ${method}`));
        }, timeoutMs);
        const waiter = {
          resolve(value) {
            clearTimeout(timeout);
            resolve(value);
          },
        };
        waiters.push(waiter);
        eventWaiters.set(method, waiters);
      });
    },
    close() {
      socket.close();
    },
  };
}

async function readStdin() {
  let value = "";
  for await (const chunk of process.stdin) value += chunk;
  return value;
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
