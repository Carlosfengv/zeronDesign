---
date: 2026-07-09
status: in-progress
type: implementation-plan
topic: runtime-harness-edit-loop-reliability
related:
  - ./2026-07-08-project-lifecycle-generation-edit-build-plan.md
  - ./2026-07-08-runtime-api-freeze.md
  - ./2026-07-07-runtime-harness-fix-plan.md
  - ./2026-07-07-tool-call-json-write-protocol-fix-plan.md
reference:
  - /Users/carlos/Downloads/claude-code-main
---

# Runtime Harness Edit Loop Fix Plan

## 1. Goal

This document is the execution baseline for making the real runtime API support
a complete Website and Docs lifecycle:

```text
create
  -> generate
  -> build
  -> promote
  -> prompt edit
  -> rebuild
  -> promote next version
```

The current phase is not DesignProfile. The target is a reliable runtime-owned
creation, generation, edit, build, screenshot, candidate, and promotion loop.
The outputs must support styled Website and Docs projects using Tailwind CSS
and local shadcn/Base UI style primitives, but DesignProfile extraction,
matching, storage, and retrieval are out of scope.

## 2. Current Status

The original plan is no longer purely proposed. Several P0 to P3 items have
already been implemented in the runtime. The remaining risk is now concentrated
in provider-backed real runtime API E2E verification and browser-computed style
evidence on live promoted artifacts.

This document should be read as an implementation baseline, not as proof that
the product loop is release-ready. Local unit and integration tests prove the
harness contracts. Release readiness still requires live provider evidence for
Website and Docs generation/editing through the public runtime API.

| Area | Status | Notes |
| --- | --- | --- |
| Dependency restore before build | Implemented | `project.build` can restore dependencies after source snapshot restore and records dependency restore metadata. |
| Package-manager-aware build | Implemented | Build command is selected from project state, lockfiles, and profile defaults. |
| Screenshot-required candidates | Implemented | `preview.report_candidate` requires screenshot evidence and rejects blank screenshots before candidate creation. |
| Workspace path normalization | Implemented for `fs.*` plus PreToolUse `cwd/path` fields | `/workspace/...` virtual paths are normalized before validation/permission for common tool path fields; `/` stays denied. |
| Nested package root guard | Implemented | Build/Edit/Repair write paths reject nested `package.json` creation or patching under the app root. |
| Patch stability | Implemented | `fs.patch.replaceAll`, read hash tracking, stale edit rejection, and `fs.multi_patch` exist. |
| Runtime style contract | Implemented | Website and Docs templates emit token files, `state/style-contract.json`, Tailwind imports, and local UI primitives. |
| Structured style edits | Implemented | `style.update_tokens` edits declared token files and returns typed `style.*` metadata for contract, token, and token-file failures. |
| High-level lifecycle tools | Implemented | `project.inspect`, `project.ensure_dependencies`, and `preview.publish` are available and referenced by prompts/policies. |
| Public runtime state | Expanded | `/projects/{projectId}/runtime-state` exposes edit anchors plus style contract, latest build, dependency, and preview state when available, including Kubernetes workspace-channel reads and PhaseA/global workspace fallback. |
| Typed recoverable errors | Covered for documented matrix | The documented path, patch, build, preview, shell, and pre-tool phase failures expose stable `errorKind` metadata and have local regression assertions. |
| Hook architecture | Core implemented | `PreToolUseHook`, `PostToolUseFailureHook`, and `PostToolUseSuccessHook` are extracted into reusable hook code and integrated with the agent loop event stream. |
| Real runtime API E2E | Provider-executed, green | Website and Docs build/edit now pass through the public runtime APIs with real provider evidence. The current green evidence is `.runtime-evidence/provider-20260709-180531/evidence-summary.json`; it covers Website build, Website edit, Docs build, Docs edit, edit text assertions, source snapshot transitions, promoted artifact serving, and browser computed-style verification for the orange theme token. |

Latest local verification on 2026-07-09:

- `bash services/runtime/scripts/run-runtime-harness-local-gates.sh` passes end to
  end.
- The local gate covers formatting, runtime tool contracts, preview gates, tool
  permission integration, agent loop behavior, full `http_api`, template build
  agents, shared package tests/typecheck, script syntax, provider wrapper dry-run
  paths, the real Fumadocs production build smoke, computed-style local artifact
  smoke, and `git diff --check`.
- The final external gate has been captured with provider-backed Website and
  Docs build/edit evidence after the TypeScript pin, plus computed-style
  verification against the promoted Website edit artifact.

Release gate summary:

- Green locally: runtime tool contracts, hook behavior, preview gates, shared API
  schemas, and style-token update mechanics.
- Green with real provider evidence: Website build/edit, Docs build/edit, edit
  artifact text assertions, source snapshot/version transitions, promoted
  artifact serving, and browser-computed style evidence.
- Product integration can consume this as the current lifecycle baseline, with
  `.runtime-evidence/provider-20260709-180531/evidence-summary.json` attached to
  the execution record.

## 3. Field Evidence

Recent real runtime API tests exposed repeated failure patterns.

### 3.1 Monad Theme Edit

Project:

```text
monad-design-website-api-1783524832
```

Result:

- Theme color edit eventually succeeded and promoted a new version.
- The success path included repeated recoverable tool failures.

Observed failures:

- `fs.patch oldStr found multiple times`
- `fs.patch oldStr not found`
- shell commands using `/workspace/project/...` instead of valid runtime paths
- `ExternalDirectory("/")` path boundary denial
- denied `grep` usage instead of `fs.search`
- first `project.build` failed with `astro: command not found`
- one candidate was created without valid screenshot evidence and could not be
  promoted

### 3.2 Second Website Theme Edit

Project:

```text
second-website-edit-test-1783526065
```

Result:

- Theme changed from green to purple and promoted successfully.
- The same path and dependency failures recurred.
- Patch ambiguity did not recur because the generated page used centralized CSS
  variables.

Observed failures:

- `/workspace/project` path misuse
- `ExternalDirectory("/")`
- `find` shell errors
- `sh` denied
- first `project.build` failed with `astro: command not found`
- `package.install(mode=restore)` fixed the missing dependency state

Important non-failure:

- No `fs.patch oldStr not found` or `oldStr found multiple times` occurred when
  theme values were centralized in CSS variables.

### 3.3 Design Markdown Style Generation

When generating from a design markdown attachment, the weak output was not best
explained as a missing attachment context problem. The stronger explanation is
that the old generation path treated style instructions as prompt guidance
instead of a runtime contract.

Observed risk:

- the model could mention Tailwind or component primitives without producing a
  durable Tailwind import, token file, or local component layer;
- visual instructions could be scattered into page-specific literals, making
  prompt edits fragile;
- the product had no public style contract to inspect before editing;
- screenshot presence alone could not prove that the intended token/theme was
  actually loaded by the browser.

Required interpretation:

- If the source lacks `state/style-contract.json`, token CSS, and a style import
  consumed by the entry page/layout, treat the failure as a runtime contract gap.
- If those files exist but computed browser values do not match the contract,
  treat it as a style loading or CSS cascade failure.
- If the files and computed values match but the visual design quality is still
  weak, treat it as model/design-quality work, not harness correctness.

### 3.4 Provider-Backed Lifecycle Evidence

Evidence folders:

```text
.runtime-evidence/provider-20260709-135815
.runtime-evidence/provider-20260709-140244
.runtime-evidence/provider-20260709-140938
.runtime-evidence/provider-20260709-142128
.runtime-evidence/provider-20260709-143804
.runtime-evidence/provider-20260709-145122
.runtime-evidence/provider-20260709-150747
.runtime-evidence/provider-20260709-151203
```

Result:

- Website build completed against the real provider.
- After the context hydration fix, Website edit completed and promoted an
  artifact containing the requested literal `TESTXXX`.
- Docs build did not complete in the latest full matrix. After increasing the
  stage timeout, dependency installation completed, but `provider-20260709-145122`
  showed the Docs workspace contained Astro website artifacts such as
  `src/pages/index.astro`, `astro.config.mjs`, and an Astro tsconfig while also
  containing Fumadocs `app/` files. Next then failed with the incompatible
  `pages` plus `app` routing-root error and the run reached max turns as
  `partial`.
- After PhaseA project-scoped workspace isolation, `provider-20260709-150747`
  got further: Website build completed and Website edit successfully patched the
  hero, updated tokens, and promoted `version-149` with `preview.publish`.
  However the edit run then attempted post-publish shell verification using
  virtual `/workspace/project/...` argv paths. In local PhaseA mode those
  absolute paths did not exist, producing repeated `shell.non_zero_exit` and a
  final `partial` status before the test could emit edit evidence.
- After local shell argv virtual-path mapping, `provider-20260709-151203`
  made Website build and Website edit fully green. Website build `run-7`
  completed, Website edit `run-106` completed, promoted `version-149`, and the
  served artifact contained the requested literal `TESTXXX 标题内容`.
- In the same `provider-20260709-151203` matrix, Docs build `run-185` reached
  dependency installation successfully, then failed in `project.build` with
  typed `errorKind=build.failed`. The captured stderr points at the
  Fumadocs/Next build chain rather than the previous workspace contamination:
  `fumadocs-mdx/dist/load-from-file-*.js` emitted webpack dynamic-import cache
  warnings and Next terminated with `The "id" argument must be of type string.
  Received undefined`.
- The corresponding `evidence-summary.json` marks the Docs stage incomplete:
  missing Docs build evidence, missing `preview.candidate`, missing
  `preview.updated`, `run.completed status is not completed: partial`, and
  missing Docs edit stage.
- Local reproduction isolated the root cause to the template dependency pin:
  the generated package allowed `typescript: latest`, which resolved to
  `7.0.2` in the provider workspace. Next still attempted to install
  TypeScript during build and then failed with `id argument undefined`. In a
  copied provider workspace, changing only TypeScript to `5.9.3` made the same
  project complete `next build --webpack` successfully.
- Docs edit did not start because Docs build did not complete.

Observed failures:

- duplicate `preview.candidate` and `preview.updated` events in the first run;
- edit timeout without enough stream visibility before the timeout-stream patch;
- a manual `preview.report_candidate` after `preview.publish`, now surfaced as
  recoverable `preview.already_promoted`;
- recoverable `content.read_source` and `fs.read` failures that needed stable
  `metadata.errorKind` values;
- the edit model used existing project files but ignored the `/continue` user
  message containing `TESTXXX 标题内容`;
- Docs dependency installation timed out without a stable error kind;
- Docs `project.build` failed after missing dependencies with no stable error
  kind in earlier evidence;
- `shell.run` non-zero exit and `fs.list` missing-directory failures surfaced as
  recoverable errors without stable `metadata.errorKind` in earlier reruns;
- PhaseA public runs were using the global runtime workspace root for agent
  sessions, so separate `projectId` values could still write into the same
  `project/` app root. This allowed Website Astro artifacts to contaminate the
  later Docs build;
- local PhaseA `shell.run` accepted argv values containing `/workspace/...`, but
  executed them on the host without mapping the virtual path to the actual
  project-scoped workspace root.
- real Fumadocs Docs source could now install dependencies and invoke Next, but
  the generated/template build path still failed before preview/candidate
  creation. This is a separate template/build-chain defect, not the old
  `pages` plus `app` contamination.
- the Docs template used an unstable `typescript: latest` dependency. Because
  the template contains `source.config.ts` and generated `.source/*.ts` files,
  Next's TypeScript phase is part of the production build path. The version must
  be pinned to a Next-compatible TypeScript release rather than whatever npm
  considers latest at runtime.
- After the project-scoped PhaseA workspace change, several local tests initially
  failed because their fixtures still assumed the old global workspace layout or
  old app-root path semantics. These were test drift, not new runtime regressions:
  staged write cleanup needed PhaseA config, edit restore snapshots needed to live
  under `workspace_root/{projectId}/outputs/...`, and tool-permission audit
  coverage needed to operate under `project/custom-app` instead of treating the
  app root itself as a deletable file target.

Current diagnosis:

- This was a real context hydration bug, not only a weak style prompt and not
  only CSS loading. `/runs/{runId}/continue` persisted the user message in the
  project conversation, but `AgentLoop::run` initialized the model
  `message_window` from checkpoints only. A queued edit run with no checkpoint
  therefore started without the user acceptance text.
- The runtime must inject run-scoped, user-visible `user_message` conversation
  items into the model context before each model turn. This also covers a
  running run that receives a queued continue message after tool interruption.
- A deterministic regression test must assert that the model request includes
  the exact continue text, including quoted or non-ASCII title content.
- Dependency install timeout and build failure paths must return typed
  recoverable metadata so the real-provider evidence summary can distinguish
  harness failures from expected recovery paths.
- PhaseA public runs must execute tools under
  `workspace_root/{projectId}` rather than the global `workspace_root`, while
  Kubernetes mode keeps `/workspace` because isolation comes from sandbox/PVC
  binding.
- Local PhaseA command execution must also translate virtual `/workspace/...`
  argv path arguments to the actual workspace root. Path field normalization
  alone is insufficient because model verification commands often pass
  workspace paths inside `argv`.
- Tests for PhaseA local behavior must construct fixtures in the same effective
  workspace root the runtime uses. For public PhaseA sessions this means
  `workspace_root/{projectId}`, while Kubernetes-mode tests should continue to
  use the configured workspace root.
- Docs generation now has a stricter template/source contract guard so the
  model cannot convert a valid Fumadocs app-router scaffold into an invalid
  hybrid `pages` plus `app` Next project during build. Template initialization
  also clears conflicting prior-template files before writing the new scaffold.
  These fixes are locally covered and were confirmed to move the provider
  matrix past the earlier contamination failure.
- The next Docs fix must target a real build smoke path for the Fumadocs
  scaffold itself. A fake command backend or route-root guard is no longer
  enough; the contract needs to prove `project.init(template=fumadocs-docs)` plus
  representative generated MDX can run `npm run build` successfully before the
  provider loop asks the model to edit/publish Docs.
- Implemented fix: both runtime template writers now pin `typescript` to
  `5.9.3`, the scaffold tests assert that pin, and
  `services/runtime/scripts/smoke-fumadocs-docs-build.sh` runs an ignored
  integration smoke that initializes a real `fumadocs-docs` project, installs
  npm dependencies, writes representative MDX pages, and invokes runtime
  `project.build` to run the actual production build, write build metadata, and
  create a source snapshot.

## 4. Diagnosis

The problem was not simply lost model context and not simply missing CSS. After
provider execution, it is clearer that the failure classes are layered:

- context hydration decides whether the model sees the user's latest edit
  intent at all;
- runtime style contracts decide whether generated styling is durable and
  editable;
- browser-computed evidence decides whether the promoted artifact actually loads
  the expected CSS/token state.

The latest provider evidence did include a concrete lost-context bug for edit
runs: run-scoped `/continue` user messages were stored in conversation history
but not included in `message_window` for the resumed model turn.

The prompt already told the model to use relative workspace paths, read
`state/project.json`, call `package.install`, use `project.build`, use
`preview.start`, report candidates, and only finish after promotion. Real model
behavior still violated those rules.

The reliable fix is therefore to move lifecycle correctness out of prompt text
and into runtime-owned contracts:

- tool input validation;
- path normalization;
- dependency recovery;
- read-before-edit state;
- stale write protection;
- screenshot and promotion gates;
- structured recoverable errors;
- high-level lifecycle tools;
- reusable pre/post tool hooks.

The model should decide the product intent. The runtime should own execution
correctness.

For styling specifically, the harness must separate three concerns:

- Intent capture: the model reads user prompt or design markdown and chooses the
  appropriate visual direction.
- Style contract: the runtime creates a durable Tailwind/CSS-token/component
  surface that future edits can inspect and modify structurally.
- Evidence: browser-computed style checks prove that promoted artifacts load the
  expected tokens after generation and after edits.

This means "use Tailwind and shadcn/Base UI" is not an adequate prompt-only
requirement. It must be reflected in generated files, runtime state, tool
contracts, and verification.

## 5. Claude Code Reference Takeaways

The useful pattern from `/Users/carlos/Downloads/claude-code-main` is not its UI
or terminal surface. The useful pattern is that tools are stateful execution
units, not plain functions.

Relevant concepts:

- input schema and validation;
- path normalization;
- read/write permission checks;
- pre-tool hooks;
- post-tool failure hooks;
- read-before-edit state;
- stale write protection;
- tool summaries;
- context modifiers for future tool calls;
- concurrency classification;
- structured recovery messages.

The matching AnyDesign harness shape is:

```text
model intent
  -> runtime tool contract
  -> preflight normalization / validation
  -> controlled tool execution
  -> structured result or recovery guidance
  -> lifecycle state update
```

## 6. Implemented Fixes

### 6.1 Dependency Restore Is Runtime-Owned

Edit runs restore source snapshots, and those snapshots intentionally exclude
`node_modules`. The runtime now treats this as lifecycle state instead of asking
the model to infer the fix.

Implemented behavior:

- edit restore writes dependency state;
- `project.build` detects missing dependencies or missing framework binaries;
- restore runs through the runtime package install executor;
- build output records package manager and restore metadata;
- failure returns a recoverable build/dependency error with diagnostics.

Required invariant:

- After a source snapshot restore, the first model call to `project.build`
  should not fail merely because `astro`, `next`, or another project binary is
  missing from `node_modules`.

### 6.2 Path Mistakes Are Normalized Or Typed

The model repeatedly produced `/workspace/project` and `/workspace` paths. This
is understandable because runtime results expose virtual workspace paths, but
raw host absolute paths must not be accepted.

Implemented behavior:

- `/workspace/project/src/x` is normalized to `project/src/x`;
- `/workspace/outputs/x` is normalized to `outputs/x`;
- `workspace/project/x` is normalized to `project/x`;
- bare `/` remains denied;
- normalized paths still pass canonical workspace boundary checks;
- external directory and invalid component errors return typed recoverable
  metadata;
- `PreToolUseHook` normalizes virtual workspace `cwd` and `path` fields before
  tool schema validation, permission checks, and audit recording, so this
  protection applies to `shell.run`, build/preview lifecycle tools, and future
  tools that use the same field names.

Required invariant:

- Virtual workspace paths should not derail `fs.read`, `fs.search`, `fs.patch`,
  or `fs.multi_patch`.
- Virtual workspace paths should not derail non-`fs.*` tools when they appear
  in standard `cwd` or `path` fields.
- Host absolute paths and escape attempts must remain blocked.

### 6.2.1 Nested Package Roots Are Denied

Model-driven scaffold and repair attempts can accidentally create nested app
roots such as `project/src/package.json`. That breaks package-manager
selection, dependency restore, build cwd selection, and source snapshot
semantics.

Implemented behavior:

- `fs.write` and `fs.commit_chunks` reject nested `package.json` writes under
  the active `appRoot`;
- `fs.patch` and `fs.multi_patch` reject editing an existing nested
  `package.json`;
- the allowed package manifest remains the app root package file, for example
  `project/package.json`;
- violations return typed recoverable metadata with
  `errorKind=path.nested_package_root`;
- agent-loop recovery guidance treats repeated nested-root attempts as a
  recoverable path failure.

Required invariant:

- Build/Edit/Repair runs should not create or mutate package roots below the
  active app root.
- Legitimate edits to the app root `package.json` remain possible through
  controlled runtime tools and package/dependency policy.

### 6.3 Candidate Creation Requires Evidence

`preview.report_candidate` previously allowed URL-only candidates that could not
be promoted later because screenshot evidence was missing.

Implemented behavior:

- Website and Docs candidate reporting requires `screenshotId`;
- the screenshot metadata file must exist;
- blank screenshots are rejected;
- source snapshot and latest build state are checked before candidate creation;
- no invalid candidate is created for missing evidence.

Compatibility note:

- Shared API fields may remain optional for older clients, but runtime tool
  behavior is stricter in build/edit phases.

### 6.4 Patch Editing Is Safer

Large generated pages often repeat color literals, classes, and button blocks.
Short `oldStr` patches are fragile.

Implemented behavior:

- `fs.patch` supports `replaceAll`;
- `fs.multi_patch` applies multiple edits atomically after one read;
- `fs.read` records content hashes;
- stale reads are rejected before writing;
- patch failures include or are being migrated to structured metadata such as
  `patch.old_str_missing`, `patch.old_str_ambiguous`, and `patch.stale_read`.

Required invariant:

- The model should be guided toward reading again or using a broader unique
  snippet, rather than repeatedly retrying stale or ambiguous patches.

### 6.5 Styling Uses Runtime Tokens

The second theme test showed that centralized CSS variables make edits reliable.
The runtime now emits a minimal style contract without introducing
DesignProfile.

Implemented Website contract:

```text
project/src/styles/tokens.css
project/src/styles/global.css
project/src/components/ui/
state/style-contract.json
```

Implemented Docs contract:

```text
project/app/tokens.css
project/app/global.css
project/components/ui/
state/style-contract.json
```

Implemented behavior:

- Tailwind v4 is imported through CSS;
- runtime tokens are declared as CSS variables;
- local UI primitives consume runtime tokens;
- `style.update_tokens` edits declared tokens through a structured tool;
- unknown token names are rejected unless declared in the style contract;
- style edit failures return stable `style.*` `errorKind` metadata so the
  agent loop and product UI do not need to parse human error text.

Required invariant:

- A theme edit should update token files or a declared style layer, not dozens
  of scattered page literals.

### 6.6 High-Level Runtime Tools Exist

The runtime now exposes higher-level tools so the model does not need to compose
the lifecycle manually every time.

Implemented tools:

- `project.inspect`
- `project.ensure_dependencies`
- `style.update_tokens`
- `preview.publish`

Prompt and policy have been updated so normal generation/edit flows prefer
these tools. Lower-level tools remain available for diagnostics.

## 7. Baselines And Remaining Fixes

### 7.1 Typed Recoverable Metadata Coverage

Typed errors should be the machine-readable contract between tools, hooks, the
agent loop, tests, and product UI.

Required shape:

```json
{
  "error": "human-readable message",
  "errorKind": "path.external_directory",
  "recoverable": true,
  "receivedPath": "/",
  "suggestedPath": "project",
  "suggestedAction": "Use workspace-relative paths such as project/src/pages/index.astro."
}
```

Documented error kinds now covered by local assertions:

- `path.external_directory`
- `path.invalid_component`
- `path.secret`
- `fs.read_failed`
- `fs.list_failed`
- `content.source_missing`
- `patch.read_required`
- `path.nested_package_root`
- `style.input_invalid`
- `style.contract_missing`
- `style.contract_invalid`
- `style.token_unknown`
- `style.token_value_invalid`
- `style.token_file_unavailable`
- `style.token_file_invalid`
- `style.token_variable_missing`
- `style.token_variable_ambiguous`
- `dependency.install_timeout`
- `dependency.install_failed`
- `docs.routing_root_forbidden`
- `docs.source_contract_invalid`
- `patch.stale_read`
- `patch.old_str_missing`
- `patch.old_str_ambiguous`
- `build.missing_dependency`
- `build.timeout`
- `build.failed`
- `preview.screenshot_missing`
- `preview.screenshot_invalid`
- `preview.screenshot_blank`
- `preview.build_missing`
- `preview.build_failed`
- `preview.source_snapshot_missing`
- `preview.source_snapshot_mismatch`
- `preview.already_promoted`
- `preview.dist_missing`
- `shell.command_denied`
- `shell.non_zero_exit`

Acceptance:

- Each repeated real-world failure returns a stable `errorKind`.
- Tests assert the metadata, not only the human-readable message.
- Permission-denied security events are still emitted for secret paths and
  external paths where required.

Current regression evidence:

- `services/runtime/tests/sandbox_tools.rs` asserts `path.*`, `patch.*`,
  `style.*`, `build.missing_dependency`, nested package-root denial, and
  `shell.command_denied`;
- `services/runtime/tests/preview_promotion.rs` asserts the full preview gate
  matrix: screenshot missing/invalid/blank, build missing/failed, source
  snapshot missing/mismatch, and dist missing;
- `services/runtime/tests/tool_permissions_integration.rs` asserts
  `tool.phase_forbidden` from PreToolUse phase gating, virtual `cwd`
  normalization before permission/audit, and dedicated-tool redirects for
  dependency and preview shell bypass attempts;
- `services/runtime/tests/agent_loop.rs` asserts repeated typed recoverable
  failures trigger recovery guidance and partial-stop behavior.

Remaining watchpoint:

- new tool failure classes must add `errorKind` metadata and tests before being
  treated as recoverable in the agent loop.

### 7.2 Hook Architecture Is Now The Runtime Baseline

Hook-like behavior should no longer be treated as incidental logic inside
individual tools. The runtime now has a reusable hook layer that records both
failure recovery decisions and successful lifecycle state changes in the event
stream.

Current implementation:

- `services/runtime/src/agent_hooks.rs` contains a reusable
  `PreToolUseHook`, `PostToolUseFailureHook`, and `PostToolUseSuccessHook`;
- PreToolUse rejects invalid phase/tool combinations such as workspace tools
  during Brief and `brief.*` write tools during Build/Edit before schema
  validation or permission checks;
- PreToolUse injects default `cwd=appRoot` for Build/Edit/Repair execution
  tools when the model omits `cwd`, so permission checks and audit records see
  explicit workspace intent;
- PreToolUse normalizes virtual `/workspace/...` and `workspace/...` values in
  standard `cwd` and `path` fields before schema validation, permission checks,
  execution, and audit recording;
- PreToolUse redirects dependency install/add attempts made through `shell.run`
  to `project.ensure_dependencies` before permission execution;
- PreToolUse redirects preview server and interactive scaffold attempts made
  through `shell.run` to `preview.start` or `project.init` before permission
  execution;
- repeated recoverable failures are fingerprinted by `tool`, `errorKind`, phase,
  and normalized path;
- `path.*`, `patch.*`, `style.*`, `build.missing_dependency`, `preview.*`,
  and `shell.command_denied` are recognized by the agent loop recovery guard;
- the loop emits `tool.recovery_suggested` and generic retry metrics before
  stopping identical repeated failures as partial;
- input-size errors keep the existing compatibility metric
  `tool_input_retry_same_large_write`;
- PostToolUseSuccess classifies successful lifecycle effects and merges
  `postToolUseSuccess` metadata into `ToolCompleted` events and conversation
  tool-result metadata.

Implemented hook layers:

#### PreToolUse

Current responsibilities:

- reject invalid phase/tool combinations;
- normalize virtual workspace paths in standard `cwd` and `path` fields before
  downstream validation and permission checks;
- inject default `cwd=appRoot` for `shell.run`, `project.build`,
  `project.ensure_dependencies`, `package.install`, and `preview.publish` when
  `cwd` is omitted;
- reject `shell.run` dependency install/add commands with
  `shell.command_denied` and a `project.ensure_dependencies` suggestion;
- reject `shell.run` preview server commands with `shell.command_denied` and a
  `preview.start` suggestion;
- reject `shell.run` interactive scaffold commands with `shell.command_denied`
  and a `project.init` suggestion;
- return typed `tool.phase_forbidden` metadata with `recoverable=true`;
- write deny audit events before schema or permission execution;
- preserve semantically equivalent tool input when no rejection is needed,
  except for deterministic virtual path normalization and default `cwd`
  injection.

Next refinements:

- extend virtual workspace path normalization to additional field names only
  when a new tool contract introduces them;
- extend dedicated-tool redirects as new shell bypass patterns are observed.

#### PostToolUseFailure

Current responsibilities:

- classify failures by `errorKind`;
- append targeted recovery guidance;
- stop repeated identical attempts after a threshold;
- expose recovery decisions in the event stream.

Next refinements:

- trigger runtime-owned recovery when safe, such as dependency restore before a
  known build retry;
- expand coverage audits for all P0 observed failure paths.

#### PostToolUseSuccess

Current responsibilities:

- update read state after `fs.read`;
- update dependency state after `package.install` or `project.ensure_dependencies`;
- update build state after `project.build`;
- update style contract state after `style.update_tokens`;
- update preview/browser state after `preview.start` and `browser.open`;
- update screenshot state after `browser.screenshot`;
- update promotion state after `preview.report_candidate` or `preview.publish`.

Acceptance:

- Hook behavior is reusable across tools rather than duplicated in each tool.
- Repeated identical failures are detected and interrupted.
- The event stream can explain what recovery was suggested, what retry guard
  fired, and which successful tool updated lifecycle state.

### 7.3 Add Real Runtime API Regression Suite

Unit tests prove tool behavior, but they do not prove the product lifecycle.
The required product validation uses real runtime APIs and a real provider; the
current green capture is `.runtime-evidence/provider-20260709-180531`.

Current implementation:

- `services/runtime/tests/http_api.rs` includes the ignored test
  `real_provider_public_runtime_website_and_docs_lifecycle_matrix`;
- `services/runtime/scripts/run-real-provider-http-lifecycle-e2e.sh` runs that
  test with `--ignored --nocapture`;
- when `RUNTIME_E2E_LOG_DIR` is set, the runner writes
  `run-metadata.env`, `provider-lifecycle.log`, `evidence-summary.json`,
  and/or `computed-style.log` without recording secret values;
- `run-metadata.env` records the computed-style target fields
  `styleProject` and `styleStage` in addition to selector/property/expected
  values, so an evidence folder can be audited without reconstructing the shell
  environment;
- `services/runtime/scripts/summarize-real-provider-evidence.mjs` validates that
  provider logs contain Website/Docs build/edit streams plus structured
  runtime-state, preview, version/snapshot, artifact, style-contract, build,
  and dependency evidence, and can merge browser computed-style verification
  into the same summary;
- by default, the evidence summary requires the full product matrix:
  `real-http-website` and `real-http-docs` across `build` and `edit`. When
  `--project` or `--stage` is supplied, that flag replaces the default required
  scope rather than appending to it; repeat the flag to validate multiple
  explicit projects or stages. This keeps full-matrix product evidence strict
  while allowing focused reruns such as `--project real-http-website --stage
  edit` to validate only the intended lifecycle slice. Summary log/output,
  filter, and computed-style target flags must include explicit non-empty
  values;
- `services/runtime/scripts/verify-computed-style.mjs` verifies browser
  `window.getComputedStyle` values for a supplied artifact or preview URL and
  treats browser-computed `rgb(...)` values as equivalent to expected hex colors
  where appropriate. Its CLI rejects missing or blank URL, selector, property,
  expected-value, and timeout arguments before launching the browser, allows CSS
  custom property names such as `--runtime-primary` as `--property` values, and
  requires the URL to be parseable. For local `file://` artifacts it reads the
  generated HTML, inlines root-relative `/_astro/*.css` and `/_next/*.css` files
  from the same artifact root after confirming the resolved file remains inside
  that root, then runs the browser check. This prevents provider
  `localArtifactUrl` evidence from failing merely because the local file URL
  bypasses the runtime artifact server's `/artifacts/{projectId}/current/...`
  HTML rewrite. Both success and failure paths emit JSON so the evidence summary
  can preserve the observed value;
- `services/runtime/scripts/smoke-computed-style-artifact.sh` creates a local
  artifact fixture with runtime CSS tokens, runs the style-only evidence path,
  and writes `computed-style.log` plus `evidence-summary.json` without requiring
  a provider key;
- provider evidence now includes `localArtifactUrl` for promoted build/edit
  artifacts, and `services/runtime/scripts/extract-provider-artifact-url.mjs`
  can select the configured project/stage artifact URL from
  `REAL_PROVIDER_EVIDENCE` lines while rejecting missing flag values and
  non-parseable artifact URLs;
- `services/runtime/scripts/run-runtime-harness-local-gates.sh` runs the
  repeatable local gate bundle: formatting, `agent_hooks`, permission engine,
  runtime tool tests, template build tests, script syntax checks,
  evidence-summary tests, the real Fumadocs build smoke, the full `http_api`
  suite, the local computed-style artifact smoke, a direct CSS custom-property
  computed-style smoke, and whitespace checks;
- `services/runtime/scripts/run-runtime-harness-provider-gates.sh` is the
  provider-ready wrapper: it loads provider env/key files, fails fast when
  `DEEPSEEK_API_KEY` is missing, supports a dry-run mode that validates key
  loading and evidence settings without running local gates or calling the
  provider, writes a safe dry-run `run-metadata.env` without secret values,
  includes the configured artifact URL and computed-style target in both
  terminal output and metadata, optionally runs local gates for non-dry provider
  executions, creates a timestamped provider evidence directory, and then
  delegates to the real provider lifecycle runner with consistent evidence
  settings;
- the provider wrapper can load secrets without putting them on the command
  line: set `RUNTIME_E2E_ENV_FILE` to source a local env file, or set
  `DEEPSEEK_API_KEY_FILE` to read a file containing only the provider key. It
  also supports `RUNTIME_E2E_DRY_RUN=1` for validating key loading, evidence
  directory creation, and computed-style target selection without calling the
  provider;
- after provider lifecycle execution, `run-real-provider-http-lifecycle-e2e.sh`
  resolves `RUNTIME_E2E_ARTIFACT_URL` automatically from
  `REAL_PROVIDER_EVIDENCE.localArtifactUrl` when the caller did not provide one,
  then runs browser computed-style verification against that local artifact URL;
- the test drives public runtime APIs for Website and Docs build/edit flows and
  asserts version changes, source snapshot changes, preview promotion, artifact
  serving, and `preview.updated` before `run.completed`;
- `AgentLoop::run` now hydrates run-scoped, user-visible `user_message`
  conversation items into the model `message_window` before every turn, so
  `POST /runs/{runId}/continue` acceptance criteria are visible to queued edit
  runs and to running runs after an interruption;
- `project.ensure_dependencies` timeout/start/status failures now return typed
  dependency `errorKind` metadata, and `project.build` classifies missing
  commands/dependencies, timeouts, and generic build failures with stable
  `build.*` metadata;
- PhaseA public run sessions now use a project-scoped local workspace root
  derived from `projectId`; Kubernetes sessions keep the configured `/workspace`
  root because sandbox/PVC binding provides isolation there;
- `project.init` removes conflicting prior-template artifacts before writing a
  new scaffold, so switching from `astro-website` to `fumadocs-docs` clears
  Astro `src/` and `astro.config.mjs`, and switching back clears Fumadocs
  `app/`, `content/`, `lib/`, `components/`, and Next/Fumadocs config files;
- `LocalCommandBackend` maps shell argv entries such as `/workspace` and
  `/workspace/project/dist` to the actual local workspace root before spawning
  the command, while channel/Kubernetes command execution keeps argv unchanged;
- `project.build` records `staticOutputPath` and `staticOutputName` in
  `outputs/build/latest.json` after a successful build, so later lifecycle
  tools do not have to infer whether the framework emitted `dist` or `out`;
- `preview.start` uses the latest build metadata when available, then falls
  back to template-aware static output detection. Website/Astro prefers
  `project/dist`; Fumadocs/Next static export prefers `project/out`. Existing
  accessible artifact URLs remain valid without forcing a local static server;
- `fumadocs-docs` projects reject `project/pages/**` and
  `project/src/pages/**` writes with `docs.routing_root_forbidden`, and
  `project.build` rejects existing pages-router roots before invoking Next;
- the provider-backed matrix prints `REAL_PROVIDER_STREAM_BEGIN`,
  `REAL_PROVIDER_EVENT`, `REAL_PROVIDER_EVIDENCE`, and
  `REAL_PROVIDER_STREAM_END` lines for each Website/Docs build and edit run
  when executed with `--nocapture`;
- when `RUNTIME_E2E_ARTIFACT_URL` is supplied, the E2E runner also invokes
  computed-style verification for the configured selector/property/value.
- `RUNTIME_E2E_STYLE_ONLY=1` runs computed-style verification against an
  existing artifact URL without requiring `DEEPSEEK_API_KEY`, which is useful
  for validating already-promoted artifacts independently of provider E2E.
- evidence summary binds computed-style evidence to the configured
  `RUNTIME_E2E_STYLE_PROJECT` and `RUNTIME_E2E_STYLE_STAGE` target. Defaults are
  `real-http-website` and `edit`, matching the theme-edit product gate. The
  resulting `evidence-summary.json` includes `computedStyleTarget` so the target
  can be audited from the summary file itself, and `requireComputedStyle` so it is
  clear whether computed-style evidence was an enforced gate for that run.
- `services/runtime/scripts/smoke-computed-style-artifact.sh` provides a
  repeatable local artifact style-only smoke for the verifier/evidence-summary
  chain. This remains a local regression guard between provider runs; the final
  product gate is the promoted provider artifact evidence captured in
  `.runtime-evidence/provider-20260709-180531`.

Current status:

- there is no remaining Website/Docs lifecycle blocker in the current evidence
  set. The current green provider matrix is
  `.runtime-evidence/provider-20260709-180531`;
- `.runtime-evidence/provider-20260709-175116` proved the real provider matrix
  could complete, but the evidence summary correctly failed because edit
  wrappers were missing top-level `sourceSnapshotUri`;
- `.runtime-evidence/provider-20260709-175614` exposed the Fumadocs static
  output mismatch: Next/Fumadocs exported to `project/out`, while
  `preview.start` and publish logic still expected `project/dist`, causing the
  model to attempt denied shell copy/symlink repairs;
- `.runtime-evidence/provider-20260709-180531` is the current product evidence:
  Website build/edit and Docs build/edit completed through real runtime APIs,
  Website edit contains `TESTXXX`, Docs edit contains `Edited docs title`, Docs
  build records `staticOutputName=out`, and computed-style verification for
  `:root` / `--runtime-primary` returns `#f97316`;
- the context hydration fix is implemented locally and covered by
  `agent_loop_includes_run_user_messages_in_model_context`;
- computed-style verification has now been validated both against local artifact
  fixtures and against the promoted provider Website edit artifact.

Evidence capture requirement:

- Run provider E2E with `--nocapture` and preserve the complete stream output.
- Do not append extra shell commands after the runner with `; ...` when using
  the shell exit code as pass/fail evidence. Echo the evidence directory before
  the run, or use `&&` after the runner succeeds.
- Keep the `REAL_PROVIDER_STREAM_BEGIN`, `REAL_PROVIDER_EVENT`, and
  `REAL_PROVIDER_STREAM_END` lines for each Website/Docs build and edit run.
  Stream boundary lines must include non-empty `project`, `stage`, and `run`
  fields, and each project/stage must have exactly one begin line and one end
  line.
- `evidence-summary.json` must derive `preview.candidate`, `preview.updated`,
  `run.completed`, preview ordering, and candidate/update `screenshotId`
  presence from `REAL_PROVIDER_EVENT` lines. The required event order is
  `preview.candidate -> preview.updated -> run.completed`, with exactly one of
  each event type per project/stage.
  Event lines must be object-valued JSON with non-empty `type` and `runId`
  fields.
- `evidence-summary.json` must cross-check the `runId` from
  `REAL_PROVIDER_STREAM_BEGIN` with `REAL_PROVIDER_STREAM_END`, every scoped
  `REAL_PROVIDER_EVENT`, and every `REAL_PROVIDER_EVIDENCE` wrapper for the
  same project/stage.
- `evidence-summary.json` must cross-check `preview.updated.versionId`,
  `/preview/{projectId}/current.versionId`, `runtimeState.currentVersionId`,
  and edit `editedVersionId` when present. Missing
  `preview.updated.versionId` or `/preview/{projectId}/current.versionId`
  is a summary failure, not an optional comparison.
- Version IDs, preview URLs, screenshot IDs, source snapshot URIs, and runtime
  identity fields must be non-empty strings after trimming whitespace. Blank
  strings are missing evidence, not valid placeholders. Preview URL fields must
  also be parseable URLs, and source snapshot URI fields must be parseable URIs.
- `evidence-summary.json` must also cross-check `preview.candidate.versionId`
  with `runtimeState.currentVersionId`. A candidate that has screenshot evidence
  but points at a different version is invalid product evidence.
- `evidence-summary.json` must cross-check `preview.updated.url` with
  `/preview/{projectId}/current.previewUrl`. Missing `preview.updated.url` is a
  summary failure because the product evidence cannot prove which promoted URL
  the runtime event referred to.
- `evidence-summary.json` should cross-check `preview.candidate.url` with
  `/preview/{projectId}/current.previewUrl` when candidate URL is emitted by
  the runtime event stream.
- Recoverable `tool.failed` events in provider streams must include
  non-empty `metadata.errorKind`; `tool.recovery_suggested` events must include
  non-empty `errorKind`.
- Provider evidence must fail if any stage emits
  `metadata.errorKind=build.missing_dependency`; dependency restore must be
  runtime-owned before the first successful provider build/edit promotion.
- Provider evidence must also fail if promoted runtime-state still reports
  `dependencyState.needsRestore=true`; successful build/edit evidence must prove
  dependency restore is closed.
- Provider evidence must require `runtimeState.preview.status` when preview
  state is present, but it may be `running` or `stopped`. A stopped transient
  static server is valid after promotion if durable product evidence still
  proves `currentPreview.status=promoted`, `preview.updated`, artifact URL
  serving, version/source-snapshot alignment, and computed-style success.
- Provider evidence must cross-check `runtimeState.latestBuild.sourceSnapshotUri`
  with `runtimeState.sourceSnapshotUri`, so build state and promoted edit anchor
  cannot drift apart.
- Keep `REAL_PROVIDER_EVIDENCE` lines for runtime-state, current preview,
  version/snapshot changes, `sourceSnapshotUri`, edit
  `initialSourceSnapshotUri`, edit `editedSourceSnapshotUri`, style contract,
  latest build, dependency state, preview state, artifact path, artifact
  verification URL, artifact served / byte length, and artifact text assertions.
- `REAL_PROVIDER_EVIDENCE` wrappers must include non-empty `project`, `stage`,
  `runId`, and an object-valued `evidence` payload. Each project/stage must
  emit exactly one `REAL_PROVIDER_EVIDENCE` wrapper.
- Edit evidence must prove both the boolean and the identity-level snapshot
  transition: `sourceSnapshotChanged=true`,
  `initialSourceSnapshotUri != editedSourceSnapshotUri`, and
  `editedSourceSnapshotUri == runtimeState.sourceSnapshotUri`.
- Edit evidence must also include non-empty `initialVersionId` and
  `editedVersionId`, and prove `initialVersionId != editedVersionId` plus
  `editedVersionId == runtimeState.currentVersionId`.
- Edit evidence must prove the user acceptance text reached the promoted
  artifact when the prompt asks for exact content, for example
  `artifactContainsEditMarker=true` and `expectedArtifactText=TESTXXX`. Current
  provider evidence also emits the legacy/general
  `artifactContainsExpectedText` field for compatibility; summary validation
  accepts either text-assertion field when present alone, fails if any emitted
  text-assertion field is explicitly false, and requires a non-empty
  `expectedArtifactText` so the checked text is auditable.
- Keep `evidence-summary.json`; it should report `ok: true` and cover
  `real-http-website` and `real-http-docs` for both `build` and `edit`.
- When `RUNTIME_E2E_ARTIFACT_URL` is supplied, `evidence-summary.json` must also
  include `requireComputedStyle: true`, `computedStyle.ok: true`, and
  `computedStyleTarget` for the configured project/stage.
- Provider evidence must include `localArtifactUrl` or `artifactUrl` so the
  browser computed-style verifier has a concrete artifact to inspect. Missing
  artifact verification URLs are a summary failure. Present artifact URLs must
  be parseable URLs, not arbitrary non-empty strings.
- Provider `artifactPath` must belong to the same project/stage evidence record,
  using the `/artifacts/{projectId}/...` path namespace.
- When provider stream evidence is present, the computed-style verifier URL must
  match one of the provider stage `localArtifactUrl` or `artifactUrl` values, or
  its URL pathname must match a provider `artifactPath` for the configured
  computed-style project/stage target. This allows both auto-extracted local file
  artifacts and manually supplied served artifact URLs, while preventing a
  passing style-only fixture or a different provider stage from being attached to
  the target lifecycle evidence.
- For product evidence runs, set `RUNTIME_E2E_REQUIRE_COMPUTED_STYLE=1` so a
  missing computed-style log fails the evidence summary even if the provider
  lifecycle itself completed.
- Attach the computed-style verifier output to the same evidence folder or test
  log; mismatch output should be machine-readable JSON with `ok: false`,
  `expected`, and `actual`. The computed-style log must contain exactly one JSON
  result.
- Successful computed-style output must also be auditable JSON, not just
  `ok: true`: it must include non-empty `url`, `selector`, `property`,
  `expected`, and `actual` fields so the evidence proves what browser value was
  checked. The `url` field must be a parseable URL.
- A successful run without saved stream and computed-style output is useful for
  debugging but should not be treated as product evidence.

Recommended provider run command:

```bash
RUNTIME_E2E_ENV_FILE=".env.runtime-e2e" \
RUNTIME_E2E_ARTIFACT_URL="http://127.0.0.1:18082/artifacts/<project>/current" \
RUNTIME_E2E_STYLE_SELECTOR=":root" \
RUNTIME_E2E_STYLE_PROPERTY="--runtime-primary" \
RUNTIME_E2E_STYLE_EXPECTED="#f97316" \
bash services/runtime/scripts/run-runtime-harness-provider-gates.sh
```

Recommended provider dry run before spending provider tokens:

```bash
RUNTIME_E2E_RUN_LOCAL_GATES=0 \
RUNTIME_E2E_DRY_RUN=1 \
RUNTIME_E2E_ENV_FILE=".env.runtime-e2e" \
RUNTIME_E2E_ARTIFACT_URL="http://127.0.0.1:18082/artifacts/<project>/current" \
RUNTIME_E2E_STYLE_SELECTOR=":root" \
RUNTIME_E2E_STYLE_PROPERTY="--runtime-primary" \
RUNTIME_E2E_STYLE_EXPECTED="#f97316" \
bash services/runtime/scripts/run-runtime-harness-provider-gates.sh
```

If the promoted artifact URL is not known before the provider run, omit
`RUNTIME_E2E_ARTIFACT_URL`; the runner will try to extract
`localArtifactUrl` from the Website edit `REAL_PROVIDER_EVIDENCE` line and use
that for computed-style verification. For a dry run, supplying
`RUNTIME_E2E_ARTIFACT_URL` is preferred when a candidate URL is already known,
because stdout and `run-metadata.env` will then show exactly which artifact the
browser verifier would inspect.

The lower-level `run-real-provider-http-lifecycle-e2e.sh` remains available for
debugging individual evidence pieces, but product evidence should prefer the
provider gate wrapper so local gates and evidence paths are consistent.

Recommended style-only command for an already promoted artifact:

```bash
RUNTIME_E2E_STYLE_ONLY=1 \
RUNTIME_E2E_LOG_DIR=".runtime-evidence/style-only-$(date +%Y%m%d-%H%M%S)" \
RUNTIME_E2E_ARTIFACT_URL="http://127.0.0.1:18082/artifacts/<project>/current" \
RUNTIME_E2E_STYLE_SELECTOR=":root" \
RUNTIME_E2E_STYLE_PROPERTY="--runtime-primary" \
RUNTIME_E2E_STYLE_EXPECTED="#f97316" \
bash services/runtime/scripts/run-real-provider-http-lifecycle-e2e.sh
```

Recommended local artifact style smoke when no promoted artifact server is
reachable:

```bash
bash services/runtime/scripts/smoke-computed-style-artifact.sh
```

Recommended local gate bundle before provider reruns:

```bash
bash services/runtime/scripts/run-runtime-harness-local-gates.sh
```

Required API path:

```text
POST /runs
POST /runs/{runId}/continue
GET /runs/{runId}/events
GET /projects/{projectId}/runtime-state
GET /preview/{projectId}/current
```

Required scenarios:

- Generate Website from markdown design input.
- Edit Website hero title.
- Edit Website primary theme color.
- Generate Docs from markdown input.
- Edit Docs title and one section.

Assertions:

- `preview.candidate` appears before `preview.updated`, and `preview.updated`
  appears before `run.completed`;
- current version changes after edit;
- source snapshot URI changes after edit and the edited URI matches
  `runtimeState.sourceSnapshotUri`;
- `preview.candidate.versionId`, `preview.updated.versionId`,
  `/preview/{projectId}/current.versionId`, and
  `runtimeState.currentVersionId` converge on the promoted version for each
  successful stage;
- artifact URL serves promoted output;
- computed browser styles reflect theme changes;
- no first build failure is caused by missing dependencies;
- no candidate is created without screenshot evidence;
- recoverable errors, if any, contain stable `errorKind` metadata.

Computed-style validation must use a real browser execution path. A screenshot is
necessary but not sufficient for token/theme correctness.

### 7.4 Harness Release Gates

The release gate is intentionally stricter than the local regression suite.

Gate 1: Local harness contracts

- all unit and integration tests listed in this document pass;
- shared API schema tests and typecheck pass;
- `git diff --check` passes;
- no new recoverable error class is added without stable `errorKind` metadata
  and an assertion.

Gate 2: Public runtime API lifecycle

- Website generation, Website hero edit, Website theme edit, Docs generation,
  and Docs content edit run through `POST /runs`, `POST /runs/{runId}/continue`,
  `GET /runs/{runId}/events`, `GET /projects/{projectId}/runtime-state`, and
  promoted preview URLs;
- Website hero edit and Website theme edit must be separate mutable runs against
  the same project, using the previous `runtime-state` response as the next
  edit anchor. This catches title-only successes that do not prove style editing;
- `preview.updated` is observed before `run.completed` for successful mutable
  runs;
- `currentVersionId` and `sourceSnapshotUri` change after edits;
- candidate/update/current/runtime-state version IDs are cross-checked in saved
  evidence, not only observed informally in the stream;
- the same project remains editable using the public runtime-state response.

Gate 3: Visual/style evidence

- generated source contains Tailwind imports, runtime token files, local UI
  primitives, and `state/style-contract.json`;
- theme edits use `style.update_tokens` or a declared style layer rather than
  scattered literal replacement;
- the rebuilt artifact contains the updated token value, proving the style edit
  survives build output rather than only mutating an intermediate source file;
- browser-computed style verification passes against the promoted artifact, not
  only against a temporary local fixture.

Gate 4: Failure behavior

- repeated recoverable failures produce `tool.recovery_suggested` or equivalent
  structured event metadata;
- identical retries stop as `partial` rather than looping indefinitely;
- product-facing errors prefer `errorKind`, `recoverable`, `suggestedAction`,
  and artifact/log references over raw stack traces.

## 8. Regression Matrix

### Unit Tests

Required:

- `fs.path_normalize_workspace_prefix`
- `fs.path_external_directory_has_suggestion`
- `fs.patch_tools_reject_nested_package_root_with_structured_error`
- `fs.patch_replace_all_updates_all_matches`
- `fs.patch_rejects_without_read`
- `fs.patch_rejects_stale_read`
- `fs.multi_patch_applies_multiple_edits_atomically`
- `fs.multi_patch_does_not_write_when_later_edit_fails`
- `project.build_uses_pnpm_when_project_uses_pnpm`
- `project.build_auto_restores_missing_node_modules`
- `project.ensure_dependencies_timeout_has_structured_metadata`
- `project.build_missing_command_failure_has_structured_metadata`
- `fs.list_missing_directory_failure_has_structured_metadata`
- `project_init_cleans_conflicting_template_files_between_templates`
- `shell_run_local_backend_maps_virtual_workspace_argv_paths`
- `fumadocs_docs_rejects_pages_router_writes_with_structured_metadata`
- `project_build_rejects_fumadocs_docs_with_pages_router_root`
- `preview.report_candidate_requires_screenshot`
- `preview.report_candidate_rejects_blank_screenshot`
- `style.update_tokens_updates_declared_tokens`
- `style.update_tokens_rejects_unknown_tokens`
- `style_update_tokens_requires_runtime_style_contract_metadata`
- `style_update_tokens_rejects_missing_css_variable_with_metadata`
- `post_tool_failure_hook_guides_repeated_style_token_errors`
- `pre_tool_hook_normalizes_workspace_virtual_path_fields`
- `pre_tool_hook_redirects_shell_preview_servers_to_preview_tools`
- `pre_tool_hook_redirects_interactive_scaffolds_to_project_tools`
- `agent_loop_includes_run_user_messages_in_model_context`

### Integration Tests

Required:

- Website create -> build -> promote -> edit hero -> rebuild -> promote.
- Website create -> build -> promote -> edit theme token -> rebuild -> promote.
- Website public-runtime lifecycle keeps the hero edit and theme edit as two
  independent `edit` runs: the second run starts from the first edit's
  `currentVersionId`, calls `style.update_tokens`, rebuilds, promotes, and
  verifies both `src/styles/tokens.css` and `dist/index.html` contain the new
  token value.
- Docs create -> build -> promote -> edit title -> rebuild -> promote.
- PhaseA public runs execute in a project-scoped workspace root instead of the
  global runtime workspace root.
- `POST /runs/{runId}/continue` user message is included in the next model
  request before an edit run can complete.
- Edit start restores corrupted workspace from `baseVersionId`.
- Stale `baseVersionId` is rejected.
- Concurrent mutable run is rejected.
- Candidate source snapshot mismatch is rejected.
- `PreToolUseHook` rejects workspace tools during Brief and Brief write tools
  during Build/Edit before tool schema execution.
- `PreToolUseHook` normalizes virtual `/workspace/...` `cwd` values before
  sandbox permission checks and audit summary creation.
- `PreToolUseHook` redirects package install/add, preview server, and
  interactive scaffold shell commands to dedicated lifecycle tools.
- `PostToolUseFailureHook` emits recovery guidance and partial-stops repeated
  identical recoverable failures.
- `PostToolUseSuccessHook` adds lifecycle metadata to `ToolCompleted` events for
  build, dependency, style, screenshot, and promotion tools.

### Recommended Local Test Commands

Run these after hook or typed metadata changes:

```bash
cargo fmt --manifest-path services/runtime/Cargo.toml -- --check
cargo test --manifest-path services/runtime/Cargo.toml agent_hooks
cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools
cargo test --manifest-path services/runtime/Cargo.toml --test preview_promotion
cargo test --manifest-path services/runtime/Cargo.toml --test tool_permissions_integration
cargo test --manifest-path services/runtime/Cargo.toml --test agent_loop
cargo test --manifest-path services/runtime/Cargo.toml --test http_api
cargo test --manifest-path services/runtime/Cargo.toml --test astro_build_agent
npm test --prefix packages/shared
npm run typecheck --prefix packages/shared
node services/runtime/scripts/test-real-provider-evidence-summary.mjs
git diff --check
```

Run these after unit/integration tests pass:

```text
real runtime API E2E with provider-backed Website generation
real runtime API E2E with provider-backed Website edit
real runtime API E2E with provider-backed Docs generation
real runtime API E2E with provider-backed Docs edit
browser computed-style verification for theme changes
```

Latest local evidence captured on 2026-07-09:

- `cargo test --manifest-path services/runtime/Cargo.toml --test http_api cancel_run_cleans_staged_chunk_sessions_for_run -- --nocapture`
- `cargo test --manifest-path services/runtime/Cargo.toml --test http_api start_edit_waits_for_continue_before_spawning_agent -- --nocapture`
- `cargo test --manifest-path services/runtime/Cargo.toml --test tool_permissions_integration each_sandbox_tool_decision_writes_one_audit_record -- --nocapture`
- `cargo fmt --manifest-path services/runtime/Cargo.toml -- --check`
- `cargo test --manifest-path services/runtime/Cargo.toml agent_hooks`
- `cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools`
- `cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools project_ensure_dependencies_timeout_has_structured_metadata -- --nocapture`
- `cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools project_build_missing_command_failure_has_structured_metadata -- --nocapture`
- `cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools project_init_cleans_conflicting_template_files_between_templates -- --nocapture`
- `cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools project_init_fumadocs_docs_writes_docs_source_contract -- --nocapture`
- template initialization tests assert the runtime style contract includes
  `globalCssFile`, `componentRoot`, Tailwind v4 metadata, token mappings, and
  local UI primitive files for both Website and Docs templates
- `cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools shell_run_local_backend_maps_virtual_workspace_argv_paths -- --nocapture`
- `cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools shell_run -- --nocapture`
- `cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools fumadocs_docs_rejects_pages_router_writes_with_structured_metadata -- --nocapture`
- `cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools project_build_rejects_fumadocs_docs_with_pages_router_root -- --nocapture`
- `cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools -- --nocapture`
- `cargo test --manifest-path services/runtime/Cargo.toml --test http_api phase_a_public_run_uses_project_scoped_workspace_root -- --nocapture`
- runtime-state HTTP tests assert that project-scoped PhaseA, global PhaseA
  fallback, and Kubernetes workspace-channel reads preserve full style contract
  metadata including `globalCssFile`, `componentRoot`, and Tailwind fields
- `cargo test --manifest-path services/runtime/Cargo.toml --test preview_promotion`
- `cargo test --manifest-path services/runtime/Cargo.toml --test tool_permissions_integration`
- `cargo test --manifest-path services/runtime/Cargo.toml --test agent_loop`
- `cargo test --manifest-path services/runtime/Cargo.toml --test agent_loop agent_loop_includes_run_user_messages_in_model_context -- --nocapture`
- `cargo test --manifest-path services/runtime/Cargo.toml --test http_api public_runtime_lifecycle_build_runtime_state_edit_and_rebuilds -- --nocapture`
- that Website public-runtime lifecycle regression now covers two separate edit
  runs on the same project: a hero-title patch followed by a theme-token edit
  through `style.update_tokens`, with the promoted rebuild containing
  `--runtime-primary: #f97316;`
- `cargo test --manifest-path services/runtime/Cargo.toml --test http_api public_runtime_docs_lifecycle_build_runtime_state_edit_and_rebuilds -- --nocapture`
- `cargo test --manifest-path services/runtime/Cargo.toml --test http_api`
- `cargo test --manifest-path services/runtime/Cargo.toml --test astro_build_agent`
- `cargo test --manifest-path services/runtime/Cargo.toml --test astro_build_agent confirmed_docs_brief_generates_fumadocs_project_candidate_and_promoted_preview -- --nocapture`
- `bash -n services/runtime/scripts/run-runtime-harness-local-gates.sh`
- `bash services/runtime/scripts/run-runtime-harness-local-gates.sh`
- `bash -n services/runtime/scripts/run-runtime-harness-provider-gates.sh`
- `RUNTIME_E2E_RUN_LOCAL_GATES=0 bash services/runtime/scripts/run-runtime-harness-provider-gates.sh` should fail clearly when `DEEPSEEK_API_KEY` is absent
- `bash services/runtime/scripts/run-runtime-harness-provider-gates.sh` should
  fail clearly before local gates when `DEEPSEEK_API_KEY` is absent
- `RUNTIME_E2E_RUN_LOCAL_GATES=0 RUNTIME_E2E_DRY_RUN=1 DEEPSEEK_API_KEY_FILE=/path/to/key bash services/runtime/scripts/run-runtime-harness-provider-gates.sh`
- `RUNTIME_E2E_RUN_LOCAL_GATES=0 RUNTIME_E2E_DRY_RUN=1 RUNTIME_E2E_ENV_FILE=/path/to/env bash services/runtime/scripts/run-runtime-harness-provider-gates.sh`
- `RUNTIME_E2E_DRY_RUN=1 DEEPSEEK_API_KEY_FILE=/path/to/key bash services/runtime/scripts/run-runtime-harness-provider-gates.sh` should report `PROVIDER_GATE_DRY_RUN=1` before local gates
- provider dry-run should write `run-metadata.env` with `providerGateDryRun=1`,
  `deepseekApiKeyPresent=true`, `artifactUrl` when supplied, and style target
  fields, without writing the actual key value; the dry-run terminal output
  should also echo `RUNTIME_E2E_ARTIFACT_URL`, `RUNTIME_E2E_STYLE_SELECTOR`,
  `RUNTIME_E2E_STYLE_PROPERTY`, and `RUNTIME_E2E_STYLE_EXPECTED` so the exact
  browser-computed style gate and target artifact are visible before spending
  provider tokens
- `RUNTIME_E2E_STYLE_ONLY=1 ... bash services/runtime/scripts/run-real-provider-http-lifecycle-e2e.sh` should write `styleProject` and `styleStage` to `run-metadata.env`
- `bash services/runtime/scripts/run-runtime-harness-local-gates.sh` now covers
  `agent_hooks`, `permission_engine`, the provider wrapper no-key failure path,
  and key-file/env-file dry-run paths with dummy secrets, without calling the
  provider. The key-file dry run also supplies a dummy artifact URL and asserts
  that the URL is echoed to stdout and persisted as `artifactUrl=` in
  `run-metadata.env`, proving the future computed-style browser target is not
  implicit or lost.
- `node --check services/runtime/scripts/extract-provider-artifact-url.mjs`
- artifact URL extractor tests must cover Website and Docs target selection,
  missing explicit flag values, and non-parseable artifact URLs
- `cargo test --manifest-path services/runtime/Cargo.toml --test permission_engine`
- `npm test --prefix packages/shared`
- `npm run typecheck --prefix packages/shared`
- `bash -n services/runtime/scripts/run-real-provider-http-lifecycle-e2e.sh`
- `bash -n services/runtime/scripts/smoke-fumadocs-docs-build.sh`
- `bash services/runtime/scripts/smoke-fumadocs-docs-build.sh`
- `bash -n services/runtime/scripts/smoke-computed-style-artifact.sh`
- `bash services/runtime/scripts/smoke-computed-style-artifact.sh`
- the bundled local gate also runs a second computed-style smoke with
  `RUNTIME_E2E_STYLE_SELECTOR=:root` and
  `RUNTIME_E2E_STYLE_PROPERTY=--runtime-primary`, proving the provider Gate 3
  argument shape stays valid
- `node --check services/runtime/scripts/verify-computed-style.mjs`
- computed-style verifier CLI tests must fail fast for missing explicit values,
  non-parseable URLs, and non-integer timeout values, while accepting CSS custom
  property values such as `--runtime-primary` for the `--property` argument
- `node --check services/runtime/scripts/summarize-real-provider-evidence.mjs`
- `node --check services/runtime/scripts/test-real-provider-evidence-summary.mjs`
- `node services/runtime/scripts/test-real-provider-evidence-summary.mjs`
- evidence-summary tests must cover both default full-matrix enforcement and
  explicit `--project` / `--stage` filtering, including the case where
  `--project real-http-website --stage edit` passes for a single-stage log while
  the same log fails without filters because Docs/build coverage is missing
- evidence-summary tests must fail fast when log/output, filter, or
  computed-style target flags are missing explicit non-empty values
- evidence-summary tests must fail when required promotion identity fields are
  absent, including `preview.updated.versionId`, `preview.updated.url`, and
  `/preview/{projectId}/current.versionId`
- evidence-summary tests must fail when required identity fields are present
  only as blank strings, such as a blank screenshot ID or blank current preview
  URL
- evidence-summary tests must fail when recoverable tool failures or recovery
  suggestions include blank `errorKind` values
- evidence-summary tests must fail when style contract metadata is incomplete,
  including missing Tailwind metadata or token mappings with blank token names or
  blank CSS variable names
- evidence-summary tests must fail when edit evidence omits
  `initialVersionId` or `editedVersionId`
- evidence-summary tests must pass when edit evidence uses the documented
  `artifactContainsEditMarker=true` field and must fail when any emitted edit
  artifact text assertion is false
- evidence-summary tests must fail when edit evidence omits
  `expectedArtifactText` or includes it only as blank text
- evidence-summary tests must fail when artifact or computed-style URL fields
  are present but not parseable URLs
- evidence-summary tests must fail when `artifactPath` points at a different
  project artifact namespace
- evidence-summary tests must fail when preview URL fields are present but not
  parseable URLs
- evidence-summary tests must fail when source snapshot URI fields are present
  but not parseable URIs
- evidence-summary tests must fail when top-level provider evidence omits
  `sourceSnapshotUri`, even if `runtimeState.sourceSnapshotUri` is present
- evidence-summary tests must fail whenever any `tool.failed` event carries
  `metadata.errorKind=build.missing_dependency`, regardless of whether that
  failure event is marked recoverable
- `git diff --check`
- `RUNTIME_E2E_STYLE_ONLY=1 RUNTIME_E2E_ARTIFACT_URL=file://... RUNTIME_E2E_STYLE_SELECTOR=#probe RUNTIME_E2E_STYLE_PROPERTY=color RUNTIME_E2E_STYLE_EXPECTED=#f97316 bash services/runtime/scripts/run-real-provider-http-lifecycle-e2e.sh`

The style-only run produced an `evidence-summary.json` with auditable
computed-style evidence. The latest repeatable local artifact smoke, run through
the bundled local gate script, wrote
`.runtime-evidence/style-only-local-20260709-180511/evidence-summary.json`:

```json
{
  "ok": true,
  "requireComputedStyle": true,
  "computedStyleTarget": {
    "project": "real-http-website",
    "stage": "edit"
  },
  "computedStyle": {
    "result": {
      "url": "file:///.../runtime-style-fixture.../index.html",
      "selector": "#probe",
      "property": "color",
      "expected": "#f97316",
      "actual": "rgb(249, 115, 22)",
      "mode": "local-file"
    }
  }
}
```

The bundled local gate now also runs the same verifier with the final provider
gate argument shape. That direct CSS custom property smoke wrote
`.runtime-evidence/style-only-custom-property-20260709-180512/evidence-summary.json`:

```json
{
  "ok": true,
  "computedStyle": {
    "result": {
      "selector": ":root",
      "property": "--runtime-primary",
      "expected": "#f97316",
      "actual": "#f97316"
    }
  }
}
```

All local commands above passed where run in this phase, including the full
`sandbox_tools` suite, the project-scoped PhaseA workspace HTTP regression, the
`agent_hooks` filtered suite, `permission_engine`, the new real Fumadocs smoke,
the local computed-style artifact smoke, and the bundled
`run-runtime-harness-local-gates.sh` gate. The provider wrapper syntax and its
no-key failure path were also verified, including fail-fast behavior before local
gates and dry-run validation through
`DEEPSEEK_API_KEY_FILE` and `RUNTIME_E2E_ENV_FILE`. The key-file dry-run path now
also passes a dummy `RUNTIME_E2E_ARTIFACT_URL` and checks both stdout and
`run-metadata.env`, so the harness proves the artifact target and
computed-style target are visible before a real provider run consumes tokens.
The provider evidence path now
emits a `localArtifactUrl`, and the runner can automatically use that URL for the
computed-style gate after a successful provider matrix. The evidence summary now
fails if neither `localArtifactUrl` nor `artifactUrl` is present for a stage. The
evidence summary also rejects computed-style output whose `url` does not match
one of the provider artifact evidence URLs or served `artifactPath` URL pathnames
for the configured computed-style project/stage target, so visual evidence
cannot come from an unrelated local fixture or the wrong lifecycle stage. The
computed-style local artifact smoke now uses a root-relative `/_astro/app.css`
stylesheet, so the local gate proves the verifier handles the same asset shape
emitted by Astro/Fumadocs production artifacts. The smoke also includes an
artifact-root escape stylesheet that would override the probe color if the
verifier followed `/_astro/../../...`; the passing result proves the local
artifact CSS path stays bounded to the artifact root. A focused style-only rerun
also verifies `:root` / `--runtime-primary` directly, covering the CSS custom
property argument shape used by the provider Gate 3 command. A manual check
against the stale browser URL
`http://127.0.0.1:18082/artifacts/monad-design-website-api-1783524832/current`
failed with `ERR_CONNECTION_REFUSED`, confirming that an open browser tab URL is
not reusable product evidence unless the artifact server is actually reachable
at verification time. The smoke first reproduced
the provider failure class from `.runtime-evidence/provider-20260709-151203`:
`typescript: latest` resolved to `7.0.2` and Next failed with
`id argument undefined`. After pinning TypeScript to `5.9.3`, the same generated
Fumadocs scaffold installed dependencies and completed runtime `project.build`,
which runs `next build --webpack`, writes `outputs/build/latest.json`, and
captures a source snapshot.

The latest local gate also forced three fixture corrections after the PhaseA
workspace isolation work:

- `cancel_run_cleans_staged_chunk_sessions_for_run` now uses
  `SandboxBackendMode::PhaseAContract` so cleanup targets
  `workspace_root/project-1/outputs/staged-writes`, matching runtime behavior.
- `start_edit_waits_for_continue_before_spawning_agent` now places its base
  source snapshot under `workspace_root/project-1/outputs/build/source-snapshots`
  before exercising edit restore from `baseVersionId`.
- `each_sandbox_tool_decision_writes_one_audit_record` now creates and uses
  `project/custom-app`, matching `state/project-state.json.appRoot` and the
  `fs.delete` invariant that only non-root paths below the app root may be
  deleted.

The current captured provider full matrix is green:

- lifecycle stream:
  `.runtime-evidence/provider-20260709-180531/provider-lifecycle.log`;
- browser computed-style evidence:
  `.runtime-evidence/provider-20260709-180531/computed-style.log`;
- machine summary:
  `.runtime-evidence/provider-20260709-180531/evidence-summary.json`.

The summary result is:

```json
{
  "ok": true,
  "errors": [],
  "computedStyle": true,
  "target": {
    "project": "real-http-website",
    "stage": "edit"
  },
  "stages": [
    "real-http-website:build",
    "real-http-website:edit",
    "real-http-docs:build",
    "real-http-docs:edit"
  ]
}
```

Older provider folders remain useful as failure baselines:

- `.runtime-evidence/provider-20260709-151203` captured the pre-TypeScript-pin
  Fumadocs/Next failure where `typescript: latest` resolved to an incompatible
  prerelease and failed before Docs preview promotion;
- `.runtime-evidence/provider-20260709-175116` captured a completed matrix whose
  summary failed because edit evidence wrappers omitted top-level
  `sourceSnapshotUri`;
- `.runtime-evidence/provider-20260709-175614` captured the Fumadocs static
  output mismatch where build succeeded to `project/out` but preview/publish
  still expected `project/dist`.

## 9. Product/UI Contract

The product or BFF should not scrape internal event text to discover edit
metadata. It should use public runtime state and stable event metadata.

Required state endpoint:

```text
GET /projects/{projectId}/runtime-state
```

Required fields:

```json
{
  "projectId": "project",
  "currentVersionId": "version",
  "sandboxBindingId": "sandbox-binding",
  "sourceSnapshotUri": "file:///workspace/outputs/build/source-snapshots/build-id",
  "appRoot": "project",
  "templateKey": "astro-website",
  "styleContractPath": "/workspace/state/style-contract.json",
  "styleContract": {
    "tokenFile": "project/src/styles/tokens.css",
    "globalCssFile": "project/src/styles/global.css",
    "componentRoot": "project/src/components/ui",
    "tailwind": {
      "version": "4",
      "entryImport": "@import \"tailwindcss\"",
      "themeSource": "css-variables"
    },
    "tokens": {
      "color.primary": "--runtime-primary"
    }
  },
  "latestBuild": {
    "status": "success",
    "sourceSnapshotUri": "file:///workspace/outputs/build/source-snapshots/build-id",
    "staticOutputName": "out",
    "staticOutputPath": "/workspace/project/out"
  },
  "dependencyState": {
    "needsRestore": false
  },
  "preview": {
    "status": "running"
  }
}
```

The lifecycle detail fields are nullable/optional for older projects or
projects that have not yet initialized those files. The edit anchor fields
remain required.

When lifecycle detail fields are present, shared API schemas validate their
minimum structure instead of treating them as arbitrary JSON:

- `styleContract.tokenFile` must be a non-empty string;
- `styleContract.globalCssFile` and `styleContract.componentRoot` must be
  non-empty strings so product integrations can locate the style entrypoint and
  local UI primitive layer from public runtime state;
- `styleContract.tailwind.version`, `entryImport`, and `themeSource` must be
  non-empty strings, and provider evidence summary enforces the same metadata;
- `styleContract.tokens` must be a non-empty token-to-CSS-variable map whose
  keys and values are non-empty strings;
- `latestBuild.status` must be present and
  `latestBuild.sourceSnapshotUri` must match `sourceSnapshotUri`;
- `latestBuild.staticOutputName` and `latestBuild.staticOutputPath` should be
  present after successful static builds so preview/publish can serve the actual
  framework output directory without guessing;
- `dependencyState.needsRestore` must be boolean;
- `preview.status` must be present when preview state is present. Provider
  evidence accepts `preview.status=running` or `preview.status=stopped`; durable
  promotion is proven by `currentPreview.status=promoted`, event ordering,
  artifact serving, source/version alignment, and computed-style evidence.

Runtime state reads the active sandbox workspace channel in Kubernetes mode.
For local contract runs, it reads project-scoped workspace files first and
falls back to the PhaseA/global workspace layout, matching artifact serving
behavior.

The UI edit request should always include:

- `baseVersionId`;
- `sandboxBindingId`;
- user edit prompt.

Shared API schemas enforce the first two edit anchors for `phase=edit` start-run
requests. Product code should treat schema failure here as a client-side
contract bug rather than waiting for the runtime to reject the request.

Shared API schemas also enforce `briefId` for top-level `phase=build` start-run
requests. The runtime still verifies that the brief exists and is confirmed;
the shared schema catches missing build anchors before the request leaves the
product/BFF layer.

For error display and retry UX, the UI should prefer structured fields such as
`errorKind`, `recoverable`, `suggestedAction`, `diagnostics`, and artifact/log
paths over parsing human text.

Shared event schemas enforce the same principle for runtime streams:
`tool.failed` events with `recoverable=true` must include
`metadata.errorKind`, and `tool.recovery_suggested` events expose top-level
`errorKind`.

## 10. Non-Goals

This plan does not include:

- DesignProfile data model;
- design embedding or retrieval;
- Figma integration;
- production multi-tenant policy;
- branch/fork editing;
- visual review model quality improvements beyond screenshot presence, blank
  page checks, and browser-computed style assertions.

## 11. Recommended Execution Order

Use this order from the current code state:

1. Keep the typed metadata regression matrix green when adding new tools or
   recoverable failure classes.
2. Keep the real local smoke check for the `fumadocs-docs` scaffold green:
   initialize the template, install dependencies, write representative generated
   MDX/content, and run runtime `project.build`. This now guards the
   TypeScript/Next/Fumadocs compatibility issue that produced
   `id argument undefined` in `.runtime-evidence/provider-20260709-151203`.
3. Run the local regression command matrix after hook, metadata, lifecycle,
   template, or promotion changes.
4. Re-execute the provider-backed real runtime API E2E matrix with
   `DEEPSEEK_API_KEY` available in the shell environment and preserve the full
   stream under a new `.runtime-evidence/provider-*` directory.
5. Run browser computed-style verification against a live promoted theme edit
   artifact, not only a temporary local HTML page.
6. Capture the provider E2E stream as product evidence for Website generation,
   Website edit, Docs generation, and Docs edit.
7. Only after this lifecycle is stable, introduce DesignProfile as an
   intelligence layer above the runtime contract.

This keeps the harness focused on deterministic lifecycle reliability first.
DesignProfile should improve style intelligence later, but it should not be
used to compensate for missing runtime correctness.
