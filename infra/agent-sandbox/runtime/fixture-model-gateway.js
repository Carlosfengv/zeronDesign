const http = require("http");

const runs = new Map();

function tool(id, name, input = {}) {
  return { id, name, input };
}

function projectId(body) {
  return String(body.projectId || body.systemPrompt?.match(/^Project:\s*(.+)$/m)?.[1] || "");
}

function briefResponse(body, state) {
  const docs = projectId(body).toLowerCase().includes("docs");
  if (state.turn++ === 0) {
    return {
      type: "tool_calls",
      toolCalls: [
        tool("fixture-content-list", "content.list_sources"),
        tool("fixture-content-read", "content.read_source", { id: "source-1" }),
      ],
    };
  }
  return {
    type: "tool_calls",
    toolCalls: [
      tool("fixture-brief", "brief.write_draft", {
        projectType: docs ? "docs" : "website",
        audience: docs ? "developer operators" : "product teams",
        contentHierarchy: docs ? ["overview", "lifecycle"] : ["hero", "proof"],
        pageStructure: docs
          ? [{ title: "Overview", level: 1, content: "Runtime lifecycle" }]
          : [{ title: "Home", purpose: "Explain the product", keyContent: ["hero", "proof"] }],
        visualDirection: "quiet technical confidence",
        recommendedTemplate: docs ? "fumadocs-docs" : "astro-website",
        assumptions: [],
        missingInformation: [],
      }),
      tool("fixture-confirm", "brief.request_confirmation", {
        message: "Confirm this deterministic RC brief.",
      }),
    ],
  };
}

function buildResponse(body, state) {
  state.docs ||= projectId(body).toLowerCase().includes("docs");
  const turn = state.turn++;
  if (turn === 0) {
    return {
      type: "tool_calls",
      toolCalls: [
        tool("fixture-init", "project.init", {
          template: state.docs ? "fumadocs-docs" : "astro-website",
        }),
      ],
    };
  }
  const docs = state.docs;
  const buildScript = docs
    ? "const fs=require('fs');fs.mkdirSync('out',{recursive:true});fs.writeFileSync('out/index.html','<!doctype html><style>body{font:40px sans-serif;background:#fff;color:#111}</style><h1>RC Docs</h1><a href=\"/docs\">Overview</a>');fs.writeFileSync('out/docs.html','<h1>RC Docs Overview</h1>');"
    : "const fs=require('fs');fs.mkdirSync('dist',{recursive:true});fs.writeFileSync('dist/index.html','<!doctype html><style>body{font:48px sans-serif;background:#fff;color:#111}</style><h1>RC Website</h1>');";
  const calls = [
    tool("fixture-package", "fs.write", {
      path: "project/package.json",
      text: '{"scripts":{"build":"node build.cjs"}}',
    }),
    tool("fixture-build-script", "fs.write", { path: "project/build.cjs", text: buildScript }),
  ];
  if (docs) {
    calls.push(tool("fixture-docs-source", "fs.write", {
      path: "project/content/docs/index.mdx",
      text: "---\ntitle: Overview\n---\n\n# RC Docs Overview",
    }));
  }
  if (turn === 1) {
    return { type: "tool_calls", toolCalls: calls };
  }
  if (turn === 2) {
    return {
      type: "tool_calls",
      toolCalls: [
        tool("fixture-dependency", "project.ensure_dependencies", {
          mode: "add",
          packages: ["is-number@7.0.0"],
          cwd: "project",
        }),
      ],
    };
  }
  return {
    type: "tool_calls",
    toolCalls: [
    tool("fixture-build", "project.build", { cwd: "project" }),
    tool("fixture-preview", "preview.start"),
    tool("fixture-open", "browser.open", { url: "http://127.0.0.1:4321" }),
    tool("fixture-shot", "browser.screenshot", { screenshotId: docs ? "rc-docs" : "rc-website" }),
    tool("fixture-promote", "preview.report_candidate", {
      url: "http://127.0.0.1:4321",
      screenshotId: docs ? "rc-docs" : "rc-website",
    }),
    tool("fixture-complete", "run.complete", {
      status: "completed",
      summary: `${docs ? "Docs" : "Website"} deployed Runtime RC gate complete`,
    }),
    ],
  };
}

function editResponse(body, state) {
  state.docs ||= projectId(body).toLowerCase().includes("docs");
  const docs = state.docs;
  const buildScript = docs
    ? "const fs=require('fs');fs.mkdirSync('out',{recursive:true});fs.writeFileSync('out/index.html','<!doctype html><style>body{font:40px sans-serif;background:#fff;color:#111}</style><h1>RC Docs Edited</h1><a href=\"/docs\">Overview</a>');fs.writeFileSync('out/docs.html','<h1>RC Docs Overview Edited</h1>');"
    : "const fs=require('fs');fs.mkdirSync('dist',{recursive:true});fs.writeFileSync('dist/index.html','<!doctype html><style>body{font:48px sans-serif;background:#fff;color:#111}</style><h1>RC Website Edited</h1>');";
  return {
    type: "tool_calls",
    toolCalls: [
      tool("fixture-edit-script", "fs.write", { path: "project/build.cjs", text: buildScript }),
      tool("fixture-edit-build", "project.build", { cwd: "project" }),
      tool("fixture-edit-preview", "preview.start"),
      tool("fixture-edit-open", "browser.open", { url: "http://127.0.0.1:4321" }),
      tool("fixture-edit-shot", "browser.screenshot", { screenshotId: docs ? "rc-docs-edit" : "rc-website-edit" }),
      tool("fixture-edit-promote", "preview.report_candidate", {
        url: "http://127.0.0.1:4321",
        screenshotId: docs ? "rc-docs-edit" : "rc-website-edit",
      }),
      tool("fixture-edit-complete", "run.complete", {
        status: "completed",
        summary: `${docs ? "Docs" : "Website"} deployed Runtime RC edit complete`,
      }),
    ],
  };
}

http.createServer((request, response) => {
  if (request.method !== "POST" || request.url !== "/v1/agent/turn") {
    response.writeHead(request.url === "/health" ? 200 : 404);
    response.end(request.url === "/health" ? "ok" : "not found");
    return;
  }
  let raw = "";
  request.on("data", chunk => raw += chunk);
  request.on("end", () => {
    try {
      const body = JSON.parse(raw);
      const state = runs.get(body.runId) || { turn: 0, docs: false };
      const payload = body.phase === "brief"
        ? briefResponse(body, state)
        : body.phase === "edit"
          ? editResponse(body, state)
          : buildResponse(body, state);
      runs.set(body.runId, state);
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify(payload));
    } catch (error) {
      response.writeHead(400, { "content-type": "application/json" });
      response.end(JSON.stringify({ error: String(error) }));
    }
  });
}).listen(9000, "0.0.0.0");
