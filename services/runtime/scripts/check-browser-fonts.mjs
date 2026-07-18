import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";

const TIMEOUT_MS = 20_000;
const browserExecutable = process.env.RUNTIME_BROWSER_EXECUTABLE || "/usr/bin/chromium";
const fixtures = [
  {
    id: "latin",
    lang: "en",
    text: "Reliable website and docs generation",
    family: "Noto Sans",
    expectedFamilies: ["Noto Sans", "Noto Sans CJK SC"],
  },
  {
    id: "cjk",
    lang: "zh-CN",
    text: "中文网站与文档生成",
    family: "Noto Sans CJK SC",
    expectedFamilies: ["Noto Sans CJK SC"],
  },
  {
    id: "emoji",
    lang: "und",
    text: "🚀✅",
    family: "Noto Color Emoji",
    expectedFamilies: ["Noto Color Emoji"],
  },
];

let browser;
let profileDir;
let server;

try {
  server = createServer((_request, response) => {
    const spans = fixtures
      .map(
        ({ id, lang, text, family }) =>
          `<p id="${id}" lang="${lang}" style="font-family:${JSON.stringify(family)};font-size:48px">${text}</p>`,
      )
      .join("");
    const html = `<!doctype html><html lang="zh-CN"><meta charset="utf-8"><style>body{margin:24px;background:#fff;color:#111}</style><body>${spans}</body></html>`;
    response.writeHead(200, {
      "content-type": "text/html; charset=utf-8",
      "content-length": Buffer.byteLength(html),
    });
    response.end(html);
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("font smoke server did not bind TCP");

  profileDir = await mkdtemp(path.join(tmpdir(), "anydesign-font-smoke-"));
  browser = spawn(
    browserExecutable,
    [
      "--headless=new",
      "--disable-gpu",
      "--no-sandbox",
      "--no-first-run",
      "--no-default-browser-check",
      "--remote-debugging-port=0",
      `--user-data-dir=${profileDir}`,
      "about:blank",
    ],
    { stdio: ["ignore", "ignore", "pipe"] },
  );
  let browserError;
  let browserStderr = "";
  browser.stderr?.on("data", (chunk) => {
    browserStderr = `${browserStderr}${String(chunk)}`.slice(-4096);
  });
  browser.once("error", (error) => {
    browserError = error;
  });
  browser.once("exit", (code, signal) => {
    if (!browserError) {
      browserError = new Error(
        `browser exited early (code=${code ?? "unknown"}, signal=${signal ?? "none"})${browserStderr.trim() ? `: ${browserStderr.trim().replace(/\s+/g, " ").slice(0, 1000)}` : ""}`,
      );
    }
  });

  const port = await waitForDevToolsPort(profileDir, () => browserError);
  const target = await createTarget(port, `http://127.0.0.1:${address.port}/`);
  const cdp = await connectCdp(target.webSocketDebuggerUrl);
  try {
    await cdp.send("Page.enable");
    await cdp.send("DOM.enable");
    await cdp.send("CSS.enable");
    const loaded = cdp.waitFor("Page.loadEventFired", TIMEOUT_MS);
    const navigation = await cdp.send("Page.navigate", {
      url: `http://127.0.0.1:${address.port}/`,
    });
    if (navigation.errorText) throw new Error(`font smoke navigation failed: ${navigation.errorText}`);
    await loaded;

    const document = await cdp.send("DOM.getDocument", { depth: -1, pierce: true });
    const results = [];
    for (const fixture of fixtures) {
      const selected = await cdp.send("DOM.querySelector", {
        nodeId: document.root.nodeId,
        selector: `#${fixture.id}`,
      });
      if (!selected.nodeId) throw new Error(`font smoke node is missing: ${fixture.id}`);
      const platform = await cdp.send("CSS.getPlatformFontsForNode", {
        nodeId: selected.nodeId,
      });
      const fonts = (platform.fonts || []).map((font) => ({
        familyName: String(font.familyName || ""),
        glyphCount: Number(font.glyphCount || 0),
        isCustomFont: Boolean(font.isCustomFont),
      }));
      const matched = fonts.some(
        (font) => fixture.expectedFamilies.includes(font.familyName) && font.glyphCount > 0,
      );
      if (!matched) {
        throw new Error(
          `${fixture.id} did not render with ${fixture.expectedFamilies.join(" or ")}: ${JSON.stringify(fonts)}`,
        );
      }
      results.push({ id: fixture.id, expectedFamilies: fixture.expectedFamilies, fonts });
    }

    const screenshot = await cdp.send("Page.captureScreenshot", { format: "png" });
    const screenshotBytes = Buffer.from(String(screenshot.data || ""), "base64");
    if (screenshotBytes.length < 1_000) {
      throw new Error(`font smoke screenshot is unexpectedly small: ${screenshotBytes.length}`);
    }
    process.stdout.write(
      `${JSON.stringify({ ok: true, browserExecutable, screenshotBytes: screenshotBytes.length, results })}\n`,
    );
  } finally {
    cdp.close();
  }
} catch (error) {
  process.stderr.write(`Runtime browser font smoke failed: ${error.message}\n`);
  process.exitCode = 1;
} finally {
  await stopBrowser(browser);
  if (server) await new Promise((resolve) => server.close(resolve));
  if (profileDir) await rm(profileDir, { recursive: true, force: true }).catch(() => {});
}

async function waitForDevToolsPort(directory, startError) {
  const activePortPath = path.join(directory, "DevToolsActivePort");
  const deadline = Date.now() + TIMEOUT_MS;
  while (Date.now() < deadline) {
    const error = startError();
    if (error) throw new Error(`failed to start browser: ${error.message}`);
    try {
      const [port] = (await readFile(activePortPath, "utf8")).trim().split("\n");
      if (/^\d+$/.test(port)) return Number(port);
    } catch {
      // Chromium creates DevToolsActivePort after its browser process is ready.
    }
    await sleep(50);
  }
  throw new Error("timed out waiting for Chromium DevTools port");
}

async function createTarget(port, url) {
  const response = await fetch(`http://127.0.0.1:${port}/json/new?${encodeURIComponent(url)}`, {
    method: "PUT",
  });
  if (!response.ok) throw new Error(`DevTools target creation failed with ${response.status}`);
  return response.json();
}

async function connectCdp(url) {
  const socket = new WebSocket(url);
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error("timed out connecting to DevTools")), TIMEOUT_MS);
    socket.addEventListener(
      "open",
      () => {
        clearTimeout(timeout);
        resolve();
      },
      { once: true },
    );
    socket.addEventListener(
      "error",
      () => {
        clearTimeout(timeout);
        reject(new Error("failed to connect to DevTools"));
      },
      { once: true },
    );
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
          eventWaiters.set(
            method,
            (eventWaiters.get(method) || []).filter((item) => item !== waiter),
          );
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

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
