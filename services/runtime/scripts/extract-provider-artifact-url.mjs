#!/usr/bin/env node
import fs from "node:fs";

function parseArgs(argv) {
  const args = {
    project: "real-http-website",
    stage: "edit",
  };
  const requireValue = (arg, value) => {
    if (typeof value !== "string" || value.trim() === "" || value.startsWith("--")) {
      throw new Error(`${arg} requires a non-empty value`);
    }
    return value;
  };
  for (let index = 2; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = argv[index + 1];
    if (arg === "--log") {
      args.log = requireValue(arg, next);
      index += 1;
    } else if (arg === "--project") {
      args.project = requireValue(arg, next);
      index += 1;
    } else if (arg === "--stage") {
      args.stage = requireValue(arg, next);
      index += 1;
    } else {
      throw new Error(`Unknown argument: ${arg}`);
    }
  }
  if (!args.log) {
    throw new Error("Missing --log");
  }
  return args;
}

function isValidUrl(value) {
  try {
    new URL(value);
    return true;
  } catch {
    return false;
  }
}

function main() {
  const args = parseArgs(process.argv);
  const text = fs.readFileSync(args.log, "utf8");
  let artifactUrl = null;

  for (const line of text.split(/\r?\n/)) {
    if (!line.startsWith("REAL_PROVIDER_EVIDENCE ")) {
      continue;
    }
    let wrapper;
    try {
      wrapper = JSON.parse(line.slice("REAL_PROVIDER_EVIDENCE ".length));
    } catch {
      continue;
    }
    if (wrapper.project !== args.project || wrapper.stage !== args.stage) {
      continue;
    }
    const evidence = wrapper.evidence ?? {};
    if (typeof evidence.localArtifactUrl === "string" && evidence.localArtifactUrl.trim()) {
      artifactUrl = evidence.localArtifactUrl.trim();
    } else if (typeof evidence.artifactUrl === "string" && evidence.artifactUrl.trim()) {
      artifactUrl = evidence.artifactUrl.trim();
    }
  }

  if (!artifactUrl) {
    process.exitCode = 1;
    return;
  }
  if (!isValidUrl(artifactUrl)) {
    throw new Error(`Artifact URL is invalid: ${artifactUrl}`);
  }
  process.stdout.write(`${artifactUrl}\n`);
}

main();
