---
date: 2026-07-04
topic: internal-ai-website-docs-generator
---

# Internal AI Website / Docs Generator MVP

## Summary

本文档定义一个面向公司内部设计师的安全 AI Website / Docs 生成平台 MVP。用户通过 prompt、Markdown 或附件提供内容源，系统先整理成可确认的作品 Brief；确认后，每个作品进入独立 agent sandbox，生成可预览、可对话修改、可导出的 Website 或 Docs。

---

## Problem Frame

内部设计师今天把内容变成 Website 或 Docs 时，常见路径有两种：一种是传统 Figma 流程，先理解和重组内容，再做视觉稿，最后还要进入实现交付；另一种是 vibe coding + design.md 流程，直接让 AI 基于内容和设计规范生成站点。前者视觉可控但慢，且设计与实现之间存在断层；后者速度快，但生成质量、工具调用稳定性、项目初始化和错误修复体验都不够可控。

由于场景发生在公司内部，内容源可能包含未公开的产品资料、设计说明、业务文档或品牌资产。外部通用生成平台无法满足数据安全、内部权限、过程审计和环境隔离要求。设计师需要的是一个把现有 AI 生成工作流产品化的内部平台：保留 prompt/design.md 的灵活性，同时让内容整理、作品确认、生成、预览、修改和导出变成稳定流程。

---

## Actors

- A1. 设计师：创建 Website 或 Docs 作品，提供内容源和设计方向，确认 Brief，并通过对话持续修改作品。
- A2. LLM 内容整理代理：读取 prompt、Markdown 和附件，将散乱内容整理成适合 Website 或 Docs 的作品 Brief。
- A3. 作品生成代理：在独立 agent sandbox 中初始化项目、生成作品、构建预览、修复错误并响应后续修改。
- A4. 内部平台管理员：维护可用技术模板、设计上下文、安全策略和运行环境能力。

---

## Key Flows

- F1. 创建并确认作品 Brief
  - **Trigger:** 设计师创建新作品并提供 prompt、Markdown 或附件作为内容源。
  - **Actors:** A1, A2
  - **Steps:** 系统接收内容源；LLM 归纳目标、受众、内容结构和视觉方向；系统生成 Website 或 Docs Brief；设计师查看、修改或继续对话调整；设计师确认 Brief。
  - **Outcome:** 平台获得一份设计师认可的生成依据，后续生成不再直接依赖散乱输入。
  - **Covered by:** R1, R2, R3, R4, R5

- F2. 生成独立作品
  - **Trigger:** 设计师确认 Brief 并选择作品类型和技术模板。
  - **Actors:** A1, A3
  - **Steps:** 系统为该作品创建独立 agent sandbox；生成代理基于 Brief、设计上下文和技术模板创建作品；系统构建并提供预览；生成代理自动处理可恢复错误；设计师获得可查看的初版作品。
  - **Outcome:** 一个可预览的 Website 或 Docs 初稿在内部隔离环境中完成生成。
  - **Covered by:** R6, R7, R8, R9, R10, R11

- F3. 对话式修改作品
  - **Trigger:** 设计师在预览后提出修改要求。
  - **Actors:** A1, A3
  - **Steps:** 设计师在作品详情页左侧通过 LLM chat 描述修改；生成代理基于当前作品上下文调整内容、结构或视觉表达；系统重新构建预览；右侧预览区域刷新展示最新结果；设计师继续迭代或导出。
  - **Outcome:** 作品可以围绕同一个上下文持续演进，而不是每次重新生成。
  - **Covered by:** R12, R13, R14, R15, R19, R20, R21, R22

```text
Prompt / Markdown / Attachments
  -> LLM Content Brief
  -> Designer Review + Revision
  -> Confirm Brief
  -> Choose Website or Docs
  -> Choose Technical Template
  -> Dedicated Agent Sandbox
  -> Detail Workspace: Chat + Preview
  -> Conversational Edits
  -> Export / Handoff
```

---

## Requirements

**内容输入与 Brief**
- R1. 平台必须支持设计师使用 prompt、Markdown 文档和附件作为作品内容源。
- R2. 平台必须先将内容源整理成结构化作品 Brief，而不是直接进入代码或站点生成。
- R3. Brief 必须表达作品类型、目标受众、核心信息、内容结构、视觉方向、推荐技术模板和潜在缺失信息。
- R4. 设计师必须能够在生成作品前查看并确认 Brief。
- R5. 设计师必须能够通过 LLM 对话继续修改 Brief，直到认为生成方向可接受。

**作品类型与技术模板**
- R6. MVP 必须同时支持 Website 和 Docs 两类作品，但每次生成必须有明确目标类型。
- R7. 平台必须在产品模型中支持 Next.js、Astro、Fumadocs 和 Docusaurus 四类技术模板，但 MVP 开发节奏按模板分阶段启用：首个 Website 闭环优先 `astro-website`，首个 Docs 闭环优先 `fumadocs-docs`，Next.js 与 Docusaurus 先作为可配置模板接口和后续扩展目标保留。
- R8. 技术模板在产品体验中应被解释为适合不同作品目标的模板选择，而不是要求设计师理解工程细节。
- R9. Website 生成结果应优先满足叙事、视觉表达、信息层级和转化路径。
- R10. Docs 生成结果应优先满足导航结构、阅读体验、章节完整度和信息查找效率。

**Agent Sandbox 与生成体验**
- R11. 每个作品必须拥有独立 agent sandbox，以隔离内容、生成过程、构建过程和后续修改上下文。
- R12. 生成过程必须呈现自然顺畅的进度状态，让设计师理解当前处于内容整理、项目生成、构建预览、修复错误或完成阶段。
- R13. 作品生成代理必须能够在 sandbox 内完成必要工具调用，并以稳定、参数正确、可恢复的方式执行生成任务。稳定的验收标准为：单次工具调用失败后系统能自动重分类（recoverable 或 terminal）并给出可操作摘要；同一错误最多触发 3 次自动修复尝试；超出修复上限后 run 进入 `partial` 或 `blocked` 状态，不无限循环。
- R14. 生成失败或构建错误时，系统应优先由生成代理自动诊断和修复，避免把工程错误直接暴露给设计师处理。自动修复失败后，设计师应看到可操作的中文摘要（如"依赖版本冲突，已尝试修复但未成功，可重试或修改 Brief"），不应看到原始构建日志或堆栈。
- R15. 生成完成后，设计师必须能够围绕同一个作品进行对话式修改，并在修改后重新预览。

**设计上下文与安全**
- R16. 平台应支持 design.md 作为可选设计上下文，用于约束品牌气质、组件偏好、排版规则、语气和风格边界。
- R17. MVP 必须保证 prompt、附件、Markdown、design.md、生成代码和预览内容都在公司内部受控环境中处理。
- R18. 平台必须为后续权限控制、审计和运行环境治理留下产品边界，但 MVP 不要求完成面向公网 SaaS 的租户化商业能力。

**作品详情工作台**
- R19. 作品生成后必须进入作品详情页，采用左侧 LLM chat message 信息区、右侧预览区的双栏工作台布局。
- R20. 左侧 chat 区必须同时承载用户修改消息、LLM 回复、生成进度、工具调用摘要和需要用户确认的信息。
- R21. 右侧预览区在生成、修复或对话式修改期间必须保留上一个已通过 gate 的正式预览，并显示重建中状态；只有在新版本通过 Build/Review/Safety gate 并收到 `preview.updated` 事件后，才切换到新版本。系统不应在重建期间清空右侧预览或展示候选（candidate）预览。
- R22. 左侧消息与右侧预览必须围绕同一个作品版本和 `GenerationRun` 对齐，避免用户看到的对话状态与预览结果不一致。具体要求：`run.completed` 事件不得早于对应的 `preview.updated` 事件到达前端（成功路径）；若 run 以 `partial` 或 `blocked` 结束，右侧保留最近一次 promoted 版本，左侧显示可操作的失败摘要。

---

## Acceptance Examples

- AE1. **Covers R1, R2, R3, R4, R5.** Given 设计师上传一份 Markdown 产品说明并补充视觉 prompt, when 创建作品, then 系统先生成包含内容结构、目标受众、视觉方向和推荐模板的 Brief，并允许设计师继续要求 LLM 修改后再确认。
- AE2. **Covers R6, R7, R8, R9, R10.** Given 同一份 Markdown 内容, when 设计师选择 Website, then 系统应生成面向叙事和视觉表达的站点 Brief；when 设计师选择 Docs, then 系统应生成面向导航、章节和阅读体验的文档站 Brief。
- AE3. **Covers R11, R12, R13, R14.** Given 设计师确认 Brief 并开始生成, when 构建过程中出现可恢复错误, then 该作品的独立 agent sandbox 应尝试自动修复并继续生成，同时向设计师展示非工程化的进度状态。
- AE4. **Covers R15, R16.** Given 作品初版已生成并使用 design.md 作为风格约束, when 设计师通过对话要求“让首页更适合高端 B2B 产品发布”, then 生成代理应基于当前作品和设计上下文修改并重新提供预览。
- AE5. **Covers R17, R18.** Given 附件包含内部未公开资料, when 系统整理 Brief、生成作品和构建预览, then 资料、生成代码和预览内容都不应离开公司内部受控环境。
- AE6. **Covers R19, R20, R21, R22.** Given 作品生成完成, when 设计师打开作品详情页, then 左侧应显示该作品的 LLM chat、生成事件和修改入口，右侧应显示当前预览；when 用户继续对话修改, then 左侧追加修改过程，右侧在新版本可用后刷新预览。
- AE7. **Covers R13, R14.** Given 作品在生成过程中出现构建失败且自动修复超过 3 次仍未成功, when 修复上限耗尽, then 系统应将 run 标记为 `partial` 或 `blocked`，左侧显示中文可操作摘要（如"构建失败，已尝试自动修复，建议重试或修改 Brief"），右侧保留最近一次成功的 promoted 预览，不展示原始构建日志。

---

## Success Criteria

- 设计师可以从 prompt、Markdown 或附件出发，在不写代码的情况下生成一个可预览的 Website 或 Docs 初版。
- 设计师在生成前能确认 AI 对内容、结构和设计方向的理解，减少直接生成导致的失控感。
- 每个作品都有独立上下文和 sandbox，后续对话修改能基于当前作品持续迭代。
- 作品详情页让设计师在同一屏内完成“看预览”和“对话修改”，不需要在聊天页、日志页和预览页之间来回跳转。
- 生成过程中的常见工具调用、构建和修复过程对设计师表现为稳定、顺畅、可恢复。
- 内部敏感内容不会因使用外部生成平台、外部运行环境或不受控预览链路而泄露。
- 后续 `ce-plan` 可以基于本文档规划产品模块、运行时边界和 MVP 交付顺序，而不需要重新发明用户流程、范围边界或成功标准。

---

## Scope Boundaries

### Deferred for later

- Figma MCP 作为视觉上下文来源。
- Figma 到代码或 Figma 到站点的高保真还原。
- 更完整的 design.md 管理、版本化、团队共享和品牌资产库能力。
- 多人协同编辑、评论、审批和发布流。
- 更丰富的导出、部署、版本回滚和站点生命周期管理。
- 细粒度权限、审计报表和管理员治理台的完整产品化。

### Outside this product's identity

- 面向公网用户开放注册的通用 SaaS 建站平台。
- 与外部 Lovable、v0、Cursor 等产品做商业化竞品对抗。
- 替代 Figma 的完整视觉编辑器。
- 通用 Web App Builder、后台系统生成器或数据库应用生成器。
- 完整 CMS 或内容运营平台。

---

## Key Decisions

- Brief-first 而不是直接生成：先让 AI 证明它理解了内容、结构和设计意图，再进入生成，可以降低设计师的失控感和返工成本。
- Website 与 Docs 同入口但不同目标：两者共享内容整理和生成底座，但信息架构、质量标准和技术模板选择必须按作品类型区分。
- 每个作品独立 agent sandbox：作品级隔离能保护内部资料，也让后续对话修改拥有稳定上下文。
- design.md 在 MVP 中作为可选输入：它是长期重要资产，但第一版不应强制设计师先完成设计规范配置才能生成作品。
- 技术栈以模板形式呈现：Next.js、Astro、Fumadocs、Docusaurus 是生成约束和输出形态，不应成为设计师的主要认知负担；实际开发先把 `astro-website` 和 `fumadocs-docs` 做成可验收闭环，再扩展其它模板。

---

## Dependencies / Assumptions

- 公司内部有可用于处理敏感内容的 LLM 调用路径或模型网关。
- 公司内部允许为每个作品创建隔离的 agent sandbox，并运行生成、构建、预览和修复流程。**若此假设不成立，MVP 应降级为：Brief Agent 在控制面运行（无 sandbox 依赖），生成阶段以受控后端进程替代 sandbox 执行，预览通过内部静态文件服务提供；完整 sandbox 隔离能力延后到基础设施就绪后再接入。**
- MVP 的首要验证对象是内部设计师，而不是市场营销、普通办公人群或外部客户。
- Markdown/prompt/附件优先足以验证核心价值，Figma MCP 可以在后续阶段补充。
- 技术模板的具体工程实现、运行时编排、安全策略和错误恢复机制以 `docs/product/2026-07-04-anydesign-mvp/2026-07-04-mvp-implementation-plan.md` 为开工计划。

---

## Planning Resolutions

### Resolved for MVP Planning

- [Resolved R7][Technical] 四个模板在产品模型中保留，但开发按阶段启用：MVP 首个 Website 闭环为 `astro-website`，首个 Docs 闭环为 `fumadocs-docs`。
- [Resolved R11, R13, R14][Technical] agent sandbox 生命周期、资源上限、日志保留和失败恢复策略进入 implementation plan 的 U4 / U5 / U8；MVP 先实现 claim、ready、busy、idle、recoverable failed，不做完整治理台。
- [Resolved R17, R18][Needs research] 内部模型网关、附件存储、预览访问和审计链路作为开工前置依赖写入 implementation plan；M0 可以用接口 mock，M1 需要真实集成或明确替身。
- [Resolved R16][Product/Technical] design.md 在 MVP 中只作为可选 Markdown 输入，不强制创建、校验或团队共享；最小格式由 Brief Agent 读取并总结，不做独立管理产品。
