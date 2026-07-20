#!/usr/bin/env node
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const require = createRequire(import.meta.url);

function parseArgs(argv) {
  const args = {
    selector: ":root",
    property: "--runtime-primary",
    timeoutMs: 15000,
  };
  const flags = new Set(["--url", "--selector", "--property", "--expected", "--timeout-ms"]);
  const requireValue = (arg, value) => {
    if (typeof value !== "string" || value.trim() === "" || flags.has(value)) {
      throw new Error(`${arg} requires a non-empty value`);
    }
    return value;
  };
  for (let index = 2; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = argv[index + 1];
    if (arg === "--url") {
      args.url = requireValue(arg, next);
      index += 1;
    } else if (arg === "--selector") {
      args.selector = requireValue(arg, next);
      index += 1;
    } else if (arg === "--property") {
      args.property = requireValue(arg, next);
      index += 1;
    } else if (arg === "--expected") {
      args.expected = requireValue(arg, next);
      index += 1;
    } else if (arg === "--timeout-ms") {
      const timeoutValue = requireValue(arg, next);
      if (!/^[0-9]+$/.test(timeoutValue)) {
        throw new Error("--timeout-ms must be a positive integer");
      }
      args.timeoutMs = Number.parseInt(timeoutValue, 10);
      index += 1;
    } else {
      throw new Error(`Unknown argument: ${arg}`);
    }
  }
  if (!args.url) {
    throw new Error("Missing --url");
  }
  if (!args.expected) {
    throw new Error("Missing --expected");
  }
  try {
    new URL(args.url);
  } catch {
    throw new Error("--url must be a parseable URL");
  }
  if (!Number.isFinite(args.timeoutMs) || args.timeoutMs <= 0) {
    throw new Error("--timeout-ms must be a positive integer");
  }
  return args;
}

function normalizeCssValue(value) {
  return value.trim().replace(/\s+/g, " ").toLowerCase();
}

function parseCssColor(value) {
  const normalized = normalizeCssValue(value);
  const hex = normalized.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/i);
  if (hex) {
    const raw = hex[1];
    const expanded =
      raw.length === 3
        ? raw
            .split("")
            .map((part) => `${part}${part}`)
            .join("")
        : raw;
    return {
      r: Number.parseInt(expanded.slice(0, 2), 16),
      g: Number.parseInt(expanded.slice(2, 4), 16),
      b: Number.parseInt(expanded.slice(4, 6), 16),
      a: 1,
    };
  }

  const rgb = normalized.match(
    /^rgba?\(\s*([0-9.]+)\s*,\s*([0-9.]+)\s*,\s*([0-9.]+)(?:\s*,\s*([0-9.]+))?\s*\)$/,
  );
  if (!rgb) {
    return null;
  }
  return {
    r: Number.parseFloat(rgb[1]),
    g: Number.parseFloat(rgb[2]),
    b: Number.parseFloat(rgb[3]),
    a: rgb[4] === undefined ? 1 : Number.parseFloat(rgb[4]),
  };
}

function cssValuesMatch(actual, expected) {
  const normalizedActual = normalizeCssValue(actual);
  const normalizedExpected = normalizeCssValue(expected);
  if (normalizedActual === normalizedExpected) {
    return true;
  }

  const actualColor = parseCssColor(actual);
  const expectedColor = parseCssColor(expected);
  if (!actualColor || !expectedColor) {
    return false;
  }

  return (
    Math.round(actualColor.r) === Math.round(expectedColor.r) &&
    Math.round(actualColor.g) === Math.round(expectedColor.g) &&
    Math.round(actualColor.b) === Math.round(expectedColor.b) &&
    Math.abs(actualColor.a - expectedColor.a) < 0.001
  );
}

function localBrowserExecutable() {
  const candidates = [
    process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE,
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
  ].filter(Boolean);
  return candidates.find((candidate) => fs.existsSync(candidate));
}

function fileUrlToExistingPath(url) {
  const parsed = new URL(url);
  if (parsed.protocol !== "file:") {
    return null;
  }
  const filePath = fileURLToPath(parsed);
  if (!fs.existsSync(filePath)) {
    throw new Error(`Local artifact file does not exist: ${filePath}`);
  }
  return filePath;
}

function inferLocalArtifactRoot(filePath) {
  let current = fs.statSync(filePath).isDirectory() ? filePath : path.dirname(filePath);
  while (true) {
    if (
      fs.existsSync(path.join(current, "_next")) ||
      fs.existsSync(path.join(current, "_next"))
    ) {
      return current;
    }
    const parent = path.dirname(current);
    if (parent === current) {
      return fs.statSync(filePath).isDirectory() ? filePath : path.dirname(filePath);
    }
    current = parent;
  }
}

function fileHrefFor(root, relativePath) {
  const href = pathToFileURL(path.join(root, relativePath)).href;
  return relativePath.endsWith("/") && !href.endsWith("/") ? `${href}/` : href;
}

function pathWithinRoot(root, candidate) {
  const resolvedRoot = path.resolve(root);
  const resolvedCandidate = path.resolve(candidate);
  return (
    resolvedCandidate === resolvedRoot ||
    resolvedCandidate.startsWith(`${resolvedRoot}${path.sep}`)
  );
}

function rootRelativeArtifactPath(root, href) {
  const pathname = href.split("?")[0];
  let relative;
  try {
    relative = decodeURIComponent(pathname.replace(/^\//, ""));
  } catch {
    return null;
  }
  const target = path.resolve(root, relative);
  return pathWithinRoot(root, target) ? target : null;
}

function rewriteRootRelativeAssetUrls(html, root) {
  return html.replace(
    /\b(href|src)=(["'])(\/(?:_next|_next)\/[^"']+)\2/g,
    (match, attr, quote, href) => {
      const target = rootRelativeArtifactPath(root, href);
      if (!target) {
        return match;
      }
      return `${attr}=${quote}${pathToFileURL(target).href}${quote}`;
    },
  );
}

function inlineLocalStylesheetLinks(html, root) {
  return html.replace(
    /<link\b[^>]*\bhref=(["'])(\/(?:_next|_next)\/[^"']+\.css(?:\?[^"']*)?)\1[^>]*>/g,
    (tag, _quote, href) => {
      const cssPath = rootRelativeArtifactPath(root, href);
      if (!cssPath || !fs.existsSync(cssPath) || !fs.statSync(cssPath).isFile()) {
        return tag;
      }
      const css = fs.readFileSync(cssPath, "utf8");
      return `<style data-runtime-inlined-href="${href}">\n${css}\n</style>`;
    },
  );
}

function rewriteLocalArtifactHtml(html, root) {
  return rewriteRootRelativeAssetUrls(inlineLocalStylesheetLinks(html, root), root).replaceAll(
    'href="/favicon.svg"',
    `href="${fileHrefFor(root, "favicon.svg")}"`,
  );
}

async function loadArtifactPage(page, args) {
  const filePath = fileUrlToExistingPath(args.url);
  if (!filePath) {
    await page.goto(args.url, {
      waitUntil: "networkidle",
      timeout: args.timeoutMs,
    });
    return { mode: "url" };
  }

  const html = fs.readFileSync(filePath, "utf8");
  const root = inferLocalArtifactRoot(filePath);
  await page.setContent(rewriteLocalArtifactHtml(html, root), {
    waitUntil: "networkidle",
    timeout: args.timeoutMs,
  });
  return { mode: "local-file", localArtifactRoot: root };
}

async function launchChromium(chromium) {
  try {
    return await chromium.launch({ headless: true });
  } catch (error) {
    const executablePath = localBrowserExecutable();
    if (!executablePath) {
      throw error;
    }
    return chromium.launch({ headless: true, executablePath });
  }
}

async function main() {
  const args = parseArgs(process.argv);
  let chromium;
  try {
    ({ chromium } = require("playwright"));
  } catch (error) {
    throw new Error(
      `Playwright is required. Install it or set NODE_PATH to a node_modules containing playwright. ${error.message}`,
    );
  }

  const browser = await launchChromium(chromium);
  try {
    const page = await browser.newPage();
    const loadResult = await loadArtifactPage(page, args);
    const actual = await page.locator(args.selector).evaluate(
      (element, property) =>
        window.getComputedStyle(element).getPropertyValue(property),
      args.property,
    );
    if (!cssValuesMatch(actual, args.expected)) {
      console.log(
        JSON.stringify({
          ok: false,
          url: args.url,
          selector: args.selector,
          property: args.property,
          expected: args.expected,
          actual,
          mode: loadResult.mode,
          localArtifactRoot: loadResult.localArtifactRoot,
          error: `Computed style mismatch for ${args.selector} ${args.property}`,
        }),
      );
      process.exitCode = 1;
      return;
    }
    console.log(
      JSON.stringify({
        ok: true,
        url: args.url,
        selector: args.selector,
        property: args.property,
        expected: args.expected,
        actual,
        mode: loadResult.mode,
        localArtifactRoot: loadResult.localArtifactRoot,
      }),
    );
  } finally {
    await browser.close();
  }
}

main().catch((error) => {
  console.log(
    JSON.stringify({
      ok: false,
      error: error.message,
    }),
  );
  process.exit(1);
});
