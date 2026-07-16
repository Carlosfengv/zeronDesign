import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { createHash } from "node:crypto";
import path from "node:path";

const TIMEOUT_MS = 20_000;
let browser;
let profileDir;
let browserStartError;
let browserStderr = "";

try {
  const input = JSON.parse(await readStdin());
  validateInput(input);
  const executable = browserExecutable(input.browserExecutable);
  if (!executable) throw new Error("Chrome or Chromium is required for computed-style evidence");

  profileDir = await mkdtemp(path.join(tmpdir(), "anydesign-fidelity-"));
  browser = spawn(executable, [
    "--headless=new",
    "--disable-gpu",
    // The Runtime Pod is the browser isolation boundary: it runs as non-root
    // with RuntimeDefault seccomp, no privilege escalation, and all Linux
    // capabilities dropped. Debian Chromium cannot initialize its setuid
    // sandbox under that no-new-privileges policy.
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
        `browser worker exited before DevTools became available (code=${code ?? "unknown"}, signal=${signal ?? "none"})${browserStderr.trim() ? `: ${browserStderr.trim().replace(/\s+/g, " ").slice(0, 1000)}` : ""}`,
      );
    }
  });

  const port = await waitForDevToolsPort(profileDir, () => browserStartError);
  const target = await createTarget(port, input.url);
  const cdp = await connectCdp(target.webSocketDebuggerUrl);
  try {
    await cdp.send("Page.enable");
    await cdp.send("Runtime.enable");
    if (input.headers) {
      await cdp.send("Network.enable");
      await cdp.send("Network.setExtraHTTPHeaders", { headers: input.headers });
    }
    const byRoute = Map.groupBy(input.assertions, (assertion) => assertion.route || "/");
    const results = {};
    for (const [route, assertions] of byRoute) {
      const url = new URL(route, input.url).toString();
      const loaded = cdp.waitFor("Page.loadEventFired", TIMEOUT_MS);
      const navigation = await cdp.send("Page.navigate", { url });
      if (navigation.errorText) throw new Error(`navigation failed: ${navigation.errorText}`);
      await loaded;
      for (const assertion of assertions) {
        if (assertion.kind === "viewport") {
          await cdp.send("Emulation.setDeviceMetricsOverride", {
            width: assertion.viewport,
            height: assertion.height || 900,
            deviceScaleFactor: 1,
            mobile: false,
          });
          const loaded = cdp.waitFor("Page.loadEventFired", TIMEOUT_MS);
          const navigation = await cdp.send("Page.navigate", { url });
          if (navigation.errorText) throw new Error(`navigation failed: ${navigation.errorText}`);
          await loaded;
          const evaluation = await cdp.send("Runtime.evaluate", {
            expression: `(() => ({ scrollWidth: document.documentElement.scrollWidth, viewportWidth: window.innerWidth, bodyScrollWidth: document.body?.scrollWidth || 0 }))()`,
            returnByValue: true,
            awaitPromise: true,
          });
          if (evaluation.exceptionDetails) throw new Error(`viewport evaluation failed for ${assertion.ruleId}`);
          const metrics = evaluation.result?.value || {};
          const screenshot = await cdp.send("Page.captureScreenshot", { format: "png" });
          const screenshotBytes = Buffer.from(String(screenshot.data || ""), "base64");
          const screenshotSha256 = createHash("sha256").update(screenshotBytes).digest("hex");
          let screenshotUri = null;
          if (input.viewportScreenshotDir && input.viewportScreenshotUriPrefix) {
            const filename = `${safeSegment(assertion.ruleId)}-${assertion.viewport}.png`;
            await mkdir(input.viewportScreenshotDir, { recursive: true });
            await writeFile(path.join(input.viewportScreenshotDir, filename), screenshotBytes);
            screenshotUri = `${input.viewportScreenshotUriPrefix.replace(/\/$/, "")}/${filename}`;
          }
          results[assertion.ruleId] = {
            route,
            kind: assertion.kind,
            viewport: assertion.viewport,
            check: assertion.check,
            scrollWidth: Number(metrics.scrollWidth || 0),
            viewportWidth: Number(metrics.viewportWidth || assertion.viewport),
            bodyScrollWidth: Number(metrics.bodyScrollWidth || 0),
            screenshotSha256,
            screenshotUri,
          };
          continue;
        }
        if (assertion.kind === "a11y") {
          const evaluation = await cdp.send("Runtime.evaluate", {
            expression: a11yExpression(assertion.check),
            returnByValue: true,
            awaitPromise: true,
          });
          if (evaluation.exceptionDetails) throw new Error(`a11y evaluation failed for ${assertion.ruleId}`);
          const violations = Array.isArray(evaluation.result?.value) ? evaluation.result.value : [];
          results[assertion.ruleId] = {
            route,
            kind: assertion.kind,
            check: assertion.check,
            violations,
          };
          continue;
        }
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
    for (const field of ["ruleId"]) {
      if (typeof assertion[field] !== "string" || assertion[field].trim() === "") {
        throw new Error(`${field} must be a non-empty string`);
      }
    }
    if (!["computed-style", "a11y", "viewport"].includes(assertion.kind || "computed-style")) {
      throw new Error(`unsupported assertion kind for ${assertion.ruleId}`);
    }
    if ((assertion.kind || "computed-style") === "computed-style") {
      for (const field of ["selector", "property"]) {
        if (typeof assertion[field] !== "string" || assertion[field].trim() === "") {
          throw new Error(`${field} must be a non-empty string`);
        }
      }
    }
    if (assertion.kind === "a11y" && !["image-alt", "button-name", "link-name"].includes(assertion.check)) {
      throw new Error(`unsupported a11y check for ${assertion.ruleId}`);
    }
    if (assertion.kind === "viewport" && (!Number.isInteger(assertion.viewport) || assertion.viewport < 320 || assertion.viewport > 3840)) {
      throw new Error(`viewport must be an integer in range for ${assertion.ruleId}`);
    }
    if (assertion.route !== undefined && (!assertion.route.startsWith("/") || assertion.route.startsWith("//"))) {
      throw new Error("route must be a root-relative path");
    }
  }
  if ((input.viewportScreenshotDir === undefined) !== (input.viewportScreenshotUriPrefix === undefined)) {
    throw new Error("viewport screenshot directory and URI prefix must be supplied together");
  }
  if (input.viewportScreenshotDir !== undefined && (typeof input.viewportScreenshotDir !== "string" || input.viewportScreenshotDir.trim() === "")) {
    throw new Error("viewportScreenshotDir must be a non-empty string");
  }
  if (input.viewportScreenshotUriPrefix !== undefined && (typeof input.viewportScreenshotUriPrefix !== "string" || !input.viewportScreenshotUriPrefix.startsWith("runtime://"))) {
    throw new Error("viewportScreenshotUriPrefix must be a runtime URI");
  }
  if (input.browserExecutable !== undefined && (typeof input.browserExecutable !== "string" || input.browserExecutable.trim() === "")) {
    throw new Error("browserExecutable must be a non-empty Runtime worker path");
  }
  if (input.headers !== undefined) {
    if (!input.headers || typeof input.headers !== "object" || Array.isArray(input.headers)) {
      throw new Error("headers must be an object");
    }
    for (const [name, value] of Object.entries(input.headers)) {
      if (!/^[a-z0-9-]+$/i.test(name) || typeof value !== "string" || value.length === 0) {
        throw new Error("headers must contain non-empty string values with valid names");
      }
    }
  }
}

function browserExecutable(boundExecutable) {
  // An enforced DCP binds the Runtime to the worker that passed its StartRun
  // health probe.  Falling back to a different local Chrome when that worker
  // disappears would incorrectly turn a verification-runtime outage into a
  // successful page verification.  The fallback list is only for legacy and
  // observe executions that have no configured Runtime worker.
  const configured = boundExecutable || process.env.RUNTIME_BROWSER_EXECUTABLE?.trim();
  if (configured) return configured;
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

function a11yExpression(check) {
  const text = `(element) => (element.getAttribute("aria-label") || element.getAttribute("title") || element.textContent || "").replace(/\\s+/g, " ").trim()`;
  if (check === "image-alt") {
    return `(() => Array.from(document.images).filter((image) => !image.hasAttribute("alt")).map((image) => image.outerHTML.slice(0, 240)))()`;
  }
  if (check === "button-name") {
    return `(() => Array.from(document.querySelectorAll("button, [role=button]")).filter((element) => !(${text})(element)).map((element) => element.outerHTML.slice(0, 240)))()`;
  }
  return `(() => Array.from(document.querySelectorAll("a[href]")).filter((element) => !(${text})(element)).map((element) => element.outerHTML.slice(0, 240)))()`;
}

function safeSegment(value) {
  return String(value).replace(/[^a-zA-Z0-9._-]/g, "-").slice(0, 160) || "viewport";
}

async function waitForDevToolsPort(directory, startError = () => undefined) {
  const activePortPath = path.join(directory, "DevToolsActivePort");
  const deadline = Date.now() + TIMEOUT_MS;
  while (Date.now() < deadline) {
    const error = startError();
    if (error) throw new Error(`failed to start browser worker: ${error.message}`);
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
