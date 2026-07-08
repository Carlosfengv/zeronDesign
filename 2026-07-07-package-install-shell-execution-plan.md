---
date: 2026-07-07
status: draft
type: implementation-plan
topic: package-install-shell-execution-policy
related:
  - ./README.md
  - ./2026-07-04-agent-harness-design.md
  - ./2026-07-04-rust-runtime-spec.md
  - ./2026-07-07-runtime-harness-fix-plan.md
  - ./2026-07-07-tool-call-json-write-protocol-fix-plan.md
  - /Users/carlos/Downloads/claude-code-main/src/constants/prompts.ts
  - /Users/carlos/Downloads/claude-code-main/src/skills/bundled/updateConfig.ts
---

# Package Install 与真实 Shell 执行落地方案

## 1. 结论

`package.install` 不应该被设计成一个和真实包管理器脱节的专属工具，也不应该退化成让模型直接调用 `shell.run(["pnpm", "install"])` 或 `shell.run(["npm", "install"])`。

正确边界是：

```text
模型侧：调用 package.install
平台侧：执行 registry、cwd、policy、audit、timeout、log、stream 管控
底层：真实 spawn npm/pnpm/yarn/bun install/add 命令
```

也就是说，`package.install` 是一个 policy wrapper。它对模型表现为稳定、可审计的依赖安装操作；对 runtime 实现表现为真实 shell/process 执行。

这和产品文档中的 hybrid runtime flow 一致：

- `project.init` / `project.detect_root` / `project.build` / `preview.start` 负责项目生命周期。
- `fs.*` 负责页面内容、代码和样式生成。
- `package.install` 负责依赖变更。
- `shell.run` 保留为诊断和修复工具，但不能成为依赖安装、项目初始化和正式 build promotion 的主路径。

## 2. 背景与问题

### 2.1 用户期望

用户希望 `package.install` 这类操作可以使用真实 shell 能力，在项目目录中执行 `pnpm install`、`npm install` 等命令，而不是依赖一个和真实开发行为割裂的专属安装器。

这个诉求是合理的。生成网站时，依赖安装必须尽量接近真实工程环境，否则会出现：

- 模型生成的 `package.json` 和实际安装行为不一致。
- `pnpm` 项目却被 runtime 固定用 `npm` 安装。
- 本地 E2E 与生产-like sandbox 的依赖解析结果不一致。
- stream 中看不到真实安装过程，难以判断是网络、registry、lockfile 还是 package script 的问题。

### 2.2 产品文档约束

产品文档同时明确要求依赖安装不能直接走裸 `shell.run`：

- `shell.run(["pnpm", "install"])` / `shell.run(["npm", "install"])` 应被 deny，并提示使用 `package.install`。
- 原因是 registry policy、lockfile policy 和 audit metadata 不能被 shell 绕过。
- public npm registry 只允许在显式 `local-e2e` / dev profile 下使用，production-like profile 必须走内部 registry/proxy。
- runtime policy 默认是 `production`，不能由模型输入覆盖。

所以当前目标不是"让 shell.run 放开 npm install"，而是"让 package.install 具备真实 npm/pnpm install 的执行能力和可观测性"。

### 2.3 Claude Code 的借鉴边界

Claude Code 更偏本地 coding agent。它有 first-class BashTool，用户可以配置类似 `Bash(npm:*)` 的权限规则，让 npm 命令无需反复确认。

但 Claude Code 的 prompt 也强调：当存在相关 dedicated tool 时，应优先使用 dedicated tool，而不是用 Bash 绕过。

本项目是面向 artifact runtime 的产品化 harness。相比本地 coding agent，它多了以下约束：

- 多租户或多项目 sandbox 隔离。
- 内部 registry/proxy 和 production-like network deny。
- preview promotion gate。
- build/edit run 的结构化事件流。
- run/project/tool 级 audit。
- 可复现模板初始化。

因此，Claude Code 的可借鉴点不是"把 npm install 全部放回 Bash"，而是：

- shell 必须是真实进程执行。
- 权限规则和工具 schema 要清晰。
- 输出要 streaming。
- 长时间无输出或交互式 prompt 要 watchdog。
- 用户或测试 profile 可以显式扩大权限，但默认安全边界不能丢。

## 3. 目标

### 3.1 功能目标

- `package.install` 底层真实执行项目选择的 package manager。
- 支持 `npm` 和 `pnpm`，P1 可扩展 `yarn`、`bun`。
- 默认 cwd 使用 `state/project.json.appRoot`，不让模型猜 `project`、`.`、`project/project`。
- 默认 registry 来自 runtime 配置的内部 registry/proxy。
- `local-e2e` profile 可以显式使用 public npm registry，并写 audit。
- `package.install` P0 必须同时支持按 `package.json` 恢复安装和新增依赖安装。
- production-like profile 下 public npm registry 必须 deny。
- install stdout/stderr 进入 run stream，并写入 log 文件。
- `shell.run npm install` / `pnpm install` 继续 deny，但返回可执行的 `package.install` guidance。
- build/edit prompt 明确说明：依赖安装使用 `package.install`，不是 shell install。

### 3.2 非目标

- 不在本阶段复刻 Claude Code 的完整 Bash permission UI。
- 不把 `shell.run` 变成任意网络访问的通用逃生口。
- 不允许模型通过 `npm create`、`npx create-*`、`astro add` 等交互式 scaffold 命令初始化项目。
- 不在 P0 支持所有包管理器生态细节，例如 workspace protocol、monorepo hoist 策略、复杂 lifecycle script allowlist。
- 不用 `shell.run npm run build` 替代 `project.build` 的 promotion evidence。

## 4. 设计原则

1. **真实执行，不裸露策略。**
   `package.install` 必须调用真实 package manager，但 registry、cwd、audit、network profile、timeout 和 log 由 harness 控制。

2. **模型不负责记住工作目录。**
   app root 是 runtime 状态，不是模型记忆。依赖安装、build、preview 默认读取 `state/project.json.appRoot`。

3. **工具 schema、permission、execute 必须一致。**
   不能 permission 阶段允许缺省 registry，execute 阶段又要求 `registry` 必填。

4. **生产默认保守，本地测试显式放开。**
   `production` 是默认 profile；public npm 只能由 runtime/test harness 显式配置为 `local-e2e` 后允许。

5. **stream 是产品能力。**
   真实 install 的 stdout/stderr、开始、完成、失败和 log path 都应该在 SSE/event stream 中可见。

6. **专用工具只管生命周期。**
   专用工具不生成最终内容和样式。页面、组件、CSS、文档内容仍由模型通过 `fs.*` 完成。

## 5. 工具契约

### 5.1 `package.install` 输入

P0 schema：

```json
{
  "packages": ["astro", "tailwindcss"],
  "packageManager": "pnpm",
  "cwd": "project",
  "registry": "https://registry.npmjs.org/",
  "mode": "add",
  "timeoutMs": 120000
}
```

字段说明：

| 字段 | 必填 | P0 | 说明 |
| --- | --- | --- | --- |
| `mode` | 否 | 是 | `restore` 或 `add`。缺省推断：`packages` 为空时为 `restore`，非空时为 `add`。 |
| `packages` | 否 | 是 | 依赖 spec 列表。`mode=add` 时必须非空；`mode=restore` 时必须为空或省略，表示按 package.json/lockfile 安装。 |
| `packageManager` | 否 | 是 | `npm` 或 `pnpm`。缺省从 project state、lockfile、template policy 推断。 |
| `cwd` | 否 | 是 | workspace 内路径。缺省 `state/project.json.appRoot`。 |
| `registry` | 否 | 是 | 缺省 `RUNTIME_NPM_REGISTRY` 或内部 registry/proxy。 |
| `timeoutMs` | 否 | 是 | 缺省 120000，允许 runtime 配置上限。 |

P0 必须支持两类调用：

```json
{ "mode": "restore", "packageManager": "pnpm", "cwd": "project" }
```

```json
{ "mode": "add", "packages": ["tailwindcss"], "packageManager": "pnpm", "cwd": "project" }
```

不合法组合返回 recoverable validation error：

- `mode=add` 但 `packages` 为空。
- `mode=restore` 但传入非空 `packages`。
- `mode` 不是 `restore` 或 `add`。

### 5.2 package manager 推断顺序

```text
input.packageManager
  -> state/project.json.packageManager
  -> lockfile detect: pnpm-lock.yaml / package-lock.json / yarn.lock / bun.lockb
  -> template policy default
  -> npm
```

P0 至少支持：

- `npm`
- `pnpm`

如果推断到不支持的 manager，返回 recoverable error：

```text
package.install unsupported packageManager "yarn"; use npm or pnpm for this runtime profile
```

### 5.3 命令构造

命令必须使用 exec array，不允许拼接字符串交给 shell：

```text
npm restore  -> ["npm", "install", "--ignore-scripts", "--audit=false", "--fund=false", "--registry", registry]
npm add      -> ["npm", "install", "--ignore-scripts", "--audit=false", "--fund=false", "--registry", registry, ...packages]
pnpm restore -> ["pnpm", "install", "--ignore-scripts", "--config.audit=false", "--registry", registry]
pnpm add     -> ["pnpm", "add", "--ignore-scripts", "--config.audit=false", "--registry", registry, ...packages]
```

说明：

- `restore` 表达用户期望的真实 `npm install` / `pnpm install`，用于安装 package.json/lockfile 中已有依赖。
- `add` 表达新增依赖；npm 生态使用 `npm install <packages>`，pnpm 生态使用 `pnpm add <packages>`。
- lifecycle scripts 默认 deny/ignore，除非 template policy 显式允许。
- 不允许 `sh -c`。
- 不允许模型传入任意 package manager binary。

### 5.4 输出结果

成功返回：

```json
{
  "ok": true,
  "cwd": "project",
  "mode": "add",
  "packageManager": "pnpm",
  "registry": "https://registry.npmjs.org/",
  "packages": ["astro", "tailwindcss"],
  "logPath": "outputs/build/package-install-<toolUseId>.log",
  "exitCode": 0
}
```

失败返回 recoverable error：

```text
package.install failed with status 1; log: /workspace/outputs/build/package-install-<toolUseId>.log
```

如果是 policy deny，返回 permission denied，不进入命令执行。

## 6. `shell.run` 策略

### 6.1 默认规则

继续 deny：

```text
shell.run(["npm", "install", ...])
shell.run(["pnpm", "install", ...])
shell.run(["pnpm", "add", ...])
shell.run(["yarn", "add", ...])
shell.run(["bun", "add", ...])
```

返回 guidance：

```text
Dependency installation must use package.install.
Use package.install({ "packages": ["..."], "packageManager": "pnpm", "cwd": "project" }).
```

允许用于诊断/修复，但不作为正式 promotion evidence：

```text
shell.run(["pnpm", "build"])
shell.run(["npm", "run", "build"])
shell.run(["pnpm", "test"])
shell.run(["npm", "run", "lint"])
```

正式 build 仍必须使用 `project.build`。

### 6.2 local debug 例外

可以新增 runtime/admin 级配置：

```text
RUNTIME_ALLOW_RAW_PACKAGE_SHELL=1
```

仅用于本机 debug，不用于默认 `local-e2e`，更不能用于 production-like profile。

即使启用，也必须：

- 写 audit。
- 标记 `rawPackageShell: true`。
- 在 stream 中显示这是 debug override。
- 不作为默认模型提示路径。

P0 可以不实现这个开关，只在文档中保留为调试逃生口。

## 7. Runtime Policy

### 7.1 Profile

```rust
pub enum RuntimePolicyProfile {
    Production,
    LocalE2e,
}
```

默认：

```text
Production
```

只有 runtime 配置或测试 harness 可以设置 `LocalE2e`，模型不能通过 tool input 覆盖。

### 7.2 Registry 策略

| profile | registry input | 行为 |
| --- | --- | --- |
| `production` | omitted | 使用内部 registry/proxy |
| `production` | internal registry | allow |
| `production` | public npm | deny |
| `local-e2e` | omitted | 使用配置 registry；测试可配置 public npm |
| `local-e2e` | public npm | ask/allow，并写 audit |
| `local-e2e` | internal registry | allow |

策略判断必须基于最终解析后的 registry，而不是只看 tool input：

- 如果 `registry` omitted，但 `ctx.npm_registry` / `RUNTIME_NPM_REGISTRY` 解析后是 public npm，则仍按 public npm 处理。
- 任何 public npm ask/allow 都必须写 audit，不论 public registry 来自 tool input 还是 runtime config。
- production-like profile 下解析后的 registry 是 public npm 时必须 deny。

audit record 至少包含：

```json
{
  "runId": "...",
  "projectId": "...",
  "toolUseId": "...",
  "tool": "package.install",
  "packageManager": "pnpm",
  "cwd": "project",
  "registry": "https://registry.npmjs.org/",
  "packages": ["astro", "tailwindcss"],
  "policyProfile": "local-e2e",
  "decision": "allow"
}
```

## 8. Event Stream

### 8.1 P0 事件模型

当前 runtime 和 shared contract 已有 `tool.started`、`tool.completed`、`tool.failed` 等通用工具事件。P0 不应优先引入 package 专用事件类型，避免扩大 Rust event enum、shared TS schema、BFF 合约和 UI 消费面的改动。

P0 推荐新增或规范化通用事件：

```text
tool.output
```

字段：

```json
{
  "type": "tool.output",
  "runId": "...",
  "tool": "package.install",
  "toolUseId": "...",
  "stream": "stdout",
  "text": "...",
  "timestamp": "..."
}
```

`stream` 只允许：

```text
stdout
stderr
```

最终事件序列：

```text
tool.started
tool.output        # stdout/stderr chunks, optional and throttled
tool.completed     # metadata includes cwd/packageManager/mode/registry/logPath/exitCode
```

失败事件序列：

```text
tool.started
tool.output        # stdout/stderr chunks, optional and throttled
tool.failed        # metadata includes cwd/packageManager/mode/registry/logPath/exitCode when available
```

需要修改 shared contract：

- `services/runtime/src/types.rs`
- `packages/shared/src/events.ts`
- `packages/shared/src/mock-bff-contract-types.test.ts`

如果 P0 暂时不扩展 shared schema，则退化方案是：

- install stdout/stderr 只写 log 文件。
- `tool.completed.metadata` / `tool.failed.metadata` 返回 `logPath`、`stdoutPreview`、`stderrPreview`。
- run events 页面展示 log path 和 preview。

但推荐 P0 实现 `tool.output`，因为用户要求查看生成过程 stream。

### 8.2 日志路径

单次安装日志写入：

```text
outputs/build/package-install-<toolUseId>.log
```

同时维护 latest 指针：

```text
outputs/build/package-install-latest.log
```

返回给模型和 UI 的路径使用 workspace 相对路径，避免暴露宿主机路径。

## 9. 实现计划

### P0.1 扩展工具 schema

修改：

- `services/runtime/src/tools/sandbox.rs`
- `services/runtime/tests/sandbox_tools.rs`
- `services/runtime/tests/tool_registry.rs`

任务：

- `PackageInstallTool` schema 增加 `packageManager`。
- `PackageInstallTool` schema 增加 `mode`，P0 支持 `restore` / `add`。
- `registry` 保持 optional。
- `cwd` 保持 optional，默认 appRoot。
- 校验 `packageManager` 只允许 `npm` / `pnpm`。
- 校验 `mode=restore` 时 `packages` 为空或省略。
- 校验 `mode=add` 时 `packages` 非空。
- 返回结果包含 `cwd`、`registry`、`packageManager`、`mode`、`logPath`。

验收：

- `package.install({ "mode": "restore", "packageManager": "pnpm" })` 构造 `pnpm install`。
- `package.install({ "mode": "add", "packages": ["astro"], "packageManager": "pnpm" })` 构造 `pnpm add astro`。
- `package.install({ "packages": ["astro"] })` 不要求 registry，并推断为 `mode=add`。
- unsupported package manager 返回 recoverable error。
- 不合法 mode/packages 组合返回 recoverable validation error。

### P0.2 实现 package manager 推断

修改：

- `services/runtime/src/tools/sandbox.rs`
- 可能新增 helper：`services/runtime/src/tools/project_state.rs` 或放在现有 sandbox helper 中。

任务：

- 读取 `state/project.json.packageManager`。
- 检测 appRoot 下 lockfile。
- fallback 到 template policy default。
- 最后 fallback `npm`。

验收：

- 有 `pnpm-lock.yaml` 时缺省使用 pnpm。
- 有 `package-lock.json` 时缺省使用 npm。
- `input.packageManager` 优先级最高。

### P0.3 接入真实 spawn 与 streaming

修改：

- `services/runtime/src/tools/sandbox.rs`
- `services/runtime/src/tools/streaming.rs`
- `services/runtime/src/sandbox_adapter.rs` 或 command backend 对应实现。
- `services/runtime/src/types.rs`
- `packages/shared/src/events.ts`
- `packages/shared/src/mock-bff-contract-types.test.ts`

任务：

- 将 `package.install` 从一次性 `Command::output()` 结果升级为 stdout/stderr streaming。
- 每个 chunk emit 通用 `tool.output` run event，必要时做节流和最大 chunk 限制。
- 同时写 `outputs/build/package-install-<toolUseId>.log` 和 `outputs/build/package-install-latest.log`。
- timeout 后 kill process，返回 recoverable error。

验收：

- run events 中能看到 `tool.output` stdout/stderr。
- shared `AgentEventSchema` 可以解析 `tool.output`。
- timeout 后 log path 可读。
- failed install 不会吞掉 stderr。

### P0.3a 确认 pnpm/corepack 可用

修改：

- sandbox image / dev runtime bootstrap。
- `services/runtime/tests/k8s_sandbox_e2e.rs` 或对应 sandbox capability test。

任务：

- 确认 sandbox 内存在 `pnpm`，或通过 corepack 可启用固定版本 pnpm。
- 如果当前镜像不包含 pnpm，P0 需要补镜像依赖或在 runtime bootstrap 中启用 corepack。
- 如果某个 profile 不支持 pnpm，`package.install(packageManager="pnpm")` 必须返回明确 recoverable error。

验收：

- sandbox capability test 能执行 `pnpm --version`。
- 真实 E2E 的 pnpm 路径不会因为 binary missing 失败。

### P0.4 保持 shell install deny，并优化 guidance

修改：

- `services/runtime/src/permission.rs`
- `services/runtime/tests/permission_engine.rs`
- `services/runtime/tests/tool_permissions_integration.rs`

任务：

- deny `npm install`、`pnpm install`、`pnpm add`。
- 补充 `yarn add/install`、`bun add/install` deny。
- deny message 带 `package.install` 等价调用建议。

验收：

- `shell.run(["pnpm","install"])` deny。
- deny reason 包含 `package.install`。
- `shell.run(["pnpm","build"])` 仍可用于 diagnostics/repair。

### P0.5 对齐 prompt

修改：

- `services/runtime/src/agent_loop.rs`
- `services/runtime/tests/agent_loop.rs`

任务：

- Build/Edit prompt 明确：
  - use `package.install` for dependencies。
  - package.install runs the real package manager under policy control。
  - do not call npm/pnpm install through shell.run。
  - use `project.build`, not shell build, for formal candidate。
  - use `fs.patch` / `fs.write_chunk` for large or existing files。

验收：

- prompt 包含 `package.install` 和 `real package manager`。
- prompt 不引导模型调用裸 `npm install`。

### P0.6 覆盖真实 E2E

修改：

- `services/runtime/tests/http_api.rs`
- `services/runtime/tests/astro_build_agent.rs`
- ignored real-provider regression test。

任务：

- 用真实 DeepSeek/OpenAI-compatible provider 跑 `design.md -> Astro website`。
- 测试 pnpm 路径和 npm 路径各一次。
- 验证 stream 包含 `tool.output`，且 `tool="package.install"`。
- 验证最终 preview URL 可访问。
- 验证文件树没有 `project/project`。

验收：

```text
run.complete success
preview URL HTTP 200
events contain tool.started/tool.output/tool.completed for package.install
outputs/build/package-install-latest.log exists
outputs/build/latest.json build status success
state/project.json appRoot == project
no nested project/package.json
```

## 10. 测试矩阵

| 层级 | 用例 | 期望 |
| --- | --- | --- |
| Unit | `package.install` 缺省 registry | 使用 `ctx.npm_registry` |
| Unit | `package.install` 指定 cwd | cwd 必须在 workspace 内 |
| Unit | `package.install mode=restore` | npm/pnpm 使用 install 且 packages 为空 |
| Unit | `package.install mode=add` | npm 使用 install packages，pnpm 使用 add packages |
| Unit | invalid mode/packages | recoverable validation error |
| Unit | `package.install` 指定 pnpm | command argv 使用 pnpm |
| Unit | `package.install` 指定 npm | command argv 使用 npm |
| Unit | unsupported package manager | recoverable error |
| Unit | public npm + production | deny |
| Unit | public npm + local-e2e | allow/ask + audit |
| Unit | omitted registry resolves to public npm in local-e2e | allow/ask + audit |
| Unit | omitted registry resolves to public npm in production | deny |
| Unit | `shell.run pnpm install` | deny + guidance |
| Unit | `shell.run pnpm build` | allow diagnostics |
| Integration | package install stdout | `tool.output` 可见 |
| Integration | package install stderr | `tool.output` 可见且写 log |
| Integration | shared event schema | `tool.output` 可被 TS schema 解析 |
| Integration | sandbox pnpm capability | `pnpm --version` 可用，或返回明确 recoverable error |
| Integration | install timeout | process killed + recoverable error |
| Integration | build after install | `project.build` writes latest status |
| E2E | real DeepSeek website | preview reachable + styled site |
| E2E | no nested app root | 没有 `project/project/package.json` |

## 11. 风险与处理

### 风险：专用工具被误解为不真实

处理：

- 文档和 prompt 明确 `package.install` runs the real package manager。
- stream 展示真实 stdout/stderr。
- 返回真实 command summary，但不暴露敏感 registry token。

### 风险：pnpm 不存在于 sandbox image

处理：

- P0 检查 sandbox image 是否包含 pnpm。
- 如果没有，提供两种选项：
  - 在 sandbox image 预装 pnpm/corepack。
  - `package.install` 对 pnpm 返回 recoverable error，提示当前 profile 仅支持 npm。
- 推荐预装 pnpm，因为 Astro/Tailwind/Fumadocs 生态中 pnpm 很常见。

### 风险：lifecycle scripts 被禁导致某些包不可用

处理：

- P0 默认 `--ignore-scripts`。
- P1 引入 template policy allowlist。
- 如果失败，错误信息说明 lifecycle scripts disabled，而不是只返回 install failed。

### 风险：local-e2e public npm 和 production 内部 registry 行为不同

处理：

- run event 和 audit 记录 registry 与 policy profile。
- local-e2e 测试报告中标记 registry 来源。
- production-like regression 使用内部 registry/proxy fixture。

### 风险：shell.run raw package command 被模型反复重试

处理：

- deny message 给出等价 `package.install` 示例。
- agent loop 识别 repeated shell install deny，两次后插入 recovery hint。
- prompt 明确不要重试同一裸 install 命令。

## 12. 验收标准

完成本方案后，应满足：

1. 模型可以通过 `package.install(mode=restore)` 触发真实 `npm install` / `pnpm install`。
2. 模型可以通过 `package.install(mode=add)` 触发真实新增依赖安装。
3. 用户在 `/runs/{runId}/events` 能通过 `tool.output` 看到真实安装 stream。
4. 本地真实 API 回归可以生成有样式 Astro website。
5. production-like profile 不允许 public npm 绕过内部 registry/proxy。
6. 解析后的 public npm registry 在 local-e2e 下有 audit，在 production-like 下 deny。
7. `shell.run npm/pnpm install` 仍被禁止，但错误可指导模型自动修复。
8. `project.build` 和 `preview.start` 继续使用 appRoot，不受 package install cwd 影响。
9. 文件树不会出现 `project/project/package.json`。

## 13. 推荐实施顺序

1. 扩展 `PackageInstallTool` schema：`mode`、`packageManager`、返回字段、cwd/appRoot。
2. 实现 `restore/add` 的 npm/pnpm command builder。
3. 实现 package manager 推断和 pnpm/corepack capability 检查。
4. 保持并强化 shell install deny guidance。
5. 接入 `tool.output` stdout/stderr streaming 和 install log。
6. 更新 shared event schema 和 mock BFF contract 测试。
7. 补 unit/integration 测试。
8. 更新 Build/Edit prompt。
9. 跑真实 DeepSeek design.md website 回归，保存 stream URL、preview URL、文件树和关键事件摘要。

这组调整完成后，`package.install` 就会从"看起来像专用工具"变成"真实包管理器执行的产品化入口"：既满足开发体验，又保留 harness 必须承担的安全、可复现和可观测边界。
