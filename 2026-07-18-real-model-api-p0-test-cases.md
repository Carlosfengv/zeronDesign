---
date: 2026-07-18
status: proposed-for-review
type: end-to-end-test-cases
scope: p0
topic: real-model-api-website-docs-generation
---

# Website 与 Docs P0 真实模型 API 测试用例

## 1. 文档目的

本文定义四个 P0 端到端测试用例：Website 两个、Docs 两个。

本文保留为现有 `next-app` / Fumadocs Docs 生成与发布链路的真实模型基线，不单独构成
视觉 React P0 的完成证据。`next-app`、多模态视觉比较、Draft HMR、组件/资产来源、元素
定位和版本恢复的新增验收矩阵定义在
`2026-07-19-visual-react-runtime-capability-implementation-plan.md`；相关 Wave 落地后，
必须增加独立的 React/视觉真实模型 E2E，不能用本文已有 Website 通过结果替代。

所有用例必须使用真实模型、真实 Provider Gateway、真实 Runtime HTTP API、真实模板构建、真实浏览器预览和真实发布 API。不得使用 `MockModelClient`、预置生成结果、直接写 Runtime Store、跳过模型调用或手工伪造 Run 完成状态。

测试覆盖以下用户主链路：

```text
创建项目
-> 输入 Prompt / Markdown
-> 选择 Design Profile 和模型
-> AI 分析并生成 Brief
-> 用户确认内容方案
-> AI 生成 Website 或 Docs
-> 用户预览并通过自然语言修改
-> 用户确认版本
-> 创建 Release 并发布
-> 通过固定 URL 访问
```

## 2. 用例概览

| 用例 | 类型 | 输入方式 | 核心验证目标 |
|---|---|---|---|
| `WEB-E2E-01` | Website | 纯 Prompt | 首次生成、Design Profile、自然语言编辑、首次发布 |
| `WEB-E2E-02` | Website | Prompt + Markdown | 内容保真、高影响修改确认、版本隔离、更新发布 |
| `DOCS-E2E-01` | Docs | Prompt + 多个 Markdown | 文档信息架构、代码内容、导航、编辑和发布 |
| `DOCS-E2E-02` | Docs | 冲突 Markdown + Prompt | 澄清问题、内容来源、版本恢复、发布失败保护 |

## 3. 通用前置条件

### 3.1 运行环境

1. Runtime、Provider Gateway、Web/BFF、Sandbox、Release Packager、对象存储和 Kubernetes Publication Controller 已部署。
2. `next-app` 和 `fumadocs-docs` 模板处于可用状态。
3. npm/pnpm Registry 可访问。
4. Browser Collector 和截图能力可用。
5. Kubernetes Ingress、TLS Secret 和外部测试域名已配置。
6. 每个用例使用独立的 `projectId` 和 `workspaceNamespace`，禁止复用其他用例状态。

### 3.2 真实模型配置

推荐使用已注册的真实模型资源：

```text
modelResourceId=deepseek-design-balanced
physicalModel=deepseek-v4-pro
providerBaseUrl=https://api.deepseek.com
```

允许使用测试环境中的其他真实模型资源，但必须满足：

- Model Resource 已启用。
- 支持 Tool Calls。
- 当前 Workspace 或 Project Policy 允许直接选择该资源。
- Provider API Key 来自 Secret，不出现在请求体、日志或证据文件中。
- Provider Gateway 返回实际 `modelResourceId`、物理模型、请求标识和 Token 用量。

测试执行前至少确认：

```text
DEEPSEEK_API_KEY 已配置
RUNTIME_PROVIDER_APPROVAL_ID 已配置
MODEL_PROVIDER 不是 mock
MODEL_RESOURCE_ID=deepseek-design-balanced
```

### 3.3 Design Profile

测试前通过真实 API 导入并激活 Design Profile。可以使用以下基准文件，但导入时必须调整为当前测试项目作用域：

- `services/runtime/fixtures/design-profiles/authkit-v2.json`
- `services/runtime/fixtures/design-profiles/elevenlabs-v2.json`

记录最终获得的：

- `designProfileId`
- `designProfileVersion`
- `designProfileHash`
- `designSourceHash`

### 3.4 API 使用规则

可以通过 Web/BFF 创建项目，也可以通过受保护的内部 API 注册项目访问关系。项目创建完成后，生成主链路必须使用真实 HTTP API。

关键 API 包括：

| 阶段 | API |
|---|---|
| 创建 Brief Run | `POST /runs` |
| 查看 Run 事件 | `GET /runs/{runId}/events` |
| 继续或回答问题 | `POST /runs/{runId}/continue` |
| 查看 Brief | `GET /briefs/{briefId}` |
| 确认 Brief | `POST /briefs/{briefId}/confirm` |
| 创建 Build Run | `POST /runs`，`phase=build` |
| 查看项目状态 | `GET /projects/{projectId}/runtime-state` |
| 查看当前预览 | `GET /preview/{projectId}/current` |
| 查看固定版本 | `GET /preview/{projectId}/{versionId}` |
| 创建 Edit Run | `POST /runs`，`phase=edit` |
| 创建 Release | `POST /projects/{projectId}/versions/{versionId}/releases` |
| 发布或更新 | `POST /projects/{projectId}/publish` |
| 查看发布 Operation | `GET /operations/{operationId}` |
| 查看线上状态 | `GET /projects/{projectId}/deployment-state` |
| 回滚发布 | `POST /projects/{projectId}/rollback` |

所有写操作必须携带合法身份和必要的 Idempotency Key。发布和更新必须遵守 `If-None-Match` 或 `If-Match` 预条件。

## 4. 通用 API 流程

### 4.1 创建 Brief Run

```json
{
  "projectId": "<project-id>",
  "phase": "brief",
  "agentProfile": "brief",
  "inputContext": {
    "contentSources": [
      {
        "id": "prompt-1",
        "kind": "prompt",
        "text": "<用户需求>",
        "readable": true
      }
    ],
    "designProfileId": "<design-profile-id>",
    "modelResourceId": "deepseek-design-balanced"
  }
}
```

### 4.2 确认 Brief 后创建 Build Run

```json
{
  "projectId": "<project-id>",
  "phase": "build",
  "agentProfile": "build",
  "inputContext": {
    "briefId": "<confirmed-brief-id>",
    "designProfileId": "<design-profile-id>",
    "designFidelityMode": "profile_only",
    "modelResourceId": "deepseek-design-balanced"
  }
}
```

### 4.3 创建 Edit Run

先通过 `GET /projects/{projectId}/runtime-state` 取得 `currentVersionId` 和 `sandboxBindingId`，再创建 Edit Run：

```json
{
  "projectId": "<project-id>",
  "phase": "edit",
  "agentProfile": "edit",
  "inputContext": {
    "baseVersionId": "<current-version-id>",
    "sandboxBindingId": "<sandbox-binding-id>",
    "modelResourceId": "deepseek-design-balanced"
  }
}
```

随后调用：

```json
{
  "userMessage": "<自然语言修改要求>"
}
```

### 4.4 发布

1. 为当前 Promoted Version 创建 Release。
2. 轮询 Packaging，直到 Release 状态为 `validated`。
3. 首次发布使用 `If-None-Match: *`。
4. 更新发布使用当前 Release ID 作为 `If-Match`。
5. 轮询 Operation，直到 `completed` 或失败终态。
6. 查询 Deployment State，并通过返回的 `publicUrl` 发起真实 HTTPS 请求。

## 5. Website 测试用例

### WEB-E2E-01：纯 Prompt 生成、编辑并首次发布 Website

**优先级：** P0<br>
**Design Profile：** AuthKit V2<br>
**真实模型：** `deepseek-design-balanced`<br>
**模板预期：** `next-app`

#### 测试目标

验证用户只提供自然语言需求时，可以完成 Website 的 Brief、生成、预览、自然语言修改和首次发布完整闭环。

#### 测试输入

```text
为 CloudDesk 创建一个面向 20 到 200 人远程团队的 SaaS 营销网站。
需要首页、功能、价格和联系我们四个页面。
首页必须展示标题“Ship work, not status updates”和按钮“Start a 14-day trial”。
价格页包含 Starter、Team、Business 三个套餐。
整体风格专业、清晰、有可信度，并遵循选择的 Design Profile。
```

#### 测试步骤

1. 创建 Website 项目并完成项目访问注册。
2. 导入、激活并绑定 AuthKit V2 Design Profile。
3. 调用 `POST /runs` 创建真实 Brief Run，同时指定 `modelResourceId`。
4. 读取 SSE，等待 AI 写入 Brief Draft 并进入 `needs_user_input`。
5. 获取 Brief，检查项目类型、受众、页面结构、视觉方向、假设和缺失信息。
6. 确认 Brief，验证确认前不能启动正式 Build。
7. 创建真实 Build Run，持续记录 SSE，直到 `preview.updated` 和 `run.completed`。
8. 获取当前 Preview 和 Artifact，使用真实浏览器访问所有页面。
9. 在 Desktop 及 375px Mobile 视口检查布局。
10. 创建 Edit Run，提交：

```text
将首页主按钮背景改为橙色，并在价格页 Team 套餐上增加“Most Popular”标记；不要修改其他页面文案。
```

11. 等待新版本生成，验证首页指定按钮和价格页标记发生变化，其他文案保持不变。
12. 为新版本创建 Release，等待 Packaging 验证通过。
13. 首次发布并轮询 Operation 至完成。
14. 访问 `publicUrl`，验证线上内容和新版本 Release 身份。

#### 预期结果

- Provider Gateway 真实调用指定模型资源，不得回退到 Mock。
- Brief 的 `projectType` 为 `website`，推荐模板为 `next-app`。
- Brief 至少包含 `/`、`/features`、`/pricing`、`/contact` 对应页面。
- 未确认 Brief 前，Build 请求被拒绝或进入等待确认状态。
- 生成结果包含全部必需页面、精确标题和 CTA 文案。
- Desktop 和 Mobile 均无页面级横向溢出。
- Edit 只影响指定按钮样式和 Team 套餐标记。
- Edit 前后形成不同的 Version ID。
- 发布成功后返回固定 HTTPS URL。
- 外部 URL 返回成功状态，并携带与当前 Release 一致的身份信息。

#### 必须保存的证据

- Project、Brief、Build Run、Edit Run、Version、Release、Operation ID。
- Provider Gateway 请求 ID、模型资源、物理模型和 Token 用量。
- Brief JSON、完整 SSE、Runtime State 和 Preview JSON。
- 修改前后 Desktop/Mobile 截图。
- Release Packaging 证据和外部 URL 探测结果。

---

### WEB-E2E-02：Markdown 内容生成、高影响修改确认和更新发布

**优先级：** P0<br>
**Design Profile：** ElevenLabs V2<br>
**真实模型：** `deepseek-design-balanced`<br>
**模板预期：** `next-app`

#### 测试目标

验证 Prompt 和 Markdown 可以共同生成 Website，并验证内容保真、高影响修改确认、草稿与线上版本隔离以及固定 URL 更新发布。

#### 测试数据

`acme-site-content.md`：

```md
# Acme Analytics

Acme Analytics helps operations teams understand delivery risk.

## Product

- Live delivery dashboard
- Risk alerts
- Weekly executive summary

## Pricing

- Basic: $29/month
- Pro: $99/month
- Enterprise: Contact sales

## FAQ

### Is a credit card required for the trial?

No.
```

Prompt：

```text
基于附件创建 Acme Analytics 产品网站，包含首页、产品、价格和 FAQ。
附件中的产品能力、价格和 FAQ 答案属于事实，不得改变数值或含义。
```

#### 测试步骤

1. 创建 Website 项目，导入并绑定 ElevenLabs V2 Design Profile。
2. 创建 Brief Run，在 `contentSources` 中同时传入 Prompt 和 Markdown 文本。
3. 等待 Brief Draft，检查系统是否区分原始事实、AI 改写、AI 新增和待确认内容。
4. 确认附件中的价格、功能和 FAQ 没有被改错后确认 Brief。
5. 创建 Build Run并验证所有页面、导航和附件事实。
6. 创建 Release A 并首次发布，记录固定 URL 和 Release A。
7. 创建 Edit Run，提交高影响修改：

```text
删除价格页面，并从全站导航中移除 Pricing；同时把首页改成单页长页面。
```

8. 验证系统在执行前返回影响范围，至少列出价格页、全站导航和首页布局，并等待用户确认。
9. 取消第一次修改，确认未产生新的 Promoted Version。
10. 再次提交相同修改并明确确认，等待生成 Version B。
11. Version B 生成后但发布前，访问固定 URL，确认仍展示 Release A 和原价格页。
12. 为 Version B 创建 Release B 并执行更新发布。
13. 验证发布后固定 URL 不变，但内容切换到 Release B。

#### 预期结果

- Prompt 和 Markdown 均被真实模型读取。
- 附件中的 `$29`、`$99`、`No.` 等事实保持正确。
- AI 新增事实必须标记并经过确认，不能静默进入生成结果。
- 删除页面和全站导航修改被识别为高影响操作。
- 用户取消后不修改源代码、不提升 Candidate、不产生可发布版本。
- 草稿 Version B 不影响线上 Release A。
- 更新发布成功前，固定 URL 始终可以访问 Release A。
- 更新完成后 URL 不变，Release 身份变为 B。

#### 必须保存的证据

- Prompt 和 Markdown ContentSource ID 及摘要 Hash。
- Brief 中的来源标记和用户确认记录。
- 高影响修改的影响范围、取消记录和确认记录。
- Version A/B、Release A/B、发布前后外部请求结果。
- Provider Gateway 两个阶段的实际模型和 Token 用量。

## 6. Docs 测试用例

### DOCS-E2E-01：多 Markdown 生成、编辑并发布 Docs

**优先级：** P0<br>
**Design Profile：** AuthKit V2<br>
**真实模型：** `deepseek-design-balanced`<br>
**模板预期：** `fumadocs-docs`

#### 测试目标

验证多个 Markdown 内容源可以生成具有正确层级、导航、代码块和响应式表现的 Docs，并支持自然语言编辑和发布。

#### 测试数据

`overview.md`：

```md
# Zeron SDK

Zeron SDK is a TypeScript client for the Zeron API.

## Requirements

- Node.js 20+
- pnpm 9+
```

`installation.md`：

````md
# Installation

```bash
pnpm add @zeron/sdk
```
````

`quickstart.md`：

````md
# Quickstart

```ts
import { ZeronClient } from "@zeron/sdk";

const client = new ZeronClient({ apiKey: process.env.ZERON_API_KEY });
```
````

`api-reference.md`：

```md
# API Reference

| Method | Description |
|---|---|
| `client.projects.list()` | List projects |
| `client.projects.get(id)` | Get a project |
```

#### 测试步骤

1. 创建 Docs 项目并绑定 AuthKit V2 Design Profile。
2. 创建 Brief Run，将需求 Prompt 和四个 Markdown 文件作为独立 ContentSource 传入。
3. 等待真实模型生成 Brief，检查项目类型、内容层级、导航和推荐模板。
4. 确认 Brief 后创建 Build Run。
5. 等待真实 Fumadocs 构建、Preview 提升和 Run 完成。
6. 在浏览器中依次访问 Overview、Installation、Quickstart、API Reference。
7. 验证代码块、表格、侧边栏、面包屑和移动端导航。
8. 创建 Edit Run，提交：

```text
新增 Troubleshooting 页面，包含“Invalid API key”和“Network timeout”两个小节；保留已有代码示例和导航顺序。
```

9. 验证新页面加入导航，原四个页面内容不变。
10. 创建 Release 并首次发布。
11. 通过固定 URL 访问全部文档路由和静态资源。

#### 预期结果

- Brief 的 `projectType` 为 `docs`，推荐模板为 `fumadocs-docs`。
- 四个 Markdown 内容源均进入内容层级，不被合并丢失。
- 代码示例中的包名、环境变量和方法名保持精确。
- Fumadocs Production Build 成功。
- 侧边栏、移动端导航和全部内部链接正常工作。
- 新增 Troubleshooting 后形成新版本，旧页面内容不被重写。
- 发布 URL 可以访问所有路由、CSS、JavaScript 和静态资源。

#### 必须保存的证据

- 四个 ContentSource ID 和 Hash。
- Brief、Run SSE、Build Log、Preview 和 Version ID。
- 每个路由的 HTTP 状态和关键文本断言。
- Desktop/Mobile 截图。
- Provider Gateway 模型执行与 Token 用量。
- Release、Operation、Deployment State 和外部访问结果。

---

### DOCS-E2E-02：冲突澄清、内容来源、版本恢复和发布失败保护

**优先级：** P0<br>
**Design Profile：** ElevenLabs V2<br>
**真实模型：** `deepseek-design-balanced`<br>
**模板预期：** `fumadocs-docs`

#### 测试目标

验证真实模型能够发现多个 Markdown 文件中的关键冲突，等待用户澄清后生成 Docs；同时验证版本恢复和更新发布失败时旧线上版本保持可用。

#### 测试数据

`install-current.md`：

````md
# Installation

Current stable release is v3.

```bash
pnpm add @nova/sdk@3
```
````

`install-legacy.md`：

````md
# Installation

Current stable release is v2.

```bash
npm install @nova/sdk@2
```
````

Prompt：

```text
根据附件生成 Nova SDK 文档站。不要自行决定冲突的稳定版本；如果资料冲突，必须先询问我。
```

#### 测试步骤

1. 创建 Docs 项目并绑定 ElevenLabs V2 Design Profile。
2. 创建 Brief Run，传入 Prompt 和两个冲突 Markdown。
3. 读取 SSE，验证 Run 进入 `needs_user_input`，问题明确指出 v2/v3 或安装命令冲突。
4. 回复：

```text
以 v3 和 pnpm 命令为准；v2 内容放入 Legacy migration 页面。
```

5. 等待新的 Brief Draft，检查来源标记、冲突解决结果和页面结构。
6. 确认 Brief，创建 Build Run，生成 Version A。
7. 验证 Installation 使用 v3，Legacy migration 保留 v2 信息。
8. 创建 Release A 并发布，记录固定 URL。
9. 创建 Edit Run，将首页标题修改为“Nova SDK Documentation”，生成 Version B。
10. 通过版本记录恢复到 Version A，确认当前预览重新对应 A。
11. 再次生成 Version C，并为其创建 Release C。
12. 在受控测试环境对 Release C 注入外部探测失败或目标 Workload 不可就绪故障。
13. 调用更新发布 API，等待 Operation 进入失败或 `reconcile_required` 状态。
14. 持续访问固定 URL，验证仍然返回 Release A。
15. 移除故障并重试发布，确认最终切换到 Release C，URL 保持不变。

#### 预期结果

- 真实模型识别冲突并先提问，不静默选择 v2 或 v3。
- 澄清答案进入对话和 Brief，后续 Build 使用已确认结果。
- Installation 和 Legacy migration 内容来源可追溯。
- Version A、B、C 均可识别，恢复 Version A 不删除其他历史版本。
- Release C 更新失败期间，线上 Release A 始终可访问。
- 失败 Operation 包含可理解的阶段和错误，不泄露 Secret。
- 故障解除后可安全重试，并最终切换到 Release C。

#### 必须保存的证据

- 冲突 ContentSource、AI 澄清问题、用户回答和 Brief 变更记录。
- Version A/B/C 与恢复操作记录。
- Release A/C、故障注入说明、失败 Operation 和重试 Operation。
- 故障期间按固定间隔采集的外部 URL 状态与 Release 身份。
- Provider Gateway 请求 ID、真实模型身份和 Token 用量。

## 7. 通用断言

每个用例都必须验证以下断言：

### 7.1 真实模型断言

- Provider Gateway 存在对应 Run 和 Turn 的真实调用记录。
- `modelResourceId` 与测试指定值一致。
- Provider 返回的物理模型非 `mock`、`fixture` 或空值。
- 输入和输出 Token 用量均大于零。
- Provider Gateway 内部审计可以关联请求、Run 和 Project；导出证据只记录 `providerRequestIdPresent=true`，不保存 Provider 原始请求 ID。
- 日志中不存在 API Key 或完整 Authorization Header。

### 7.2 Brief 断言

- Brief 在用户确认前处于 Draft 或等待确认状态。
- Brief 包含正确的 `projectType`、受众、内容层级、页面结构、视觉方向和模板。
- 用户确认后 Brief 状态为 Confirmed。
- Build 使用的 `briefId` 与用户确认的版本一致。

### 7.3 生成和预览断言

- Run 事件包含开始、工具执行、构建、Candidate、Preview 更新和完成状态。
- `preview.updated` 发生在成功完成之前。
- 只有通过构建与验证的 Candidate 才能成为 Promoted Version。
- 新生成失败时，上一成功 Preview 保持可用。
- Website 和 Docs 均通过 Desktop 与 375px Mobile 检查。

### 7.4 编辑和版本断言

- Edit 必须基于当前 `baseVersionId`。
- Stale Version 修改请求被拒绝，不能覆盖新版本。
- 成功修改产生新的不可变 Version。
- 未要求修改的页面和关键文本保持不变。
- 失败 Edit 不改变当前 Promoted Version。

### 7.5 发布断言

- 只有 Promoted Version 可以创建 Release。
- 只有 Validated Release 可以发布。
- 首次发布和更新发布使用正确的幂等键及并发预条件。
- 固定 URL 在更新发布后保持不变。
- 新版本完成外部探测前，旧线上 Release 不得被视为发布成功。
- 发布失败不破坏最后一个成功 Release。

## 8. 证据目录建议

每个用例使用独立目录：

```text
.runtime-evidence/<case-id>-<timestamp>/
├── run-metadata.env
├── project.json
├── content-sources.json
├── brief.json
├── brief-events.jsonl
├── build-events.jsonl
├── edit-events.jsonl
├── provider-executions.json
├── token-usage.json
├── runtime-state.json
├── preview.json
├── versions.json
├── release.json
├── packaging.json
├── publication-operation.json
├── deployment-state.json
├── external-probe.json
└── screenshots/
```

`run-metadata.env` 只记录非敏感元数据，不得写入 Provider API Key、Bearer Token、Cookie 或其他 Secret。

## 9. 通过准则

单个用例通过需要满足：

1. 真实模型断言全部通过。
2. API 主链路无人工修改 Runtime Store 或生成产物。
3. 所有该用例的功能预期均通过。
4. Build、Preview、Version、Release 和外部 URL 证据完整。
5. 没有 Secret 泄露或跨项目访问。

P0 总体验收要求：

- `WEB-E2E-01`、`WEB-E2E-02`、`DOCS-E2E-01`、`DOCS-E2E-02` 四个用例全部通过。
- Website 和 Docs 都至少完成一次首次发布和一次发布后访问验证。
- Website 至少完成一次成功更新发布。
- Docs 至少完成一次受控发布失败保护及恢复验证。
- 任一用例使用 Mock、跳过真实模型、跳过真实构建或跳过外部 URL 验证，均不得计为通过。

## 10. 2026-07-18 阶段执行结果

本轮已经完成真实 `deepseek-v4-pro` 的两个生成与外部 HTTPS 验证子集，但尚不等同于第 9 节完整 P0 通过：

- Website `zenova-agent-cloud`：Brief 与 Build accepted，真实 Token `153926`，验证 URL 为 `https://real-e948643ffd91995dd539.works.zerondesign.localhost:18443/`；
- Docs `agent-cloud-quickstart`：修复 `build.missing_dependency` 未进入 repair state 的 Runtime 缺陷后，Brief 与 Build accepted，真实 Token `745723`，验证 URL 为 `https://real-066ab0c8d89e3606ae91.works.zerondesign.localhost:18443/docs/`；
- 两个子集均验证 Provider Resource/Policy revision、非估算 Token、真实构建、Artifact manifest、预期文本、精确域名 TLS 和 Release identity；原始 Provider request id 与 API Key 均未写入证据；
- 当前外部发布属于隔离的 `validation` Publication，尚未经过产品 Release API 和 Release Packager；稳定性审计也尚未达到连续 `3/3`。

因此当前状态为“真实 Provider 首个 Website/Docs 生成闭环通过，完整 P0 仍在进行中”。剩余必须执行：Edit/并发保护、失败发布保持旧版本、版本恢复、澄清流、产品 Release API，以及连续稳定性门禁。
