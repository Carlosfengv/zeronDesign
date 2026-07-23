---
date: 2026-07-13
status: proposed-for-review
type: product-user-story-map
topic: rapid-generation-preview-edit-version-publish
source: 2026-07-13-v0-lovable-productization-implementation-plan.md
---

# zeronDesign 快速生成、预览、编辑与发布 User Stories

## 1. 文档用途

本文把产品化实施方案转换为可排期、可验收的 User Story Backlog，覆盖：

```text
创建项目 -> 输入内容 -> 确认 Brief -> 生成与预览 -> 对话修改
-> 形成不可变版本 -> 一键发布 -> 稳定 Live URL
-> 更新发布 -> 回滚/下线/重新发布
```

本文只描述用户可感知结果。Create Release API、Store Adapter、Outbox、Sandbox Controller
等纯技术工作不伪装成 User Story，而是在第 10 节作为 Enablement Backlog 单独列出。

## 2. 产品交付分层

| 交付层 | Wave | 用户获得的结果 |
|---|---|---|
| Internal Alpha | P0 + P1 | 可以创建、生成、查看 Promoted Preview、对话编辑、查看版本；预览可以先由完整 Build 驱动 |
| 可发布 MVP / Internal Beta | P0 + P1 + P2 | 可以一键发布、获得稳定 HTTPS URL、更新、回滚、下线和重新发布 |
| v0/Lovable 体验版 | P0 + P1 + P2 + P3 | Draft Preview 常驻，文本/CSS 秒级刷新，错误可见且不破坏上一成功版本 |
| Public Production | P0–P4 + Production Gates | 多副本持久化、备份恢复、配额、审计、滥用防护和正式运行保障 |

关键判断：P0–P2 代表“完整可发布”；只有完成 P3，才可以对外宣称具备接近
v0.dev / Lovable 的快速编辑预览体验。

## 3. 用户角色

| 角色 | 说明 | 核心目标 |
|---|---|---|
| Builder | 项目 Owner 或 Editor | 创建、生成、编辑、形成版本并发布作品 |
| Reviewer | 有项目读取权限的成员 | 查看固定版本、验证结果并反馈 |
| Organization Admin | 组织管理员 | 管理项目成员和访问范围，防止跨项目泄露 |
| Published Visitor | 访问公开 Live URL 的访客 | 始终访问当前正式发布内容 |
| Platform Operator | 内部平台运维人员 | 定位失败、恢复操作、回滚和安全下线 |

## 4. Story 优先级规则

| 标识 | 含义 |
|---|---|
| Must | 对应交付层不可缺少，缺失则该层 No-Go |
| Should | 不阻断首个闭环，但会显著影响可用性或运营效率 |
| Could | 可在后续迭代补充，不进入当前 MVP 承诺 |

## 5. Epic A：身份、项目与访问控制

### US-001 登录并查看项目列表 `[Must][P1]`

作为 Builder，我希望登录后看到自己有权访问的项目，从而可以继续之前的工作。

验收标准：

- 登录成功后进入 `/projects`，只返回本人所属组织及有权限的项目。
- 每个项目显示名称、Website/Docs 类型、最近状态、最后修改时间和发布状态。
- 刷新页面后登录态和项目列表可恢复。
- 未登录访问项目页时跳转登录，不向浏览器暴露 Runtime 内部地址。

### US-002 创建 Website 或 Docs 项目 `[Must][P1]`

作为 Builder，我希望创建 Website 或 Docs 项目，从而让系统选择正确的生成模板和运行能力。

验收标准：

- 用户至少填写项目名称并选择 `website` 或 `docs`。
- 创建成功后生成唯一 `projectId`，并进入内容输入或 Brief 流程。
- 重复提交不会产生多个项目。
- MVP 不显示自定义 Dockerfile、自定义运行镜像或自定义后端选项。

### US-003 项目数据严格隔离 `[Must][P0]`

作为项目 Owner，我希望其他项目成员无法读取或修改我的项目，从而保护源码、对话、预览和发布内容。

验收标准：

- 无权限用户不能读取 Conversation、SSE、Runtime State、Draft/Promoted Preview、Version Artifact。
- 无权限用户不能 Start、Continue、Cancel Run 或处理 Permission。
- 跨项目 token 返回 403/404，响应不得泄露目标项目是否存在。
- SSE token 不写入 URL；浏览器不直接获得 Runtime principal。
- Published Live URL 的公开访问与 Authoring 项目权限相互隔离。

### US-004 刷新后恢复项目工作状态 `[Must][P1]`

作为 Builder，我希望刷新或重新打开页面后恢复当前工作状态，从而不会因为浏览器中断而丢失进度。

验收标准：

- 恢复 Conversation、当前 Run、当前 Promoted Version 和正在进行的 Operation。
- SSE 使用 Last-Event-ID 重连，不重复展示已处理消息。
- 正在运行的后端任务不因浏览器离开而被取消。
- 如果 Runtime 已恢复任务，UI 展示恢复后的真实状态而不是永久 Loading。

## 6. Epic B：内容输入与 Brief 确认

### US-010 使用 Prompt 创建内容 `[Must][P1]`

作为 Builder，我希望用自然语言描述我要的网站或文档，从而无需先准备完整设计稿或代码。

验收标准：

- 支持多行 Prompt，并明确显示提交状态。
- 空 Prompt 或超出限制的输入在客户端和服务端均被拒绝。
- 提交后立即进入 Brief 生成状态，并显示可理解的进度。
- 原始 Prompt 作为项目 ContentSource 可追溯，但不在日志中泄露敏感 token。

### US-011 使用 Markdown 或附件创建内容 `[Must][P1]`

作为 Builder，我希望上传 Markdown 或支持的附件，从而基于已有资料生成 Website 或 Docs。

验收标准：

- 上传前显示支持的文件格式和大小限制。
- 上传内容被保存为不可变 ContentSource，并显示文件名、类型和处理状态。
- 不支持、损坏或超限文件返回明确错误，不启动错误 Run。
- Website 与 Docs 都能从至少一种真实附件完成生成 E2E。

### US-012 查看结构化 Brief `[Must][P1]`

作为 Builder，我希望系统先把输入整理成结构化 Brief，从而在消耗生成时间前确认页面目标和结构。

验收标准：

- Brief 至少展示项目类型、目标用户、页面/内容层级、视觉方向、假设和缺失信息。
- Brief 生成过程通过 Activity/SSE 可见。
- Brief 失败时显示可重试错误，并保留原始输入。
- 未确认 Brief 前不能进入正式 Build。

### US-013 修改并确认 Brief `[Must][P1]`

作为 Builder，我希望修正并确认 Brief，从而确保后续生成使用我认可的范围。

验收标准：

- 用户可以针对缺失信息补充说明，或要求系统重新生成 Brief。
- 点击确认前展示最终结构；确认操作幂等。
- 确认后启动 Build，并锁定本次 Build 使用的 Brief 版本。
- 后续编辑不能静默修改已确认 Brief 的历史内容。

## 7. Epic C：生成工作台与可信预览

### US-020 在统一工作台观察生成过程 `[Must][P1]`

作为 Builder，我希望左侧看到对话和执行状态、右侧看到预览，从而理解系统正在做什么。

验收标准：

- Workspace 同时提供 Chat/Activity 和 Preview 区域。
- UI 能区分 `generating`、`building`、`build_failed`、`promoted` 等状态。
- 工具执行、权限请求和错误以用户可理解的形式展示，不暴露凭证或内部堆栈。
- 页面不能只依赖 `/health` 判断生成是否成功。

### US-021 看到首次可用 Preview `[Must][P1；P3 加速]`

作为 Builder，我希望生成过程中尽快看到首个可用画面，从而能尽早判断方向是否正确。

验收标准：

- P1 至少在首次成功 Production Build 后展示 Promoted Preview。
- P3 完成后，Brief 确认到首次 Draft 可见 P50 不超过 15 秒。
- Preview iframe 在 Ready 后再替换旧画面，切换期间不显示白屏。
- Preview 资源全部经 BFF 代理加载，浏览器不直连 Sandbox。

### US-022 区分 Draft、Promoted 与 Published `[Must][P1/P2/P3]`

作为 Builder，我希望明确知道当前看到的是草稿、可信版本还是线上版本，从而避免误发布或误分享。

验收标准：

- Preview 顶部明确显示 `Draft` 或 `Promoted` 标签。
- `preview.candidate` 不能让 UI 标记为 Promoted；只有 `preview.updated` 可以切换 Promoted。
- Published 状态单独显示 Live URL 和当前 Release identity。
- Draft 永远不能直接成为 Publish target。

### US-023 构建失败时保留上一成功画面 `[Must][P1]`

作为 Builder，我希望新生成或编辑失败时仍能看到上一成功版本，从而不会因一次错误失去可工作的基线。

验收标准：

- Build 失败不改变 `currentVersionId`。
- 右侧继续显示上一 Promoted Preview，并显示“构建失败”状态。
- 左侧提供结构化错误、失败阶段和可重试动作。
- 如果项目从未成功生成，则显示明确空状态而不是空白 iframe。

### US-024 响应需要用户确认的权限请求 `[Must][P1]`

作为 Builder，我希望在 Agent 需要敏感操作授权时查看请求、修改输入、允许或拒绝，从而保持对执行行为的控制。

验收标准：

- 权限卡片显示工具、原因和待处理状态，不显示原始 secret。
- 用户可以 Allow、Ask 或 Deny；`updatedInput` 在 Allow 后用于匹配工具的下一次重试。
- 修改后的输入重新经过 schema 和硬性 deny policy，用户批准不能绕过平台禁令。
- 同一批准只能消费一次；刷新或 Runtime 重启后结果保持一致。
- 只有项目 Editor/Owner 可以处理 Permission。

### US-025 控制 Preview 视口和打开方式 `[Should][P1]`

作为 Builder，我希望切换常用视口、刷新预览并在新窗口打开，从而检查响应式表现和完整页面。

验收标准：

- 至少支持 Desktop 和 Mobile 视口。
- Refresh 只刷新 Preview，不重启 Run。
- Open 使用受保护的项目预览地址，不暴露 Sandbox URL。
- 固定 Version Review URL 与当前 Draft 地址在 UI 中明确区分。

### US-026 停止当前生成或编辑并安全重试 `[Must][P1]`

作为 Builder，我希望停止耗时过长或方向错误的生成/编辑任务，并从最后成功状态重新开始，从而避免持续消耗资源或破坏可工作的版本。

验收标准：

- 只有运行中的 Build/Edit Run 显示“停止”入口，停止操作需要明确确认且具有 `project.write` 权限。
- 取消成功后 Run 进入不可逆的 `cancelled` 终态；已经完成的工具结果和 Conversation 记录继续保留。
- 取消不能改变当前 Promoted Version、Published Release 或 Live URL，未提交 transaction 不得暴露为成功结果。
- UI 在 SSE 重连或页面刷新后仍能恢复真实的 cancelled 状态，不得永久显示“停止中”。
- 用户可以基于最后成功的 `baseVersionId` 重新提交请求；重复 Cancel 或 Retry 不产生重复副作用。

## 8. Epic D：对话编辑与快速 Draft Preview

### US-030 用自然语言修改当前项目 `[Must][P1]`

作为 Builder，我希望通过对话修改文案、颜色、布局或内容结构，从而持续迭代项目而无需直接编辑代码。

验收标准：

- 用户消息先显示为已提交，再根据 Runtime 状态更新执行进度。
- Edit 基于用户当前看到的 `baseVersionId`，不能静默覆盖更新版本。
- stale baseVersion 返回冲突，UI 提示刷新或重新应用修改。
- 成功 Edit 形成新的不可变 ProjectVersion。

### US-031 看到编辑的实时反馈 `[Must][P3]`

作为 Builder，我希望文本和样式修改在右侧快速出现，从而获得接近 v0/Lovable 的迭代节奏。

验收标准：

- 文本/CSS 修改从 source commit 到 iframe Ready 的 P50 不超过 1 秒。
- 结构修改刷新 P95 不超过 5 秒。
- UI 只接受不小于当前 `sourceRevision` 的更新，旧结果不能覆盖新结果。
- HMR 超时时自动 fallback 到完整 iframe reload，但不改变 Promoted Version。

### US-032 在 Draft 编译错误后继续修改 `[Must][P3]`

作为 Builder，我希望 Draft 出现语法或编译错误时仍能继续对话修复，从而不必重建整个项目。

验收标准：

- 错误以 overlay 和 Activity 形式展示，同时保留上一可见画面。
- Dev Session 不因一次语法错误被销毁。
- 修复后相同 Session 可以恢复 Ready。
- Dev Server 崩溃最多自动重启两次，之后提供明确人工重试入口。

### US-033 Sandbox 丢失后恢复编辑会话 `[Must][P3]`

作为 Builder，我希望临时 Sandbox 丢失后系统恢复源码和预览，从而不丢失已提交修改。

验收标准：

- 从当前 source snapshot 恢复新 Sandbox 和 Dev Preview Session。
- UI 显示“重新连接/恢复中”，而不是把项目标记为永久失败。
- 已提交 sourceRevision 不丢失，未提交的 transaction 不暴露为成功状态。
- 恢复后 Production Build 结果与恢复前源码一致。

## 9. Epic E：版本、审阅与可追溯性

### US-040 每次成功 Build 形成不可变版本 `[Must][P1]`

作为 Builder，我希望每次成功验证都形成不可变 ProjectVersion，从而可以安全审阅、发布和回退。

验收标准：

- Version 记录创建时间、来源 Run、source snapshot、ArtifactManifest 和状态。
- 相同 Version 的 Artifact bytes 和 manifest hash 永远不变。
- Build/Gate 未通过不能产生 promoted Version。
- Sandbox 释放后 Version Artifact 仍可读取。

### US-041 查看版本历史 `[Must][P1]`

作为 Builder，我希望查看项目版本历史，从而理解每次修改形成了什么结果。

验收标准：

- 版本列表按时间倒序展示 versionId、状态、创建时间、来源 Run 和是否已发布。
- 当前 Promoted、当前 Published 和历史版本有不同标识。
- 刷新后列表和标识保持一致。
- Retention 即将删除的非保护版本应能被识别，不得删除 current/previous/lastSuccessful。

### US-042 使用固定 Version Review URL `[Must][P1]`

作为 Reviewer，我希望打开固定版本地址进行审阅，从而不会因项目继续编辑而看到变化的内容。

验收标准：

- Version Review URL 固定绑定 `projectId + versionId`。
- 同一 URL 重复访问返回相同 Artifact bytes。
- URL 仅对有项目读取权限的成员开放。
- manifest hash 漂移、路径逃逸或跨项目 version lookup 必须 fail closed。

## 10. Epic F：一键发布与稳定 Live URL

### US-050 发布当前 Promoted Version `[Must][P2]`

作为 Builder，我希望点击一次“发布”即可上线当前可信版本，从而不需要理解 Release、镜像或 generation。

验收标准：

- UI 只提交 `versionId`，不要求用户填写 `releaseId`、ETag 或 Registry 信息。
- 只有属于当前项目且状态为 promoted 的 Version 可以发布。
- 重复点击使用 Idempotency-Key，不产生重复 Packaging 或重复 Publish。
- 发布按钮在无权限、无 promoted Version 或已有冲突操作时不可误提交。

### US-051 查看发布进度并在刷新后恢复 `[Must][P2]`

作为 Builder，我希望看到 Packaging 和 Publication 的真实阶段，从而知道发布是否仍在进行以及失败在哪里。

验收标准：

- 至少展示 Preparing、Building、Pushing、Scanning、Signing、Publishing、Completed/Failed。
- 页面离开不会取消后端 Operation；刷新后从真实状态恢复。
- 失败展示阶段、可重试性和安全的错误摘要。
- UI 不显示 Registry credential、签名私钥或内部 token。

### US-052 首次发布获得稳定 HTTPS Live URL `[Must][P2]`

作为 Builder，我希望首次发布后获得一个可以复制分享的 HTTPS 地址，从而让访客访问正式作品。

验收标准：

- 发布完成后返回唯一 Live URL、发布时间和当前 Release identity。
- Live URL 使用平台域名和有效 TLS。
- Authoring Sandbox 释放后 Live URL 仍返回 200。
- 首次 Publish 从点击到 Live URL 200 的 P95 不超过 120 秒。

### US-053 访客始终访问当前正式 Release `[Must][P2]`

作为 Published Visitor，我希望稳定 URL 始终提供最近一次成功发布的内容，从而不会看到草稿或失败更新。

验收标准：

- Live URL 不读取 Authoring Workspace，不展示 Draft/Promoted Preview。
- 响应能够验证当前 Release identity。
- 静态 hash asset 可长缓存，更新窗口 HTML 使用安全缓存策略。
- 发布基础设施不注入 Runtime、Sandbox 或 Registry credential。

### US-054 识别未发布更改 `[Must][P2]`

作为 Builder，我希望新 Promoted Version 与线上 Release 不一致时看到“有未发布更改”，从而知道需要更新发布。

验收标准：

- 当前 Promoted versionId 与 Published Release source versionId 不一致时显示该状态。
- 状态中明确显示线上仍服务旧 Release。
- Build 失败或 Draft 更新不能触发“已发布”。
- 发布成功后状态自动清除。

### US-055 在原 Live URL 上更新发布 `[Must][P2]`

作为 Builder，我希望发布新版本后继续使用原 Live URL，从而不需要重新分发链接。

验收标准：

- Update 前后 `hostSlug` 和 Live URL 完全不变。
- 新 Release Ready 且内外部 identity probe 成功后才切换流量。
- 从 Validated Release 到外部探测成功的 P95 不超过 45 秒。
- 更新完成后原 URL 返回新内容和新 Release identity。

### US-056 更新失败不影响线上旧版本 `[Must][P2]`

作为 Builder，我希望新版本发布失败时旧网站继续服务，从而避免更新故障造成线上中断。

验收标准：

- Packaging、Ready、EndpointSlice 或 external probe 任一失败时不提交错误 currentRelease。
- Live URL 持续返回上一成功 Release。
- UI 显示 `publish_failed` 和失败阶段，并允许安全重试。
- 不允许 Green/Blue 混流；失败后的 Store current 与真实外部流量一致。

## 11. Epic G：Release 历史、回滚与下线

### US-060 查看发布历史 `[Must][P2]`

作为 Builder，我希望查看 Release 和 Deployment 历史，从而知道何时发布了哪个版本以及结果如何。

验收标准：

- 展示 releaseId、来源 versionId、操作类型、状态、操作者和时间。
- 标识 current、previous、lastSuccessful Release。
- Operation 进行中、失败和已完成状态可在刷新后恢复。
- 历史记录不复制或篡改 Runtime 的状态真相。

### US-061 回滚到历史 Release `[Must][P2]`

作为 Builder，我希望选择历史成功 Release 回滚，从而快速恢复稳定内容。

验收标准：

- 只能选择可用且已验证的历史 Release。
- 确认页显示目标 Release、来源 Version 和当前线上 Release。
- 回滚完成后 Live URL 不变，内容和 Release identity 恢复为目标版本。
- 回滚必须走 Runtime state machine，不允许直接修改 Service selector。

### US-062 下线 Published Work `[Must][P2]`

作为 Builder，我希望主动下线 Live URL，从而停止对外提供内容但保留历史记录。

验收标准：

- 下线需要明确二次确认和 `publish:unpublish` 权限。
- 先移除外部访问，再清理 Service/workload。
- 下线后 Live URL 不再对外提供作品内容。
- hostSlug、Release history 和审计记录继续保留。

### US-063 重新发布并复用原 Live URL `[Must][P2]`

作为 Builder，我希望下线后重新发布仍使用原地址，从而保留此前分发的链接。

验收标准：

- Republish 不生成新的 hostSlug。
- 重新发布只能选择有效的 promoted/validated 目标。
- 完成后原 Live URL 恢复 200 并返回正确 Release identity。
- Unpublish 和 Republish 全过程进入审计记录。

## 12. Epic H：组织管理与平台运营

### US-070 管理项目成员权限 `[Should][P1]`

作为 Organization Admin，我希望给成员分配读取、编辑和发布权限，从而控制谁可以查看或改变项目。

验收标准：

- 至少区分 Reader、Editor、Owner/Admin。
- Reader 不能发起 Run、处理 Permission 或发布。
- Editor 是否具备 Publish 权限必须由显式策略决定，不能仅因可编辑而默认获得。
- 权限变更对新请求立即生效，已签发短期 token 在短 TTL 后失效。

### US-080 按关联 ID 定位失败 `[Must][P2]`

作为 Platform Operator，我希望按 projectId/runId/versionId/releaseId/operationId 查询完整链路，从而快速定位生成或发布失败。

验收标准：

- 查询结果串联请求、Run、Sandbox、Version、Packaging、Release 和 Publication Operation。
- 日志不记录完整 Authorization、principal token、Registry credential。
- UI 或工具展示失败阶段、重试次数、最后错误和当前 owner。
- 证据可导出并包含 commit、image digest、manifest hash 和外部 probe 结果。

### US-081 恢复可重试 Operation `[Must][P2/P4]`

作为 Platform Operator，我希望在 Runtime/Controller 重启后重放可恢复操作，从而避免发布永久卡住。

验收标准：

- 重启后 Operation 从持久状态继续，不能重复提交已完成副作用。
- 相同 generation 只能有一个 owner 提交。
- 超过 5 分钟未终态的 Publication Operation 触发告警。
- 恢复后 Store currentReleaseId、Service selector 和外部 Release identity 一致。

### US-082 安全冻结写入并执行应急回滚 `[Should][P2]`

作为 Platform Operator，我希望在异常时冻结项目写入并通过正常状态机回滚，从而控制事故影响范围。

验收标准：

- 冻结后新的 Build/Edit/Publish 请求被拒绝，读取和线上服务保持可用。
- 应急回滚仍生成正式 Operation 和审计记录。
- 禁止把手工修改 Kubernetes Service selector 作为正常操作入口。
- 解冻前可以校验 DNS、TLS、Ingress、Service、Deployment 与 Store identity。

## 13. Story Map 与 Release Slice

| 用户活动 | Alpha：P0/P1 | Publishable MVP：P2 | v0/Lovable Experience：P3 | Production：P4 |
|---|---|---|---|---|
| 进入产品 | US-001–004 | — | — | 多副本会话与审计增强 |
| 输入与规划 | US-010–013 | — | — | 配额与滥用防护 |
| 生成与预览 | US-020–026 | — | US-021、US-022 的 Draft 加速 | HA Preview Controller |
| 对话编辑 | US-030 | — | US-031–033 | 多副本 session lease |
| 版本审阅 | US-040–042 | — | — | Object Storage/Retention |
| 发布与更新 | — | US-050–056 | 性能优化 | HA Publication Owner |
| 历史与恢复 | — | US-060–063、US-080–082 | — | 多副本确定性恢复 |

### 13.1 Internal Alpha 最小 Story 集

```text
US-001, US-002, US-003, US-004
US-010, US-011, US-012, US-013
US-020, US-021(Promoted 模式), US-022, US-023, US-024, US-026
US-030
US-040, US-041, US-042
```

### 13.2 Publishable MVP 增量 Story 集

```text
US-050, US-051, US-052, US-053, US-054, US-055, US-056
US-060, US-061, US-062, US-063
US-080, US-081
```

### 13.3 v0/Lovable 体验增量 Story 集

```text
US-021(Draft 首屏目标), US-031, US-032, US-033
```

视觉参考、`next-app`、多模态视觉审查、组件/资产、元素定位和版本恢复的增量 Story 见
`2026-07-18-ai-website-docs-generator-p0-user-stories.md`，工程拆分见
`2026-07-19-visual-react-runtime-capability-implementation-plan.md`。

## 14. 非目标与避免误收范围

以下内容不进入当前 User Story Backlog：

- Figma/Canvas 级拖拽编辑器；
- 多人同时编辑同一个项目；
- Lovable 式全栈能力，包括 SSR 动态业务逻辑、Server Actions、Route Handlers、自定义后端、
  用户数据库、认证、运行时 Secret 和第三方业务集成；
- 自定义域名自助接入；
- 模板市场、插件市场和计费；
- 任意 Dockerfile、自定义镜像和浏览器直连 Sandbox；
- 每个历史 Release 的独立公开域名。

如后续加入，应单独完成产品、权限、成本和运维评审，不能作为当前 MVP 的“顺手实现”。

通用 `next-app` React 前端模板属于当前视觉创作范围，但必须使用静态导出和受控前端交互
契约；采用 Next.js 不等于把上述全栈能力带入当前 MVP。

## 15. Enablement Backlog：支撑 Story 的技术项

这些是工程交付项，不是用户故事：

| Enablement | 支撑 Story | Wave |
|---|---|---|
| Create Release / Packaging Query 契约 | US-050、US-051 | P0 |
| 固定 Version Artifact 路由 | US-040、US-042 | P0 |
| Shared publication schemas/client | US-050–063 | P0 |
| Run/Conversation/Preview/Artifact 项目授权 | US-003、US-024 | P0 |
| Product DB、认证与 BFF | US-001–004 | P1 |
| Chat/SSE/Conversation replay | US-004、US-020、US-030 | P1 |
| Promoted Preview workspace | US-021–023 | P1 |
| Release Packaging application service | US-050–052 | P2 |
| Blue/Green、DNS/TLS、Registry、scan/sign | US-052–056 | P2 |
| DevPreviewSession 和 Template dev capabilities | US-021、US-031–033 | P3 |
| WarmPool 与依赖预热 | US-021、US-031 | P3 |
| PostgreSQL/Object Storage adapters | US-004、US-040–063、US-081 | P4 |
| Outbox/lease/multi-replica concurrency | US-051、US-055、US-061、US-081 | P4 |

## 16. Product Backlog Ready 门槛

单条 Story 进入迭代前必须满足：

1. 用户角色、价值和权限边界明确；
2. UI 入口、空状态、Loading、成功、失败和恢复状态齐全；
3. 所需 Shared Contract 已冻结，不允许 Web 手写 Runtime payload；
4. 幂等、并发冲突、刷新恢复和失败不破坏旧状态的语义明确；
5. Browser E2E 验收路径和证据要求明确；
6. 涉及 Preview、Artifact、SSE、Release 的 Story 已定义项目级授权；
7. 涉及性能目标时，使用真实模板、Sandbox 和浏览器测量。

## 17. 产品级 Definition of Done

一个 Release Slice 只有同时满足以下条件才算完成：

- Website 与 Docs 都有真实输入到浏览器可见结果的 E2E；
- 未授权和跨项目访问测试通过；
- 刷新、SSE 重连和 Runtime/Controller 重启不会丢失已提交状态；
- 失败 Build 不替换上一 Promoted Version；
- 失败 Publish/Update 不影响当前 Live Release；
- Version URL 固定、Live URL 稳定，两者语义不混用；
- UI、Runtime Store、Kubernetes workload 和外部 URL 的 identity 可对齐；
- 验收证据包含 commit、dirty state、image digest、SSE、manifest hash、截图和 Live URL 探测。
