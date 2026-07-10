#!/usr/bin/env node
import { createRequire } from "node:module";
import fs from "node:fs";

const require = createRequire(import.meta.url);
const args = parseArgs(process.argv);

let chromium;
try {
  ({ chromium } = require("playwright"));
} catch (error) {
  throw new Error(`Playwright is required: ${error.message}`);
}

const browser = await launchBrowser(chromium);
try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
  await page.goto(args.url, { waitUntil: "networkidle", timeout: 30_000 });
  const result = await page.evaluate((preset) => {
    const normalize = (value) => String(value || "").trim().toLowerCase().replace(/\s+/g, " ");
    const rgb = (value) => {
      const match = normalize(value).match(/rgba?\((\d+),\s*(\d+),\s*(\d+)/);
      return match ? `#${[match[1], match[2], match[3]].map((part) => Number(part).toString(16).padStart(2, "0")).join("")}` : normalize(value);
    };
    const styleText = Array.from(document.styleSheets)
      .map((sheet) => {
        try {
          return Array.from(sheet.cssRules).map((rule) => rule.cssText).join("\n");
        } catch {
          return "";
        }
      })
      .join("\n");
    const rootStyle = getComputedStyle(document.documentElement);
    const bodyStyle = getComputedStyle(document.body);
    const h1 = document.querySelector("h1");
    const h1Style = h1 ? getComputedStyle(h1) : null;
    const assertions = [];
    const check = (id, passed, actual, expected) => assertions.push({ id, passed: Boolean(passed), actual, expected });
    const elements = Array.from(document.querySelectorAll("a, button, [role=button]"));

    if (preset === "authkit") {
      const background = rootStyle.getPropertyValue("--runtime-bg") || bodyStyle.backgroundColor;
      check("root-background", rgb(background) === "#05060f", background, "#05060f");
      const primary = rootStyle.getPropertyValue("--runtime-primary");
      check("runtime-primary", rgb(primary) === "#663af3", primary, "#663af3");
      const font = h1Style?.fontFamily || "";
      check("display-font", /aeonik|space grotesk/i.test(font), font, "aeonikPro or Space Grotesk");
      const gradientCandidates = h1 ? [h1, ...h1.querySelectorAll("*")] : [];
      const gradientTarget = gradientCandidates.find((element) => {
        const style = getComputedStyle(element);
        const clip = style.webkitBackgroundClip || style.backgroundClip || "";
        return style.backgroundImage !== "none" && /gradient/i.test(style.backgroundImage) && clip === "text";
      });
      const gradientStyle = gradientTarget ? getComputedStyle(gradientTarget) : null;
      check("gradient-heading", Boolean(gradientTarget), gradientStyle ? `${gradientStyle.backgroundImage}; clip=${gradientStyle.webkitBackgroundClip || gradientStyle.backgroundClip}` : "none", "gradient + background-clip:text on h1 or descendant");
      const eyebrow = document.querySelector("[data-eyebrow], .runtime-kicker, .eyebrow, [class*=eyebrow]");
      const eyebrowStyle = eyebrow ? getComputedStyle(eyebrow) : null;
      const tracking = Number.parseFloat(eyebrowStyle?.letterSpacing || "NaN");
      const eyebrowSize = Number.parseFloat(eyebrowStyle?.fontSize || "NaN");
      check("eyebrow-tracking", eyebrow && Math.abs(tracking - eyebrowSize * 0.1) <= 0.35, `${tracking}px at ${eyebrowSize}px`, "0.10em");
      const gridLayer = document.querySelector("[data-blueprint-grid], .authkit-blueprint-grid");
      const spotlight = document.querySelector(".authkit-spotlight, [data-spotlight]");
      const gridImage = gridLayer ? getComputedStyle(gridLayer).backgroundImage : "";
      const gridSize = gridLayer ? getComputedStyle(gridLayer).backgroundSize : "";
      const spotlightImage = spotlight ? getComputedStyle(spotlight).backgroundImage : "";
      const grid = /linear-gradient/i.test(gridImage) && /(?:8\d|9\d|100)px/i.test(gridSize) && /conic-gradient/i.test(spotlightImage);
      check("blueprint-grid", grid, { gridImage, gridSize, spotlightImage }, "80-100px grid plus conic spotlight");
      const authForms = document.querySelectorAll("[data-auth-form], .auth-form, form:has(input[type=password]), form:has(input[type=email])").length;
      check("auth-form-signal", authForms >= 2, authForms, ">= 2 overlapping auth forms");
      const violetInteractive = elements.filter((element) => rgb(getComputedStyle(element).backgroundColor) === "#663af3");
      const violetLeak = violetInteractive.filter((element) => !element.closest("form, [data-auth-form], [data-auth-submit]") && !element.matches("[data-auth-submit]")).length;
      check("violet-action-scope", violetInteractive.length >= 1 && violetLeak === 0, { violetInteractive: violetInteractive.length, violetLeak }, "violet only in auth submit context");
    } else {
      const background = rootStyle.getPropertyValue("--runtime-bg") || bodyStyle.backgroundColor;
      check("root-background", rgb(background) === "#fdfcfc", background, "#fdfcfc");
      const surfaces = Array.from(document.querySelectorAll("article, section, [class*=card], [data-feature-card]"));
      const taupeSurfaceCount = surfaces.filter((element) => rgb(getComputedStyle(element).backgroundColor) === "#f5f3f1").length;
      check("feature-surface", taupeSurfaceCount >= 1, taupeSurfaceCount, "feature surface #f5f3f1");
      const weight = Number.parseInt(h1Style?.fontWeight || "0", 10);
      check("display-weight", weight >= 250 && weight <= 350, weight, "300");
      const h1Size = Number.parseFloat(h1Style?.fontSize || "NaN");
      const h1Tracking = Number.parseFloat(h1Style?.letterSpacing || "NaN");
      check("display-tracking", h1Size >= 32 && Math.abs(h1Tracking - h1Size * -0.02) <= 0.5, `${h1Tracking}px at ${h1Size}px`, "-0.02em");
      const blackCtas = elements.filter((element) => rgb(getComputedStyle(element).backgroundColor) === "#000000");
      check("black-primary-cta", blackCtas.length >= 1, blackCtas.length, ">= 1 black filled CTA");
      const accentCtas = elements.filter((element) => ["#0447ff", "#ff4704"].includes(rgb(getComputedStyle(element).backgroundColor)));
      check("accent-cta-forbidden", accentCtas.length === 0, accentCtas.length, "0 violet/orange interactive CTAs");
      const voiceEvidence = document.querySelectorAll("audio, .waveform, .audio-sphere, [data-voice-product], [data-product-visual]").length;
      check("voice-product-evidence", voiceEvidence >= 1, voiceEvidence, ">= 1 audio or voice product visual");
      const dividerCount = Array.from(document.querySelectorAll("hr, [class*=divider], section"))
        .filter((element) => {
          const style = getComputedStyle(element);
          return rgb(style.borderTopColor) === "#ebe8e4" && Number.parseFloat(style.borderTopWidth) >= 1;
        }).length;
      check("hairline-divider", dividerCount >= 1, dividerCount, "#ebe8e4 1px divider");
    }
    return { assertions, styleChars: styleText.length };
  }, args.preset);

  const failed = result.assertions.filter((assertion) => !assertion.passed);
  const output = { ok: failed.length === 0, url: args.url, preset: args.preset, ...result, failedIds: failed.map((item) => item.id) };
  console.log(JSON.stringify(output, null, 2));
  if (failed.length > 0) process.exitCode = 1;
} finally {
  await browser.close();
}

function parseArgs(argv) {
  const result = {};
  for (let index = 2; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!value) throw new Error(`${flag} requires a value`);
    if (flag === "--url") result.url = value;
    else if (flag === "--preset") result.preset = value;
    else throw new Error(`Unknown argument: ${flag}`);
  }
  if (!result.url) throw new Error("Missing --url");
  if (!["authkit", "elevenlabs"].includes(result.preset)) throw new Error("--preset must be authkit or elevenlabs");
  new URL(result.url);
  return result;
}

async function launchBrowser(playwrightChromium) {
  try {
    return await playwrightChromium.launch({ headless: true });
  } catch (error) {
    const candidates = [
      process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE,
      "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
      "/Applications/Chromium.app/Contents/MacOS/Chromium",
      "/usr/bin/google-chrome",
      "/usr/bin/chromium"
    ].filter(Boolean);
    const executablePath = candidates.find((candidate) => fs.existsSync(candidate));
    if (!executablePath) throw error;
    return await playwrightChromium.launch({ headless: true, executablePath });
  }
}
