---
date: 2026-07-10
status: implemented-and-validated
type: implementation-plan
topic: design-profile-fidelity
related:
  - ./2026-07-09-design-profile-spec.md
  - ./2026-07-08-project-lifecycle-generation-edit-build-plan.md
  - ./2026-07-08-runtime-api-freeze.md
  - ./2026-07-04-agent-harness-design.md
---

# DesignProfile 保真修复落地方案

## 1. 文档结论

当前 DesignProfile 已经可以完成创建、绑定、版本记录、冲突检查和 Runtime 注入，但它还不能稳定复现原始 `DESIGN.md` 的生成效果。

问题不在 DesignProfile API 的 JSON 持久化过程。Runtime 会保存传入的 `product`、`brand`、`visual`、`tokens`、`components`、`content` 等完整 JSON 字段。实际损失发生在三个位置：

1. 原始 `DESIGN.md` 被人工整理成 preset 时，只保留了摘要后的结构化信息，无法从 Profile 反向恢复原始文档。
2. Runtime 生成的 `inputs/design.md` 使用固定 8,000 字符上限，并在组件 JSON 中间直接截断。
3. `runtimeTokenMapping` 只能自动应用 12 个基础 token，无法表达展示字体、字号体系、渐变、完整间距、组件级圆角和多级 elevation。

因此，本方案的核心决策是：

```text
原始设计来源负责可追溯和无损保留
Canonical DesignProfile 负责结构化设计语义
Runtime Design Capsule 负责高优先级模型上下文
Style Contract 负责可验证的代码级 token 应用
```

四层必须独立存储、独立校验，不再让一份 8,000 字符的 Markdown 同时承担存档、结构化契约和模型上下文三种职责。

## 2. 已验证的现状

### 2.1 数据规模

| 输入 | 原始文档 | Profile JSON | Runtime `design.md` |
|---|---:|---:|---:|
| AuthKit | 27,191 字符 | 13,424 字符 | 最多 8,000 字符 |
| ElevenLabs | 21,753 字符 | 13,286 字符 | 最多 8,000 字符 |

旧的 `design_md` 流程会把原始文档原样写入 `inputs/design.md`。已有 E2E evidence 证明 AuthKit Build Agent 实际读取了完整的 27,191 字节。

新的 DesignProfile 流程会同时写入：

- `inputs/design-profile.json`：完整的结构化 Profile；
- `inputs/design.md`：从 Profile 渲染的模型摘要；
- `state/context.md`：本次 run 采用的 Profile ID、版本、hash 和 token policy。

结构化 JSON 没有在写入 workspace 时丢字段，但 `inputs/design.md` 会被截断，而且导入前的原始 Markdown 没有随 Profile 一起保存。

### 2.2 已确认的信息损失

#### AuthKit

原始文档中的以下高辨识度信息没有被完整结构化：

- Untitled Sans、aeonikPro/Space Grotesk、dotDigital/JetBrains Mono 三种字体角色；
- 12px 到 48px 的完整 type scale、line-height 和 letter-spacing；
- Skywash 标题渐变、fading hairline 和 conic spotlight 三类渐变配方；
- Steel Plate、Moon Mist、Ice Highlight、Gridline Blue 等完整颜色角色；
- 输入框、badge、icon container、modal 等组件级圆角；
- 多组 inset frost 和 outer halo 阴影；
- background grid、theme toggle、provider button、customization swatch 的精确规格。

#### ElevenLabs

原始文档中的以下高辨识度信息没有被完整结构化：

- Waldenburg 300 作为 display font，以及 Inter/Geist Mono 的角色划分；
- display 标题 `-0.02em` tracking 和各级字号的精确 line-height；
- Graphite、Ash、button border 等辅助颜色；
- 96-125px section rhythm、64px outer gutter、32px card padding；
- 20px/24px card radius 和 4px input radius 的组件差异；
- audio sphere 的尺寸、径向渐变、play overlay 和使用频次；
- 50px top nav、6-column trust logo grid、hairline divider 等精确布局规则。

### 2.3 Runtime 摘要截断

当前 renderer 的输出顺序是：

```text
Product
Brand
Visual Direction
Runtime Token Mapping
Components
Content
Accessibility
Technical
Governance
```

AuthKit 和 ElevenLabs 的完整渲染结果都约为 11 KB。固定 8,000 字符上限会在 `Components.featureGrid` 中间截断，导致 `content`、`accessibility`、`technical` 和 `governance` 不进入摘要。

此外，renderer 没有渲染 `tokens`，只渲染了 `runtimeTokenMapping`。即使未触发 8,000 字符上限，完整 typography、spacing、radius 和 shadow 也不会出现在 `design.md` 中。

### 2.4 Runtime token 能力边界

当前 Style Contract 只支持以下 12 个 token：

```text
color.background
color.surface
color.surfaceStrong
color.text
color.muted
color.primary
color.primaryContrast
color.border
radius.card
radius.control
font.sans
shadow.soft
```

这些 token 足以表达基础主题，不足以表达完整设计语言。结果通常能够匹配“暗色玻璃 + 紫色”或“暖白编辑风 + 黑色按钮”，但会失去真正决定品牌辨识度的 typography、gradient、composition 和 component recipe。

### 2.5 Shared schema 与 preset 字段漂移

当前产品规格中的 `ComponentGuideline` 使用 `role`、`anatomy`、`variants`、`usage` 和 `avoid`。AuthKit、ElevenLabs 以及 Runtime Harness preset 也都使用 `role`。

但 shared `ComponentGuidelineSchema` 当前要求 `intent`、`usage` 和 `avoid`。Runtime API 因为把 components 保存为宽松 JSON，可以成功创建这些 Profile；BFF 或客户端使用 shared schema 严格解析响应时会失败。

这不是视觉内容丢失的直接原因，但它会导致同一 Profile 在 Runtime 和产品侧出现不同的有效性判断。V2 必须统一使用 `role`，并在一个兼容周期内接受旧的 `intent`，解析后规范化为 `role`。

## 3. 修复目标

### 3.1 产品目标

- 用户导入一份 `DESIGN.md` 后，系统能够长期保存并回溯原始来源。
- 同一个 DesignProfile 在 Website、Docs、Build、Edit、Review 中保持可解释的一致性。
- Profile 可以表达品牌签名规则，而不仅是通用颜色和圆角。
- 生成结果发生偏离时，可以判断是 source import、profile conversion、agent execution 还是 template constraint 导致。

### 3.2 工程目标

- 原始上传 bytes 可恢复，服务端和客户端 hash 可交叉验证。
- Runtime 不再在 JSON 或 Markdown section 中间截断内容。
- 所有 `required` signature rules 必须进入模型上下文。
- 现有 12-token API 保持向后兼容。
- 新增扩展 token 后，模板可以自动应用高辨识度样式。
- 生成前后都有可自动执行的 fidelity gate。

### 3.3 非目标

- 不在本阶段实现完整设计系统管理 UI。
- 不实现 Figma import/export。
- 不要求不同模型调用产生逐像素一致的页面。
- 不把 Website 风格参考强行视为 Docs 的像素级设计稿。
- 不允许 Runtime 读取用户机器上的绝对路径作为长期 source of truth。

## 4. 目标架构

```text
Imported DESIGN.md
  -> immutable DesignSourceArtifact
       original bytes + sha256 + metadata
  -> DesignProfile Converter
       canonical structured profile + loss report
  -> Runtime Design Capsule
       prioritized, deterministic, bounded model context
  -> Workspace Inputs
       design-profile.json
       design.md
       design-source.md
  -> Style Contract
       base tokens + optional extended tokens
  -> Build / Review
       signature-rule checks + visible artifact checks
```

### 4.1 四层职责

| 层 | Source of truth | 主要消费者 | 是否允许压缩 |
|---|---|---|---|
| Source Artifact | 原始 Markdown/附件 | 审计、重新转换、Build/Review | 否 |
| Canonical Profile | 结构化 JSON | Runtime、BFF、管理 UI、Review | 否 |
| Design Capsule | 高优先级 Markdown | 模型上下文 | 是，但必须按 section 压缩 |
| Style Contract | 可编辑 CSS token map | 模板、style tools | 不适用 |

## 5. 数据契约修改

### 5.1 区分 schema version、Profile revision 和 draft

`version` 继续表示同一个 Profile 的业务 revision。新增独立的 schema version：

```ts
type DesignProfileSchemaVersion = "design-profile@1" | "design-profile@2";

type DeepPartial<T> = {
  [K in keyof T]?: T[K] extends Array<infer U>
    ? Array<U>
    : T[K] extends object
      ? DeepPartial<T[K]>
      : T[K];
};
```

导入结果不能直接伪装成满足 Runtime 严格契约的 `DesignProfile`。新增 draft record：

```ts
type DesignProfileDraft = {
  id: string;
  schemaVersion: "design-profile@2";
  version: number;
  name: string;
  status: "draft";
  scope: DesignProfileScope;
  source: DesignProfileSource;
  candidate: DeepPartial<DesignProfileV2Payload>;
  conversionReportId: string;
  validationIssues: Array<{
    path: string;
    code: string;
    message: string;
    blocking: boolean;
  }>;
  createdAt: string;
  updatedAt: string;
};

type ActiveDesignProfile = {
  id: string;
  schemaVersion: "design-profile@1" | "design-profile@2";
  version: number;
  status: "active" | "archived";
  // Existing strict DesignProfile fields.
};

type DesignProfileRecord = DesignProfileDraft | ActiveDesignProfile;
```

`DesignProfileV2Payload` 表示现有 strict Profile payload 加上 `signatureRules`、`overrides` 和 `extendedTokenMapping`；它不包含 id、status、revision timestamps 等 record metadata。

状态规则：

- deterministic import 只创建 `DesignProfileDraft`，允许字段不完整；
- draft 不能绑定 project，也不能解析为 run context；
- draft update 只更新 `candidate` 并追加 revision；
- activation 把 candidate 转成 strict Profile，运行完整 schema、source integrity、template 和 token 校验；
- activation 成功后创建新的 active revision，不原地修改 draft；
- current Runtime 的 `validate_for_runtime()` 只用于 active/archived strict payload；
- `design-profile@1` 缺少 `schemaVersion` 时只在读取兼容层归一化为 V1，不修改历史 bytes。

### 5.2 新增 DesignSourceArtifact

MVP 使用 Runtime 本地持久化目录保存 immutable source blob，不把完整 Markdown 内嵌在 Profile list response 中。

```ts
type DesignSourceArtifact = {
  id: string;
  scope: DesignProfileScope;
  fileName: string;
  mediaType: "text/markdown" | "text/plain";
  contentEncoding: "identity";
  sizeBytes: number;
  sha256: string;
  createdAt: string;
};
```

存储约束：

- 单个 source 最大 256 KiB；
- Runtime 根据实际 bytes 计算 `sha256`，不信任客户端传入值；
- 客户端可以提供 `clientSha256`，不匹配时创建失败；
- artifact 创建后不可覆盖，只能创建新 artifact；
- 内部 `storageRef` 只能由 Runtime 生成，不能接受或暴露用户路径；
- artifact 必须带 project/workspace/organization scope，复用 Profile 可见性规则；
- list/get Profile 默认只返回 metadata，不返回 source body。
- `fileName` 只能作为展示 metadata，不能参与服务端路径拼接；
- source API 只允许受信任 BFF/service token 调用，不作为 browser-public API 暴露。

RuntimeStore 落盘建议：

```text
{RUNTIME_STORAGE_DIR}/design-source-artifacts.jsonl
{RUNTIME_STORAGE_DIR}/design-source-artifacts/{artifactId}/source.md
```

创建顺序：先把 source bytes 写入同目录临时文件，完成 hash、`sync_all` 和 UTF-8 校验后原子 rename，再 append metadata 并同步 metadata 文件。启动恢复时，metadata 存在但 blob 缺失应标记为 `missing`；孤立 blob 不自动绑定 Profile，只记录 recovery warning。

### 5.3 扩展 DesignProfile.source

```ts
type DesignProfileSource = {
  kind: "manual" | "brief" | "imported" | "generated";
  sourceIds?: string[];
  sourceArtifactIds?: string[];
  primarySourceArtifactId?: string;
  sourceHash?: string;
  converterVersion?: string;
  importedAt?: string;
  integrity: "verified" | "unverified" | "missing";
  notes?: string;
};
```

规则：

- `kind = imported` 时，新建 Profile 必须提供 `primarySourceArtifactId`；
- `sourceHash` 必须等于 primary artifact 的 `sha256`；
- 旧 Profile 没有 artifact 时标记为 `unverified`，不得伪造 hash；
- version diff 应显示 source artifact 和 converter version 的变化。

### 5.4 新增 conversionReport

每次 imported Profile 转换必须产生机器可读报告，禁止只返回“转换成功”。

```ts
type DesignProfileConversionReport = {
  id: string;
  designProfileId: string;
  profileVersion: number;
  converterVersion: string;
  deterministicParserVersion: string;
  semanticEnrichment?: {
    modelProvider: string;
    model: string;
    promptHash: string;
    parametersHash: string;
  };
  sourceArtifactId: string;
  sourceHash: string;
  extractedSections: string[];
  extractedTokenCount: number;
  extractedComponentCount: number;
  requiredSignatureRuleCount: number;
  unmappedItems: Array<{
    sourceSection: string;
    startByte: number;
    endByte: number;
    excerpt: string;
    excerptHash: string;
    reason: "unsupported-field" | "ambiguous" | "duplicate" | "invalid-value";
  }>;
  warnings: string[];
  createdAt: string;
};
```

转换流程必须分两步：

1. 确定性解析：提取 Markdown headings、token tables、CSS custom properties、Tailwind theme、明确的 Do/Don't 列表和组件标题。
2. 语义整理：模型只负责把 prose 归类到 visual、components、content 和 signature rules，不得覆盖确定性解析得到的精确值。

合并规则：

- CSS/token table 的精确值优先于模型概括；
- 同一个 token 出现不同值时写入 `unmappedItems` 并要求人工确认；
- 所有未映射的 source item 必须出现在 report 中；
- `excerpt` 最大 500 字符，完整原文通过 byte range 回到 source artifact；
- conversion report 与 Profile version 一起保存；
- imported Profile 激活前必须人工确认 required signature rules 和所有 ambiguity warning。

### 5.5 新增 signatureRules

`signatureRules` 保存少量但决定品牌辨识度的原子规则。它们必须始终进入 Design Capsule，并参与自动 Review。

```ts
type DesignSignatureRule = {
  id: string;
  category:
    | "color"
    | "typography"
    | "spacing"
    | "component"
    | "composition"
    | "imagery"
    | "content";
  statement: string;
  priority: "required" | "preferred";
  appliesTo: "all" | Array<"website" | "docs">;
  verification:
    | { kind: "token"; token: string; expected: string; comparator: ValueComparator }
    | {
        kind: "computed-style";
        route: string;
        selector: string;
        property: string;
        expected: string;
        comparator: ValueComparator;
        minMatches?: number;
        excludeWithin?: string;
        matchPolicy?: "all" | "any";
        referenceProperty?: string;
      }
    | { kind: "dom"; route: string; selector: string; minMatches: number }
    | { kind: "source-pattern"; paths: string[]; pattern: string }
    | { kind: "visual-review"; rubric: string };
};

type ValueComparator =
  | { kind: "exact" }
  | { kind: "contains" }
  | { kind: "color-equivalent" }
  | { kind: "numeric-tolerance"; tolerance: number; unit?: string }
  | { kind: "numeric-ratio"; ratio: number; tolerance: number }
  | { kind: "forbidden-anywhere" };
```

规则：

- `appliesTo` 数组至少包含一项且不得重复；
- 单个 Profile 最多 24 条 required rules、64 条总 rules；
- `statement` 最大 240 字符，visual-review rubric 最大 500 字符；
- `source-pattern` 只能作为实现证据，不能单独满足 required visual rule；
- selector 未匹配到 `minMatches` 时 assertion 失败；
- computed style 在比较前必须规范化颜色、引号、字体列表、数字和 CSS unit；浏览器返回的 `rgb()` 必须与等价 hex 正确比较；
- `forbidden-anywhere` 必须检查所有匹配元素，可用 `excludeWithin` 排除明确允许的语义容器；
- `numeric-ratio` 用 `property / referenceProperty` 验证 `em` 等相对值，不能把 `0.10em` 固定换算成某个字号下的 px；
- `matchPolicy` 默认 `all`；只有规则明确允许候选元素任一满足时才使用 `any`。

示例：

```json
{
  "id": "authkit-display-font",
  "category": "typography",
  "statement": "Display headings use aeonikPro or Space Grotesk at weight 500.",
  "priority": "required",
  "appliesTo": ["website"],
  "verification": {
    "kind": "computed-style",
    "route": "/",
    "selector": "h1",
    "property": "font-family",
    "expected": "Space Grotesk",
    "comparator": { "kind": "contains" },
    "minMatches": 1
  }
}
```

### 5.6 扩展 tokens

保留现有字段，并增加明确的高保真结构：

```ts
type DesignProfileTokensV2 = {
  color: Record<string, string>;
  typography: {
    families: {
      body: string;
      display?: string;
      mono?: string;
      eyebrow?: string;
    };
    scale: Record<string, {
      fontSize: string;
      lineHeight: string;
      fontWeight: string;
      letterSpacing?: string;
    }>;
  };
  gradients?: Record<string, string>;
  radius: {
    scale?: Record<string, string>;
    byComponent?: Record<string, string>;
  };
  elevation: Record<string, string>;
  spacing: {
    baseUnit?: string;
    scale?: Record<string, string>;
    layout?: Record<string, string>;
  };
};
```

现有 `tokens.typography.fontSans` 等字段继续接受一个兼容周期。读取端优先使用 V2，缺失时 fallback 到旧字段。

### 5.7 定义 Website/Docs effective Profile

全局 tokens 和 components 不能只依赖 `signatureRules.appliesTo`。V2 增加 surface/template override：

```ts
type DesignProfileSurfaceOverride = {
  tokens?: DeepPartial<DesignProfileTokensV2>;
  extendedTokenMapping?: ExtendedRuntimeTokenMapping;
  components?: DeepPartial<ComponentRules>;
  content?: Record<string, unknown>;
  signatureRules?: DesignSignatureRule[];
};

type DesignProfileOverrides = {
  surfaces?: {
    website?: DesignProfileSurfaceOverride;
    docs?: DesignProfileSurfaceOverride;
  };
  templates?: Partial<Record<
    "astro-website" | "fumadocs-docs" | "nextjs-website" | "docusaurus-docs",
    DesignProfileSurfaceOverride
  >>;
};
```

effective Profile 解析顺序：

```text
base profile
  -> surface override (website/docs)
  -> template override
  -> explicit run override accepted by conflict flow
```

合并规则：

- object 按 key deep merge；
- token map 后层覆盖前层；
- signature rules 按 `id` 合并，同 ID 后层替换；
- component recipes 按 primitive/pattern key 合并；
- 普通数组默认整体替换，不做隐式 concat；
- required rule 不能通过 `null` 或空数组删除，只能显式降级并触发 conflict flow；
- run snapshot 记录 base profile hash、surface/template override hash 和 effective profile hash。

### 5.8 扩展 Runtime token mapping

现有 `runtimeTokenMapping` 保持必填和不变，新增可选 `extendedTokenMapping`：

```ts
type ExtendedRuntimeTokenMapping = Partial<{
  "color.action": string;
  "color.actionContrast": string;
  "color.authSubmit": string;
  "font.display": string;
  "font.mono": string;
  "type.display.size": string;
  "type.display.lineHeight": string;
  "type.display.letterSpacing": string;
  "type.body.letterSpacing": string;
  "spacing.pageGutter": string;
  "spacing.section": string;
  "spacing.cardPadding": string;
  "spacing.gridCell": string;
  "radius.input": string;
  "radius.badge": string;
  "radius.largeCard": string;
  "gradient.display": string;
  "gradient.ambient": string;
  "shadow.cardStrong": string;
}>;
```

约束：

- `color.primary` 表示 Profile 主色，不得默认等同于所有按钮填充；模板通用 action 使用 `color.action`，认证提交使用 `color.authSubmit`；
- 只有模板 Style Contract 声明支持的 token 才能自动应用；
- 不支持的 extended token 记录为 `skipped`，不能静默忽略；
- 所有值继续使用现有 CSS value validator；
- Edit run 默认不自动重置现有 token。

## 6. Runtime API 修改

### 6.1 创建 source artifact

```http
POST /design-source-artifacts
Content-Type: application/json
X-AnyDesign-Internal: true
X-Runtime-Admin-Token: <RUNTIME_INTERNAL_ADMIN_TOKEN>

{
  "scope": { "projectId": "authkit-style-reference" },
  "fileName": "DESIGN.md",
  "mediaType": "text/markdown",
  "contentBase64": "IyBTdHlsZSBSZWZlcmVuY2UuLi4=",
  "clientSha256": "..."
}
```

`contentBase64` 按原始上传 bytes 编码，避免 JSON 字符串解码时丢失 BOM、换行或编码信息。Runtime 先对 bytes 计算 hash，再验证内容是合法 UTF-8 Markdown/Text；若产品只需要规范化文本，应另行定义 `normalizedTextHash`，不能与原始 bytes hash 混用。

Source API 复用现有 `internal_admin_authorized` 语义。decoded source 限制为 256 KiB，JSON request body 限制为 384 KiB；`contentBase64`、解码内容和 token 必须从 access log、audit log、error body 和 tracing field 中脱敏。

响应：

```json
{
  "artifact": {
    "id": "design-source-123",
    "scope": { "projectId": "authkit-style-reference" },
    "fileName": "DESIGN.md",
    "mediaType": "text/markdown",
    "contentEncoding": "identity",
    "sizeBytes": 27191,
    "sha256": "...",
    "createdAt": "..."
  }
}
```

### 6.2 读取 source artifact

```http
GET /design-source-artifacts/{artifactId}
GET /design-source-artifacts/{artifactId}/content
```

- 两个 endpoint 都要求 BFF/service authorization；
- 不允许浏览器直接使用 artifact ID 拉取 source；
- metadata endpoint 返回 JSON；
- content endpoint 返回原始 media type；
- 返回内容前重新校验 hash；
- hash 不匹配时返回 500 并记录 integrity failure。
- D1 在 Profile 创建、绑定和 materialize source 时要求 artifact scope 与 Profile scope 完全一致；project artifact 不能被其他 project 引用。
- workspace/organization 到 project 的跨 scope 复用需要可信 BFF 提供可验证 scope lineage；D1 没有 lineage 数据时不得自行推断“后代可见”，应创建目标 scope 的新 immutable artifact 引用。

### 6.3 从 source artifact 导入 draft Profile

```http
POST /design-profiles/import
Content-Type: application/json

{
  "name": "AuthKit Frosted Glass Cathedral",
  "scope": { "projectId": "authkit-style-reference" },
  "sourceArtifactId": "design-source-123"
}
```

D1 只执行确定性 Markdown/CSS 解析，创建 `draft` Profile 和 conversion report，不在 HTTP handler 中等待模型调用。响应返回：

```ts
type ImportDesignProfileResponse = {
  designProfileDraft: DesignProfileDraft;
  conversionReport: DesignProfileConversionReport;
  requiresReview: true;
};
```

D1 不实现隐藏的同步模型调用。D2 若增加模型语义整理，必须新增明确的 `design_profile_import` AgentPhase、`design-profile-import` agent profile、只读 source tools、draft write tool、SSE 事件和 shared/BFF contract tests；模型结果创建新 draft revision，不能原地覆盖确定性解析结果。

Import 和 conversion-report endpoint 会读取或返回 source-derived data，因此同样要求 `internal_admin_authorized`；普通 Profile list/get 只返回 source metadata 和 integrity，不返回 excerpts 或 source content。

新增读取接口：

```http
GET /design-profiles/{id}/conversion-report
```

默认返回当前 draft revision 的 report；历史读取使用：

```http
GET /design-profiles/{id}/versions/{version}/conversion-report
```

### 6.4 创建或更新 Profile

现有 `POST /design-profiles` 和 `PUT /design-profiles/{id}` 保持路由不变，但 response 扩展为 `DesignProfileRecord`。V2/draft update 使用 optimistic version：

```ts
type UpdateDesignProfileRequest = {
  expectedVersion?: number;
  name: string;
  profile: DesignProfileDraft["candidate"] | DesignProfileV2Payload;
};
```

- V2 和 draft 的 `expectedVersion` 必填；
- V1 在一个兼容周期内允许缺失，并写 deprecation audit；
- version 不匹配返回 409，响应包含 current version，不执行写入；
- imported source 只能通过 import route 创建 draft；
- manual create 可以创建完整 active Profile，或显式创建 manual draft；
- list/get response 必须携带 `schemaVersion` 和 record status，使 BFF 能解析 union。

导入型 Profile 的服务端校验新增：

- source artifact 存在；
- source artifact 对当前 scope 可见；
- source hash 匹配；
- `signatureRules` 至少有一条 `required`；
- converter version 非空；
- imported Profile 的 `integrity` 必须为 `verified`。

组件字段兼容规则：

- API response 和 canonical Profile 一律输出 `role`；
- create/update 在兼容期接受 `role` 或旧字段 `intent`；
- 同时提供两个字段且值不一致时返回 400；
- shared schema 使用 transform 将 `intent` 规范化为 `role`；
- 下一个 major contract 移除 `intent` 输入兼容。

### 6.5 激活 draft Profile

```http
POST /design-profiles/{id}/activate
Content-Type: application/json

{
  "expectedVersion": 3
}
```

激活流程：

1. 使用 optimistic version 检查避免覆盖并发修改；
2. 把 draft candidate 解析为对应 `schemaVersion` 的 strict Profile；
3. 校验 required fields、source integrity、signature rules、template 和 token mapping；
4. validation issue 存在 blocking 项时返回 409 和完整 issue list；
5. 成功后追加 active revision；
6. draft history 和 conversion report 保留用于审计。

### 6.6 Fidelity report

新增只读接口：

```http
GET /design-profiles/{id}/versions/{version}/fidelity-report?surface=docs&template=fumadocs-docs
```

响应至少包含：

```ts
type DesignProfileFidelityReport = {
  designProfileId: string;
  version: number;
  schemaVersion: DesignProfileSchemaVersion;
  surface: "website" | "docs";
  template: string;
  styleContractVersion: "runtime-style-contract@p2" | "runtime-style-contract@p3";
  effectiveProfileHash: string;
  sourceIntegrity: "verified" | "unverified" | "missing";
  sourceHashMatches: boolean | null;
  requiredSignatureRuleIds: string[];
  capsuleIncludedRuleIds: string[];
  capsuleMissingRuleIds: string[];
  unsupportedExtendedTokens: string[];
  warnings: string[];
};
```

该接口只检查指定 revision、surface、template 和 Runtime 能力，不触发模型，也不修改项目。缺少 surface 或 template 时返回 400，避免生成无法解释的 capability report。

新增 source、import、activation 和 fidelity API 后，必须同步更新 Runtime API freeze 文档、shared API types、runtime client、mock BFF contract tests 和错误响应表。

## 7. Design Capsule 生成规则

### 7.1 替换当前硬截断 renderer

新的 renderer 不再序列化完整 JSON 后调用 `chars().take(8000)`。它应按固定 section 和预算生成确定性 Markdown。

推荐预算：

| Section | 预算 | 截断策略 |
|---|---:|---|
| Identity / source | 500 | 不截断 |
| Required signature rules | 2,500 | 不截断，超限则 Profile 校验失败 |
| Visual direction | 1,200 | 按完整条目保留 |
| High-impact tokens | 2,000 | 按 token 条目保留 |
| Component recipes | 1,800 | 优先 required component |
| Content / accessibility / governance | 1,500 | 每类至少保留一条 |
| Footer / hashes | 500 | 不截断 |

默认总预算 10,000 字符。预算不足时只能删除完整的 `preferred` 条目，禁止在 JSON、句子或 Markdown section 中间截断。

Capsule 中 signature rule 只渲染 ID、statement、appliesTo 和紧凑 verification summary，不直接 pretty-print 完整 JSON。若全部 required rules 在 schema 上限内仍超过 2,500 字符，Profile activation 失败并要求拆分或缩短规则，不能把失败推迟到 Build run。

### 7.2 Capsule 固定结构

```markdown
# Design Capsule

## Identity
## Source Integrity
## Required Signature Rules
## Visual Direction
## High-impact Tokens
## Required Component Recipes
## Content and Voice
## Accessibility
## Governance
## Runtime Capability Gaps
```

### 7.3 Workspace 文件

当 Build/Edit/Review run 绑定 imported Profile 时，Runtime 写入：

```text
inputs/brief.md
inputs/content-sources.json
inputs/design-profile.json
inputs/design.md
inputs/design-source.md
inputs/design-source-index.json
state/context.md
```

其中：

- `design-profile.json` 是 canonical structured source；
- `design.md` 是新的 Design Capsule；
- `design-source.md` 是 immutable source artifact 的原始 bytes，禁止添加 header 或 hash；
- `design-source-index.json` 记录 Markdown section、byte range、section hash 和 priority；
- profile/source/capsule hash 写入 index、context 和 run snapshot，不能改写原始 source。

Source index 最小结构：

```ts
type DesignSourceIndex = {
  sourceArtifactId: string;
  sourceHash: string;
  sizeBytes: number;
  sections: Array<{
    id: string;
    heading: string;
    startByte: number;
    endByte: number;
    sha256: string;
    requiredByRuleIds: string[];
  }>;
};
```

### 7.4 Agent 读取策略

run snapshot 新增：

```ts
type DesignFidelityMode = "profile_only" | "source_fallback";
```

- imported initial Build 默认 `source_fallback`；
- 普通 Edit 默认 `profile_only`；
- Profile resync、visual drift repair 和真实回归 C 组使用 `source_fallback`；
- A/B/C E2E 可以显式设置 mode，但产品侧不能在不记录 audit 的情况下临时切换；
- mode、source budget 和已读 section hashes 必须进入 run snapshot。

Build run：

- 必须读取 `brief.md`、`design.md` 和 `design-profile.json`；
- `profile_only` 不向 agent 暴露 raw source tool content，只使用 Profile 和 Capsule；
- `source_fallback` 下，source 小于等于 32 KiB 时可以整文件读取 `design-source.md`；
- `source_fallback` 下，source 大于 32 KiB 时必须先读取 `design-source-index.json`，再通过 `design_source.read_sections` 读取 required sections；
- required sections 是 signature rules 引用的 section，加上 conversion report 中存在 ambiguity 的 section；
- 单个 `design_source.read_sections` 返回不超过 16 KiB，单个 run 的 raw source 注入预算默认不超过 48 KiB；
- 超出预算时进入 `needs_user_input:design_profile_source_budget_exceeded`，不能静默截断；
- 在 mode 对应的 required inputs/sections 未读取前，不允许调用 `project.init` 或第一次写入 app source。

```ts
type ReadDesignSourceSectionsRequest = {
  sourceArtifactId: string;
  sectionIds: string[];
  expectedSourceHash: string;
};
```

tool result 返回完整 section、byte range 和 section hash。read tracking 按 section ID/hash 记录，Gate 不以单个文件路径命中作为已完成读取的证据。

Tool executor 必须验证 `sourceArtifactId` 和 `expectedSourceHash` 与当前 run snapshot 完全一致，不能把该 tool 当作任意 artifact 读取接口；section ID 必须来自当前 source index。

Raw source 一律标记为 `untrusted_design_reference`：

- 只允许提供设计 token、组件、内容语气和视觉参考；
- source 中要求调用工具、修改权限、读取其他路径、忽略 system prompt 或上传数据的文本一律无效；
- `design_source.read_sections` tool result 必须携带 trust label；
- system prompt 明确声明 Profile/source 低于用户已确认 Brief 和 Runtime policy；
- converter 检测到 operational instruction 时写入 conversion warning，不把它转换成 signature rule。

读取 Gate 由 `design_context_read_gate` agent hook 执行。默认阻塞所有 sandbox/workspace non-read-only tools，至少覆盖：

```text
project.init
style.update_tokens
fs.write
fs.write_chunk
fs.commit_chunks
fs.patch
fs.multi_patch
project.ensure_dependencies
project.build
preview.publish
shell.run
```

对 `fs.*` 只在目标路径位于 app root 或 `state/style-contract.json` 时阻塞；Runtime 自己执行的 bootstrap input writes 不受该 hook 影响。

Edit run：

- 必须读取 `design.md` 和当前项目 source；
- 用户要求重新同步 Profile、修改品牌风格或修复视觉漂移时，必须读取 `design-profile.json` 和相关 source sections；
- 普通文案编辑不强制重新读取完整 source。

Review run：

- 必须读取 `signatureRules`；
- imported Profile 至少读取 `design.md` 和 `design-profile.json`；
- visual rule 依赖原始细节时，按 source index 读取对应 required sections；
- required visual rule 无法自动验证时，必须创建 visual review finding，不能默认通过。

## 8. Style Contract V3

### 8.1 兼容策略

- `runtime-style-contract@p2` 保留现有 12 个 token；
- 新模板声明 `runtime-style-contract@p3`；
- p3 包含 base tokens 和模板实际支持的 extended tokens；
- Profile 可以同时提供 `runtimeTokenMapping` 与 `extendedTokenMapping`；
- p2 模板只应用 base token，并报告 capability gap。

### 8.2 Website 最小扩展集

Astro Website 首批支持：

```text
color.action
color.actionContrast
color.authSubmit
font.display
font.mono
type.display.size
type.display.lineHeight
type.display.letterSpacing
type.body.letterSpacing
spacing.pageGutter
spacing.section
spacing.cardPadding
spacing.gridCell
radius.input
radius.badge
radius.largeCard
gradient.display
gradient.ambient
shadow.cardStrong
```

### 8.3 Docs 最小扩展集

Fumadocs Docs 首批支持：

```text
color.action
color.actionContrast
color.authSubmit
font.display
font.mono
type.display.letterSpacing
type.body.letterSpacing
spacing.pageGutter
spacing.section
radius.input
radius.badge
gradient.display
```

Docs 不要求实现 Website 专用 hero composition。effective Profile 的 docs surface override、template override、`appliesTo` 和 `content.docs` 共同决定 Docs 的适配规则。

## 9. Preset V2 修复

### 9.1 AuthKit 必须补齐

至少补齐以下 required signature rules：

1. 页面背景必须为 `#05060f`。
2. display font 使用 aeonikPro 或 Space Grotesk，weight 500。
3. eyebrow 使用 mono/dotDigital 风格，15px，`0.10em` tracking。
4. 最大标题使用 `#d8ecf8 -> #98c0ef` Skywash gradient。
5. 背景必须有 80-100px blueprint grid 和 top-center conic spotlight。
6. Website hero 必须显示重叠的 auth form product signal。
7. 紫色 `#663af3` 只用于 auth submit/continue action。
8. card elevation 使用 inset frost edge 和 cool dark halo。

Profile tokens 必须包含：

- 三种字体角色和 fallback；
- 完整 type scale；
- gradient recipes；
- input/badge/icon/modal 圆角；
- glass card、auth modal、feature card elevation；
- grid、spotlight 和 customization accent colors。

### 9.2 ElevenLabs 必须补齐

至少补齐以下 required signature rules：

1. 页面背景使用 `#fdfcfc`，不能使用纯白作为 canvas。
2. display font 使用 Waldenburg 或 Inter 300 fallback。
3. 32px 以上标题使用 `-0.02em` tracking。
4. 主按钮只能使用黑色 filled pill，不能使用 violet/orange CTA。
5. violet `#0447ff` 和 orange `#ff4704` 只能用于 product visual。
6. feature card 使用 `#f5f3f1`、20-24px radius、无重阴影。
7. Website 必须包含 audio/product visual 或明确的 voice product evidence。
8. section divider 使用 `#ebe8e4` 1px hairline。

Profile tokens 必须包含：

- Waldenburg/Inter/Geist Mono 三种字体角色；
- display 和 body tracking；
- Graphite、Smoke、Ash 的文本层级；
- 96-125px section rhythm 和 64px gutter；
- card/input/button 的独立 radius；
- audio sphere gradient recipe。

## 10. Fidelity Gate

### 10.1 Pre-build gate

Build 开始修改 sandbox 前检查：

- Profile status 为 active；
- `design-profile@2` imported Profile 的 source integrity 必须为 verified；
- legacy `design-profile@1` 缺少 source 时按 `legacy-warning` 继续，并写入 audit；
- source/profile hash 与 run snapshot 一致；
- template 在 `allowedTemplates` 中；
- effective Profile 已按 surface/template 解析并记录 hash；
- capsule 包含全部 required signature rules；
- Style Contract capability gap 已写入 run metadata；
- required Profile/Capsule inputs 已读取；`source_fallback` mode 下 required source section hashes 也已读取。

阻塞状态使用：

```text
needs_user_input:design_profile_source_missing
needs_user_input:design_profile_integrity_failed
needs_user_input:design_profile_capability_gap
needs_user_input:design_profile_source_budget_exceeded
```

只有 required rule 依赖模板不支持的能力时，才进入 capability gap；preferred rule 不阻塞生成。

### 10.2 Post-build gate

`preview.publish` 后、`run.complete` 前执行：

- token assertions；
- computed-style assertions；
- source-pattern assertions；
- screenshot blank/viewport checks；
- visual-review rubric。

执行上下文必须固定 effective Profile version/hash、surface、template、route、viewport 和 artifact version。computed-style 使用规范化 comparator；`source-pattern` 只能证明实现存在，不能证明元素可见或满足 required visual rule。

Runtime 通过隔离的 headless Chrome/Chromium CDP 会话批量采集 computed-style evidence。采证进程使用独立临时 profile，完成后等待浏览器退出再清理；浏览器不可用、导航失败、超时、selector/property 执行失败或采证进程异常退出时，required assertion 必须明确失败并记录 stderr，不能回退为 token/source 命中或沿用上一次证据。

required assertion 失败时：

- Build run 不得直接 complete；
- 最多允许一次自动修复；
- 同一 assertion 第二次失败时进入 Review finding 或 `partial`，保留可访问 candidate。

每条 assertion 记录 `ruleId`、route、selector、raw actual value、normalized actual value、expected、comparator 和 pass/fail reason，避免只保存汇总数量。

### 10.3 不使用单一像素相似度作为 Gate

真实模型输出存在布局和内容随机性。保真 Gate 应优先验证稳定的不变量：

- computed tokens；
- font family/weight/tracking；
- CTA 色彩约束；
- hero 是否包含产品信号；
- 禁止项是否出现；
- required component 是否存在。

截图像素差只作为辅助诊断，不作为唯一通过条件。

## 11. Observability

每个绑定 DesignProfile 的 run 记录：

```json
{
  "designProfileId": "...",
  "designProfileSchemaVersion": "design-profile@2",
  "designProfileVersion": 2,
  "designProfileHash": "...",
  "effectiveProfileHash": "...",
  "surface": "website",
  "template": "astro-website",
  "designFidelityMode": "source_fallback",
  "sourceReadBudgetBytes": 49152,
  "sourceArtifactId": "...",
  "sourceHash": "...",
  "capsuleHash": "...",
  "requiredSignatureRules": 8,
  "verifiedSignatureRules": 7,
  "readSourceSectionIds": ["tokens-colors", "agent-prompt-guide"],
  "unsupportedExtendedTokens": ["gradient.ambient"]
}
```

新增内部 audit/conversation item：

```text
design_profile_source_resolved
design_profile_capsule_rendered
design_profile_tokens_applied
design_profile_fidelity_checked
design_profile_capability_gap
```

若需要暴露到 public SSE，必须同步更新 shared event schema 和 BFF contract tests；第一阶段可以只记录内部 audit 和 metric，避免无意扩展已冻结的 SSE surface。

## 12. 测试方案

### 12.1 Unit tests

Source artifact：

- Markdown 创建后 byte-for-byte 读取一致；
- base64 decode 后 server hash 与 `clientSha256` 一致；
- hash 由服务端计算且稳定；
- hash 不匹配能够检测；
- 超过 256 KiB 被拒绝；
- 编码后 request body 超过 384 KiB 被拒绝；
- 非 UTF-8 或不支持的 media type 被拒绝。
- 缺少或错误 service authorization 被拒绝；
- import 和 conversion-report 缺少 service authorization 被拒绝；
- logs、audit、tracing 和 error response 不包含 token、base64 或 source plaintext；
- project artifact 不能被其他 project 的 Profile 引用；
- `fileName = ../../secret` 不影响实际 storage path；
- metadata/blob crash recovery 能区分 missing blob 和 orphan blob。

Profile schema：

- 缺少 `schemaVersion` 的历史 Profile 只在兼容层解析为 V1；
- `version` revision 与 `schemaVersion` 独立变化；
- deterministic import 可以保存不完整 draft；
- draft 不能 bind 或进入 run context；
- activation 对 blocking validation issues 返回 409；
- activation 成功创建新的 strict active revision；
- imported Profile 缺少 source artifact 被拒绝；
- signature rule ID 重复被拒绝；
- required rule 缺少 verification 被拒绝；
- rule count、statement 和 rubric 超过上限时 activation 被拒绝；
- V1 Profile 仍可解析；
- V2 typography、gradient、elevation 可 round-trip。
- Website/Docs/template override 按规定 precedence 合并；
- required rule 不能被 override 静默删除。

Capsule renderer：

- required signature rules 全部出现；
- 不出现半截 JSON、半截句子或 truncation marker；
- content/accessibility/governance 至少各有一个完整条目；
- 同一 Profile 多次渲染 hash 一致；
- 超预算时只删除完整 preferred entry。
- source 大于 32 KiB 时不会通过 `fs.read` 整体进入模型上下文；
- required section 缺失或 hash 不符时 read gate 阻塞；
- `design_source.read_sections` 读取非当前 run artifact/section 时被拒绝；
- raw source 注入超过 48 KiB 时进入明确状态而不是截断。
- source 中的 prompt/tool/permission 指令不会改变 agent policy，也不会进入 signature rules。

Style Contract：

- p2 只应用 base tokens；
- p3 应用支持的 extended tokens；
- 不支持 token 明确进入 skipped list；
- CSS validator 拒绝分号、花括号、换行和超长值；
- Edit run 不自动覆盖现有 token。

Verification：

- `#fdfcfc` 与浏览器返回的等价 `rgb(...)` 可以通过 color comparator；
- font list 支持 contains comparator 和引号规范化；
- letter-spacing 支持 numeric tolerance；
- route/selector 无匹配时 required assertion 失败；
- source-pattern 单独命中不能让 required visual rule 通过；
- fidelity report 必须绑定 version、surface、template 和 effective profile hash。

### 12.2 Integration tests

- create source artifact -> import draft -> review/update -> activate -> bind -> start run；
- workspace 同时存在 Profile、Capsule、raw source 和 source index；
- source/profile/capsule hash 写入 context 和 run metadata；
- 未读取 required input 时 `project.init` 被 recoverable gate 阻止；
- 读取完成后可以继续；
- 大 source 只读取 required section IDs，read tracking 保存 section hash；
- Profile version 更新不会改变已启动 run 的 snapshot；
- source integrity 失败时 sandbox 未发生 mutation；
- V1 unverified Profile 产生 warning 但不阻塞历史项目；
- V2 unverified imported Profile 在 mutation 前阻塞；
- `profile_only` 不产生 raw source tool result，`source_fallback` 只读取预算内 required sections。

### 12.3 Preset fidelity fixtures

AuthKit fixture 至少断言：

```text
root background == #05060f
runtime primary == #663af3
h1 font contains Space Grotesk or approved fallback
h1 has gradient text treatment
eyebrow letter-spacing == 0.10em
background contains grid treatment
hero contains auth-form product signal
violet is not used by non-auth navigation CTA
```

ElevenLabs fixture 至少断言：

```text
root background == #fdfcfc
feature surface == #f5f3f1
h1 weight == 300
h1 letter-spacing approximately -0.02em
primary CTA background == #000000
violet/orange do not appear in interactive CTA styles
website product visual includes audio sphere or equivalent voice evidence
divider uses #ebe8e4 hairline
```

### 12.4 Real provider A/B/C

使用真实 DeepSeek API，分别运行三组，以隔离结构化 Profile 和 raw source fallback 的贡献：

```text
A: 原始 DESIGN.md 作为 design_md
B: DesignProfile V2 + Capsule，designFidelityMode=profile_only
C: DesignProfile V2 + Capsule，designFidelityMode=source_fallback
```

必须保持：

- 同一模型；
- 同一用户 prompt；
- 同一 template；
- 同一 Runtime policy；
- 同一 viewport；
- 同一验收脚本。

每个 preset、每个组至少运行 3 次。provider 支持稳定 seed 时记录 seed；不支持时记录 request/model parameters，并使用中位数而不是单次结果。

评分：

```text
requiredPassRate = passed required rules / total required rules
allRequiredPass = every required rule passed
repairCount = automatic repair attempts before terminal status
capabilityGapCount = unsupported required capabilities
```

验收阈值：

- deterministic token/comparator fixtures 必须 100% 通过；
- B 组 requiredPassRate 中位数不得比 A 组低超过 5 个百分点；
- C 组不得低于 B 组，并应减少 source-related drift；
- B/C 生产候选组不能出现关键 color/typography/CTA required rule 的系统性失败；同一 rule 在 3 次中失败至少 2 次定义为系统性失败；A 是 raw source 控制组，允许暴露系统性 drift，但必须单独报告，不能计入 B/C 放行；
- repairCount 和 capabilityGapCount 必须单独报告，不能用最终页面通过掩盖多次修复。

Website 与 Docs 分开评估。不能把 Website 原始参考生成结果和 Docs 模板结果做直接像素比较。

## 13. 实施阶段

### Phase 0：建立回归基线

交付：

- 固化现有 AuthKit 原始 `design_md` E2E evidence；
- 为 ElevenLabs 增加同等基线；
- 保存 prompt、model、template、source hash、artifact screenshot 和 computed-style report。

完成标准：两套基线都可以独立复跑，并能定位输入和输出版本。

### Phase 1：无损 source artifact

交付：

- `DesignSourceArtifact` 类型和 RuntimeStore；
- service-authenticated create/get/content API；
- base64 byte upload、client/server hash 验证；
- source hash 和 integrity validation；
- scope containment 和 filename/path hardening；
- Profile source 引用；
- shared schemas、client 和 contract tests。

完成标准：原始文档可以 byte-for-byte round-trip，Profile 不再只保存本地文件名。

### Phase 2：Profile V2 与 preset 修复

交付：

- `schemaVersion` 与 revision version 分离；
- `DesignProfileDraft`、import 和 activation flow；
- `signatureRules`；
- `ComponentGuideline.role` canonicalization 和 `intent` 兼容解析；
- typography scale、gradients、elevation、component radius；
- surface/template overrides 和 effective Profile hash；
- AuthKit V2；
- ElevenLabs V2；
- V1 fallback 和版本 diff。

完成标准：两份原始文档中的 required signature traits 都有结构化归属，并通过 fixture validation。

### Phase 3：Design Capsule 与读取 Gate

交付：

- section-aware capsule renderer；
- `design-source.md` workspace materialization；
- source index 与 `design_source.read_sections`；
- required input/section read gate 和 48 KiB source budget；
- capsule/source hash audit；
- agent prompt 更新。

完成标准：不再出现硬截断；Build Agent 在修改源码前读取所有 required inputs。

### Phase 4：Style Contract V3

交付：

- optional extended tokens；
- Astro Website p3 contract；
- Fumadocs Docs p3 contract；
- token apply/skip report；
- Edit token non-reset regression。

完成标准：两个 preset 的 required computed-style assertions 可以由 Runtime 自动应用并验证。

### Phase 5：Fidelity Review 与真实 API 验收

交付：

- pre-build/post-build fidelity gate；
- version/surface/template-aware fidelity report；
- AuthKit 和 ElevenLabs real-provider A/B/C；
- Website/Docs 独立报告；
- capability gap 和 drift findings。

完成标准：deterministic fixtures 100% 通过，B/C 组按已定义 requiredPassRate 阈值不低于原始 `design_md` 基线。

## 14. Commit 计划

建议按以下顺序提交，保证每个 commit 可独立 review 和回滚：

1. `feat(runtime): persist authorized immutable design sources`
2. `feat(shared): add design profile schema versions and drafts`
3. `feat(runtime): import and activate design profile drafts`
4. `feat(runtime): resolve surface-specific effective profiles`
5. `feat(runtime): render deterministic design capsules`
6. `feat(runtime): read indexed design source sections`
7. `feat(runtime): add style contract v3 extended tokens`
8. `feat(runtime): verify versioned design profile fidelity`
9. `test(runtime): add authkit and elevenlabs fidelity fixtures`
10. `test(e2e): compare design md profile and source fallback builds`

不要把 source persistence、schema migration、renderer 和 Style Contract V3 合并成一个 commit。它们的风险和回滚边界不同。

## 15. 迁移方案

现有 Profile 处理规则：

- 不修改原 version；
- 缺少 `schemaVersion` 的历史 Profile 在兼容读取层标记为 `design-profile@1`；
- V1 `source.integrity` 缺失时按 `unverified` 和 `legacy-warning` 处理；
- `legacy-warning` 允许已有项目继续 Build/Edit，并写入 audit，避免破坏现有流程；
- V2 imported Profile 使用 `strict` integrity policy，未 verified 时不得激活或修改 sandbox；
- 管理端显示“需要重新导入来源”的 warning；
- 重新导入时创建新 source artifact 和 Profile version；
- AuthKit、ElevenLabs preset 以 V2 新版本发布，不覆盖 V1 审计记录。

首次迁移：

1. 为两份原始 `DESIGN.md` 创建 immutable source artifacts。
2. 生成并人工 review V2 Profile。
3. 运行 fidelity report。
4. 创建 Profile version 2。
5. 只对新 run 使用 version 2；历史 run 继续绑定原 snapshot。

## 16. 风险与控制

| 风险 | 影响 | 控制措施 |
|---|---|---|
| 原始文档增大模型上下文 | 成本和延迟上升 | 32 KiB 阈值、section index、单次 16 KiB 和单 run 48 KiB 预算 |
| Source artifact 泄露 | 未发布设计内容被读取 | 所有 source API 强制 service auth 和 scope containment |
| Raw source prompt injection | source 试图覆盖工具或权限策略 | trust label、system precedence、converter warning 和 policy regression tests |
| 扩展 token 过多 | 模板维护成本上升 | 先支持两个 preset 的高辨识度最小集合 |
| 模型仍忽略部分规则 | 输出漂移 | required read gate + signature assertions + 一次自动修复 |
| Website 和 Docs 语义不一致 | 错误比较 | effective Profile 使用 surface/template overrides，验收分模板执行 |
| V1 Profile 无 source | 无法证明来源完整性 | `legacy-warning` 兼容，V2 使用 strict policy |
| public SSE 扩展造成 BFF 回归 | 合约不一致 | 第一阶段只写内部 audit/metric |
| 字体资源不可用 | fallback 后视觉变化 | Profile 明确 approved fallback，并验证 computed font |

## 17. Definition of Done

本方案完成必须同时满足：

- [x] 导入源可以 byte-for-byte 读取，服务端 hash 验证通过。
- [x] Source API 需要 service auth，scope containment 和 filename/path tests 通过。
- [x] imported Profile 不再只引用本地文件名。
- [x] `schemaVersion` 与 Profile revision 已分离。
- [x] deterministic import 生成 draft，只有 activation 能创建 strict active revision。
- [x] AuthKit 和 ElevenLabs required signature rules 已结构化。
- [x] Runtime、shared schema 和 preset 统一输出 `ComponentGuideline.role`。
- [x] Website/Docs/template effective Profile 合并和 hash 可复现。
- [x] Design Capsule 不包含硬截断或半截 JSON。
- [x] 所有 required signature rules 都进入 Capsule。
- [x] Build 在 required inputs/sections 未读取时不能修改 app source。
- [x] 大 source 不会作为单个 `fs.read` tool result 进入模型上下文。
- [x] Raw source operational instructions 不能改变 agent tools、permissions 或 system precedence。
- [x] p2 Style Contract 保持兼容。
- [x] p3 可以应用并报告 extended token。
- [x] Fidelity report 绑定 revision、surface、template 和 effective Profile hash。
- [x] AuthKit Website fidelity fixture 全部通过。
- [x] ElevenLabs Website fidelity fixture 全部通过。
- [x] Docs 使用独立适配规则和验收，不与 Website 做错误像素对比。
- [x] 真实 DeepSeek A/B/C 每组至少运行 3 次并记录 model、prompt、template、source/profile hash。
- [x] Deterministic assertions 100% 通过，B/C requiredPassRate 满足相对 A 组阈值。

## 18. 推荐执行顺序

Phase 0 到 Phase 5 已按本文顺序实施。后续不再继续扩 schema，先保持当前 API 和 fidelity 规则稳定，进入分 commit review、迁移演练和持续回归。

### 18.1 实施结果

- immutable source artifact、V2 draft/import/activation、effective Profile、Capsule、source index/read gate、Style Contract p3 和 post-publish fidelity gate 已落地；
- fidelity evidence 使用独立 headless Chrome/Chromium CDP 会话采集真实 computed style；浏览器不可用或采证失败会明确阻断 required rule；
- build source 使用 `sourceFingerprint` 绑定 fidelity report；失败后原样重发会返回 `design_profile.no_source_change_after_fidelity_failure`，不再消耗修复机会；
- `local-e2e` 的 `preview.publish` 忽略模型指定的 URL/port/command/mode，始终创建 Runtime 管理的动态端点，避免 `localhost:4321` 旧页面污染；
- AuthKit 新增 `color.action`、`color.authSubmit` 和正反双向 violet 语义规则：至少一个 auth submit 必须为 violet，非认证交互不得泄漏 violet；
- fidelity report 的 `repairContext` 明确给出 Style Contract、token file、global CSS 和 component root，修复不得写入未导入的旁路 CSS。

### 18.2 验收证据

完整真实模型矩阵：

```text
.runtime-evidence/design-fidelity-matrix-20260710-060248
requested model: deepseek-chat
provider reported model: deepseek-v4-flash
repetitions: 3 per preset/variant
thresholdsPassed: true
AuthKit A median: 0.625
AuthKit B median: 1.000
AuthKit C median: 1.000
ElevenLabs A median: 0.750
ElevenLabs B median: 1.000
ElevenLabs C median: 1.000
capabilityGapCount: 0 for every group
```

该矩阵 18 次中有 1 次 ElevenLabs C 因模型显式复用 `localhost:4321`，对旧页面反复采证并达到 max turns。该失败保留在 evidence 中，并直接推动受管端点修复。修复后对同一 preset/variant 连续运行 3 次，结果全部为 Runtime completed、computed-style exit 0、external fidelity exit 0：

```text
.runtime-evidence/design-elevenlabs-C-website-http-20260710-064700
.runtime-evidence/design-elevenlabs-C-website-http-20260710-064914
.runtime-evidence/design-elevenlabs-C-website-http-20260710-065106
```

确定性回归：

```text
cargo test --manifest-path services/runtime/Cargo.toml
  0 failed; external/network tests remain explicitly ignored

packages/shared:
  typecheck passed
  22 tests passed

format/syntax:
  cargo fmt --check passed
  git diff --check passed
  all four runtime .mjs scripts passed node --check
```

### 18.3 下一步

1. 按第 14 节边界拆分并 review commits，不把 source persistence、schema、Capsule、Style Contract 和 E2E 混成一个不可回滚提交。
2. 在 staging 使用真实 BFF/service auth 演练 V1 legacy-warning 到 V2 strict activation 的迁移。
3. 将 A/B/C 矩阵纳入定期回归；对 provider model 变化、repairCount 上升或同一 required rule 连续失败设置告警。
