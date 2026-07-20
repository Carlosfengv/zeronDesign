#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { join } from "node:path";

const endpoint = "http://host.docker.internal:49091/release-packaging";
const root = process.env.ANYDESIGN_PACKAGER_ROOT;

if (!root) {
  process.stderr.write("ANYDESIGN_PACKAGER_ROOT is required\n");
  process.exit(2);
}

try {
  const token = readFileSync(join(root, "proxy-token"), "utf8").trim();
  const body = readFileSync(0);
  const response = await fetch(endpoint, {
    method: "POST",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body,
  });
  const output = await response.text();
  if (!response.ok) {
    throw new Error(`host packager returned ${response.status}: ${output}`);
  }
  process.stdout.write(output);
} catch (error) {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  process.exit(1);
}
