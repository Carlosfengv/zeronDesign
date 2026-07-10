---
date: 2026-07-09
status: proposed
type: product-runtime-contract
topic: design-profile
related:
  - ./2026-07-04-agent-harness-design.md
  - ./2026-07-04-rust-runtime-spec.md
  - ./2026-07-08-project-lifecycle-generation-edit-build-plan.md
  - ./2026-07-08-runtime-api-freeze.md
  - ./2026-07-09-live-sse-implementation-guide.md
---

# DesignProfile 完整规格

## 1. 背景与定位

现有产品文档已经给 DesignProfile 留了位置，但刻意没有在 Phase A / lifecycle 阶段引入：

- `brief.md` 是已确认的生成契约。
- `design.md` 是可选但长期重要的风格约束。
- `context.md` 是作品级长期记忆。
- Build/Edit Agent 都必须读取 `brief.md`，并在存在时读取 `design.md`。
- lifecycle 文档明确要求：只有 Website / Docs 的 create -> edit -> rebuild -> promote 生命周期稳定后，才把 DesignProfile 作为 style intelligence layer 引入。

当前 runtime 已经具备：

- public runtime API；
- live SSE；
- Website / Docs build-edit lifecycle；
- `style-contract.json`；
- `style.update_tokens`；
- provider gate 真实验证。

因此 DesignProfile 的正确定位是：

```text
DesignProfile = 可保存、可复用、可验证的产品设计上下文契约
```

它不是完整设计系统管理台，也不是 Figma 替代品。它的首要目标是让 runtime 在 brief/build/edit/review 中稳定理解并执行同一套品牌、视觉、组件、内容和可访问性规则。

## 2. 目标

- 把零散的 `visualDirection`、`design.md`、runtime style contract 统一成结构化上下文。
- 支持 Website 和 Docs 两种项目类型。
- 让 Build/Edit Agent 可读取同一份 profile，减少生成漂移。
- 能映射到当前 `style-contract.json` 的可编辑 tokens。
- 允许产品侧做最小 CRUD，但避免一开始做复杂设计系统管理台。
- 为后续 Figma/design-system import、版本管理、组件库治理预留字段。

## 3. 非目标

- 不在第一阶段做完整可视化 token editor。
- 不做 Figma import/export。
- 不自动从页面反推完整设计系统。
- 不做多租户权限模型。
- 不做复杂发布流、审批流、分支合并。
- 不要求 agent 生成 pixel-perfect 设计系统。
- 不替代 `brief.md`。Brief 仍然决定生成什么，DesignProfile 决定如何表达。

## 4. 与现有概念的关系

```text
Content Sources
  -> Brief Agent
  -> brief.md / Brief JSON
       defines what to build

DesignProfile
  -> design-profile.json / design.md
       defines how it should look, sound, and behave

Build/Edit Agent
  -> reads brief.md
  -> reads design-profile.json and design.md if present
  -> initializes or updates project
  -> writes state/style-contract.json
  -> uses style.update_tokens for token-safe changes

context.md
  -> records project-specific decisions after generation/edit
```

优先级规则：

1. 用户本轮明确指令最高。
2. 已确认 Brief 次之。
3. DesignProfile 约束次之。
4. `context.md` 记录的历史决策次之。
5. 模板默认样式最低。

如果用户指令或 Brief 与 DesignProfile 冲突，Agent 不应静默覆盖，应触发明确的冲突状态，再由用户决定是否临时覆盖或更新 profile。

### 4.1 Scope 解析规则

DesignProfile 可以挂在 project / workspace / organization 任一层，但 run 启动时必须解析为唯一 profile：

```text
explicit inputContext.designProfileId
  -> project active profile
  -> workspace default active profile
  -> organization default active profile
  -> no profile
```

规则：

- 如果 `designProfileId` 显式传入，runtime 只使用该 profile，不再 fallback。
- 显式 profile 必须对当前 project 可见，否则 `POST /runs` 返回明确错误。
- 同一 project 同一时间最多一个 active profile。
- workspace / organization default 只能作为 fallback；一旦 project 绑定 active profile，以 project 为准。
- 如果多个 fallback 同时匹配且无法确定优先级，run 启动失败，要求产品侧显式传 `designProfileId`。
- run 必须记录解析后的 `designProfileId`、`version` 和 profile hash，保证后续审计与 replay 可解释。

### 4.2 冲突状态机

冲突检查必须发生在 sandbox mutation 前，避免已经改写源码后才发现规则不一致。

```text
StartRun / ContinueRun
  -> resolve DesignProfile
  -> preflight conflict check
  -> if conflict:
       update run status = needs_user_input
       emit state.changed(needs_user_input:design_profile_conflict)
       append conversation item approval_request
       do not spawn or resume mutating agent session
  -> if user confirms override:
       record override in run metadata/context.md
       resume run
  -> if user chooses update profile:
       update profile first
       resume with new profile version/hash
```

MVP 冲突检查可以先覆盖高信号场景：

- Brief recommended template 不在 `technical.allowedTemplates` 中。
- User edit 明确要求的风格被 `visual.avoidKeywords` 或 `brand.messaging.forbiddenClaims` 禁止。
- User edit 要求改 token，但 profile governance 为 `required` 且该 token 不允许偏离。
- Docs run 请求使用未注册 MDX 组件，而 profile content rules 禁止。

## 5. 数据模型

### 5.1 TypeScript Contract

```ts
type DesignProfileStatus = "draft" | "active" | "archived";

type DesignProfileScope = {
  projectId?: string;
  workspaceId?: string;
  organizationId?: string;
};

type DesignProfile = {
  id: string;
  name: string;
  status: DesignProfileStatus;
  version: number;
  scope: DesignProfileScope;

  source: {
    kind: "manual" | "brief" | "imported" | "generated";
    sourceIds?: string[];
    notes?: string;
  };

  product: {
    name: string;
    category: string;
    audience: string[];
    primaryUseCases: string[];
    productQualities: string[];
  };

  brand: {
    voice: BrandVoice;
    messaging: MessagingRules;
  };

  visual: {
    direction: string;
    principles: string[];
    moodKeywords: string[];
    avoidKeywords: string[];
    composition: CompositionRules;
    imagery: ImageryRules;
    motion: MotionRules;
  };

  tokens: DesignTokens;
  runtimeTokenMapping: RuntimeTokenMapping;
  components: ComponentRules;
  content: ContentRules;
  accessibility: AccessibilityRules;
  technical: TechnicalRules;
  governance: GovernanceRules;

  createdAt: string;
  updatedAt: string;
};
```

### 5.2 BrandVoice

```ts
type BrandVoice = {
  tone: string[];
  sentenceStyle: "concise" | "balanced" | "editorial" | "technical";
  vocabulary: {
    prefer: string[];
    avoid: string[];
  };
  writingRules: string[];
};
```

用途：

- Brief Agent 可用它细化 `visualDirection` 和 assumptions。
- Build/Edit Agent 用它写页面文案、CTA、空状态、错误提示。
- Review Agent 用它判断内容是否跑偏。

### 5.3 MessagingRules

```ts
type MessagingRules = {
  headlineStyle: string;
  bodyStyle: string;
  ctaStyle: string;
  proofStyle: string;
  forbiddenClaims: string[];
};
```

要求：

- 禁止无依据夸张承诺。
- 对内部工具、runtime、harness 类产品，优先清晰、可信、可验证。
- 文案服务任务流，不做营销噪声。

### 5.4 CompositionRules

```ts
type CompositionRules = {
  density: "compact" | "balanced" | "spacious";
  informationHierarchy: string[];
  layoutPreference: string[];
  navigation: "top-nav" | "side-nav" | "docs-sidebar" | "single-page";
  cardUsage: "minimal" | "moderate" | "heavy";
  sectionRhythm: string;
};
```

当前产品建议：

- SaaS / CRM / operational tool 应偏密集、克制、扫描友好。
- Docs 项目应优先清晰导航、可读性、代码/步骤结构。
- Website 项目可以更有品牌表达，但不要牺牲可检查性。

### 5.5 ImageryRules

```ts
type ImageryRules = {
  allowed: string[];
  avoid: string[];
  productSignal: string;
  iconStyle: string;
  screenshotPolicy: string;
};
```

建议默认：

- 真实产品、状态、工作流优先于抽象装饰。
- 避免纯氛围图、深色模糊背景、无信息插画。
- 工具型产品应优先展示界面状态、运行证据、preview、timeline。

### 5.6 MotionRules

```ts
type MotionRules = {
  level: "none" | "subtle" | "expressive";
  allowed: string[];
  avoid: string[];
  reducedMotionBehavior: string;
};
```

第一阶段 runtime 可以只把 motion 作为 prompt 约束，不要求模板自动实现动画系统。

## 6. Tokens

DesignProfile 的 token 范围要比当前 runtime style contract 更完整，但必须能降级映射到现有可编辑 tokens。

```ts
type DesignTokens = {
  color: {
    background: string;
    surface: string;
    surfaceStrong: string;
    text: string;
    muted: string;
    primary: string;
    primaryContrast: string;
    border: string;
    success?: string;
    warning?: string;
    danger?: string;
    info?: string;
  };
  typography: {
    fontSans: string;
    fontMono?: string;
    scale: "compact" | "default" | "editorial";
    headingWeight: string;
    bodyWeight: string;
    lineHeight: string;
  };
  radius: {
    card: string;
    control: string;
    pill?: string;
  };
  shadow: {
    soft: string;
    focus?: string;
  };
  spacing: {
    density: "compact" | "default" | "comfortable";
    pageMaxWidth: string;
    sectionGap: string;
    controlHeight: string;
  };
};
```

### 6.1 Runtime Token Mapping

当前 runtime 已有 `state/style-contract.json`，可编辑 token 名为：

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

DesignProfile 必须提供到这些 token 的映射：

```ts
type RuntimeTokenMapping = {
  "color.background": string;
  "color.surface": string;
  "color.surfaceStrong": string;
  "color.text": string;
  "color.muted": string;
  "color.primary": string;
  "color.primaryContrast": string;
  "color.border": string;
  "radius.card": string;
  "radius.control": string;
  "font.sans": string;
  "shadow.soft": string;
};
```

Agent 修改 token 时必须通过 `style.update_tokens`，不要直接 patch token CSS，除非是在修复损坏的 style contract。

### 6.2 Token Apply 策略

`runtimeTokenMapping` 不是每次 run 都强制写入项目。否则 edit run 会把用户或上一次生成已经调整过的样式覆盖掉。

MVP 规则：

```text
first build after project.init
  -> may initialize runtime style contract tokens from profile

edit run
  -> read profile as constraint
  -> do not auto-apply runtimeTokenMapping
  -> only call style.update_tokens when:
       user explicitly requests style/profile application, or
       profile version changed and request asks to sync profile, or
       repair fixes a broken/missing style contract

review run
  -> compare current tokens against profile
  -> report drift; do not mutate
```

每次 token application 必须记录：

- profile id；
- profile version/hash；
- changed token names；
- previous value；
- new value；
- reason: `initial_build` | `explicit_apply` | `profile_sync` | `repair`.

## 7. Components

```ts
type ComponentRules = {
  primitives: {
    button: ComponentGuideline;
    input: ComponentGuideline;
    card: ComponentGuideline;
    badge: ComponentGuideline;
    table?: ComponentGuideline;
    tabs?: ComponentGuideline;
    sidebar?: ComponentGuideline;
    navbar?: ComponentGuideline;
  };
  patterns: {
    hero?: PatternGuideline;
    featureGrid?: PatternGuideline;
    timeline?: PatternGuideline;
    previewPanel?: PatternGuideline;
    docsPage?: PatternGuideline;
    settingsPanel?: PatternGuideline;
    emptyState?: PatternGuideline;
  };
};

type ComponentGuideline = {
  role: string;
  anatomy: string[];
  variants: string[];
  usage: string[];
  avoid: string[];
};

type PatternGuideline = {
  purpose: string;
  requiredElements: string[];
  layoutRules: string[];
  contentRules: string[];
  avoid: string[];
};
```

当前 runtime 模板已有基础 Button、tokens、global CSS。DesignProfile 不应要求 agent 一次性生成完整组件库，而应提供组件使用规则，让 agent 在实际页面中保持一致性。

## 8. Content Rules

```ts
type ContentRules = {
  website: {
    requiredSections: string[];
    optionalSections: string[];
    ctaRules: string[];
    proofRules: string[];
  };
  docs: {
    navigationRules: string[];
    pageRules: string[];
    codeBlockRules: string[];
    calloutRules: string[];
  };
  microcopy: {
    emptyStates: string[];
    errors: string[];
    loading: string[];
    confirmations: string[];
  };
};
```

Docs 特别规则：

- 避免生成模板不支持的 MDX 组件，除非已在项目中注册。
- 对 Fumadocs，优先使用普通 Markdown / MDX 基础语法。
- 导航结构应与 `content/docs/meta.json` 一致。

## 9. Accessibility

```ts
type AccessibilityRules = {
  wcag: "AA" | "AAA";
  contrast: string;
  keyboard: string[];
  focus: string[];
  motion: string[];
  images: string[];
  semantics: string[];
};
```

默认要求：

- 文本和背景满足 WCAG AA。
- 所有可交互控件必须有清晰 focus state。
- 不依赖颜色作为唯一状态表达。
- 支持 reduced motion。
- 图片、截图、图标需要有可理解替代文本或语义。

## 10. Technical Rules

```ts
type TechnicalRules = {
  allowedTemplates: Array<"astro-website" | "fumadocs-docs" | "nextjs-website" | "docusaurus-docs">;
  preferredTemplates: {
    website: "astro-website" | "nextjs-website";
    docs: "fumadocs-docs" | "docusaurus-docs";
  };
  cssStrategy: "runtime-style-contract" | "tailwind-css-variables";
  dependencyPolicy: {
    preferExisting: boolean;
    allowNewRuntimeDeps: boolean;
    notes: string[];
  };
  filePolicy: {
    designProfilePath: "/workspace/inputs/design-profile.json";
    designMarkdownPath: "/workspace/inputs/design.md";
    styleContractPath: "/workspace/state/style-contract.json";
  };
};
```

第一阶段建议：

- `cssStrategy = "runtime-style-contract"`。
- 新依赖默认不鼓励。
- Website 使用 `astro-website`。
- Docs 使用 `fumadocs-docs`。

## 11. Governance

```ts
type GovernanceRules = {
  strictness: "advisory" | "preferred" | "required";
  conflictBehavior: "prefer-user" | "ask" | "block";
  versioning: {
    strategy: "replace" | "append-version";
    keepHistory: boolean;
  };
  review: {
    requireVisualReview: boolean;
    checks: string[];
  };
};
```

推荐默认：

```json
{
  "strictness": "preferred",
  "conflictBehavior": "ask",
  "versioning": { "strategy": "append-version", "keepHistory": true },
  "review": {
    "requireVisualReview": false,
    "checks": [
      "token usage matches style contract",
      "preview is nonblank",
      "primary CTA and navigation are visible",
      "content follows brand voice"
    ]
  }
}
```

## 12. Runtime 注入方式

### 12.1 StartRun API 扩展

在现有 `StartRunRequest.inputContext` 增加：

```ts
type StartRunInputContext = {
  contentSources?: ContentSource[];
  briefId?: string;
  baseVersionId?: string;
  sandboxBindingId?: string;
  parentRunId?: string;
  findingIds?: string[];
  designProfileId?: string;
};
```

D1 不能只增加 `designProfileId` 字段，还必须提供 runtime 可解析 profile 的最小能力。否则 `POST /runs` 无法判断 profile 是否存在、是否可见、版本是什么。

D1 最小 runtime API：

```text
POST /design-profiles
GET  /design-profiles/{designProfileId}
POST /projects/{projectId}/design-profile
GET  /projects/{projectId}/design-profile
```

其中 `POST /projects/{projectId}/design-profile` 只负责绑定或替换 project active profile，不要求做完整管理 UI。列表、归档、版本 diff、clone 等仍放到 D2/D4。

### 12.2 Workspace 文件

当 run 绑定了 DesignProfile：

```text
/workspace/inputs/design-profile.json
/workspace/inputs/design.md
```

`design-profile.json` 是完整结构化契约，供工具和后续自动校验使用。

`design.md` 是给模型读的压缩版，建议由 runtime 从 JSON 渲染生成，不由用户手写：

```md
# Design Profile

Name: Runtime Harness Product System
Strictness: preferred

## Product
...

## Brand Voice
...

## Visual Direction
...

## Runtime Tokens
...

## Component Rules
...

## Accessibility
...
```

`design.md` 渲染必须稳定、可预算、可追踪：

- 固定 section 顺序，不随 JSON key 顺序变化。
- 最大 8,000 字符；超出时保留 product、brand voice、visual direction、runtime tokens、governance，压缩 components/content 细节。
- 文件顶部写入 `DesignProfile ID`、`version`、`hash`。
- 只渲染 agent 需要执行的规则，不渲染完整 audit/history。
- `design-profile.json` 是 source of truth，`design.md` 是 derived artifact。
- 如果 JSON 和 markdown hash 不匹配，runtime 应重新生成 `design.md`。

### 12.3 Agent prompt 注入

Build Agent：

- 读取 `brief.md`。
- 读取 `design-profile.json` 和 `design.md`。
- 首次 build 可用 `runtimeTokenMapping` 初始化 tokens；edit run 默认不得自动覆盖现有 tokens。
- 生成页面时遵守 `components`、`content`、`accessibility`。
- 写入 `context.md` 记录实际采用的设计决策。

Edit Agent：

- 读取 `context.md`、`brief.md`、`design-profile.json`、`design.md`。
- 如果用户请求与 DesignProfile 冲突，进入 `needs_user_input:design_profile_conflict`，等待用户选择临时覆盖或更新 profile。
- 样式修改优先调用 `style.update_tokens`。

Review Agent：

- 对照 DesignProfile 检查视觉漂移、内容语气、可访问性、preview 可见性。
- 发现偏离时写 `review.finding`，category 可用 `visual` / `content` / `safety`。

## 13. API 设计

Design Context MVP 只需要最小 runtime API：

```text
POST   /design-profiles
GET    /design-profiles/{designProfileId}
GET    /projects/{projectId}/design-profile
POST   /projects/{projectId}/design-profile
```

请求示例：

```ts
type CreateDesignProfileRequest = {
  projectId?: string;
  name: string;
  profile: Omit<DesignProfile, "id" | "version" | "createdAt" | "updatedAt">;
};
```

响应示例：

```ts
type DesignProfileResponse = {
  designProfile: DesignProfile;
};
```

后续再补：

- PUT；
- list；
- clone；
- archive；
- version history；
- diff；
- import/export。

## 14. 完整示例：Runtime Harness Product System

```json
{
  "id": "design-profile-runtime-harness",
  "name": "Runtime Harness Product System",
  "status": "active",
  "version": 1,
  "scope": {
    "projectId": "runtime-harness"
  },
  "source": {
    "kind": "manual",
    "notes": "Derived from the internal AI Website / Docs generator product docs and current runtime style contract."
  },
  "product": {
    "name": "Runtime Harness",
    "category": "internal AI website and docs generator",
    "audience": [
      "internal designers",
      "frontend engineers",
      "platform engineers",
      "product operators"
    ],
    "primaryUseCases": [
      "turn prompts and markdown into website/docs briefs",
      "generate editable website or docs projects",
      "preview, review, repair, and promote generated artifacts",
      "observe live runtime lifecycle events"
    ],
    "productQualities": [
      "trustworthy",
      "observable",
      "controlled",
      "recoverable",
      "workflow-oriented"
    ]
  },
  "brand": {
    "voice": {
      "tone": ["clear", "calm", "technical", "confident"],
      "sentenceStyle": "balanced",
      "vocabulary": {
        "prefer": [
          "runtime",
          "evidence",
          "preview",
          "lifecycle",
          "repair",
          "promote",
          "contract",
          "observable"
        ],
        "avoid": [
          "magic",
          "instant perfection",
          "revolutionary",
          "fully autonomous",
          "pixel-perfect without review"
        ]
      },
      "writingRules": [
        "Prefer concrete workflow language over broad marketing claims.",
        "Explain what the system verifies, not only what it generates.",
        "Use short labels for operational UI and fuller prose for docs pages.",
        "Do not hide uncertainty; state assumptions and missing information."
      ]
    },
    "messaging": {
      "headlineStyle": "literal product capability or product name first",
      "bodyStyle": "concise, evidence-oriented, grounded in runtime lifecycle",
      "ctaStyle": "action-oriented verbs such as Generate, Review, Promote, Continue",
      "proofStyle": "show preview status, build evidence, event timeline, and source snapshot",
      "forbiddenClaims": [
        "guarantees perfect design",
        "replaces designer review",
        "supports arbitrary production deployment without validation"
      ]
    }
  },
  "visual": {
    "direction": "Quiet technical confidence for an internal operational design tool.",
    "principles": [
      "Make lifecycle state visible.",
      "Prefer dense but readable operational layouts.",
      "Use restrained color with clear semantic accents.",
      "Show real artifacts, previews, timelines, and evidence instead of abstract decoration."
    ],
    "moodKeywords": [
      "precise",
      "structured",
      "modern",
      "calm",
      "inspectable"
    ],
    "avoidKeywords": [
      "overly playful",
      "decorative",
      "one-note purple gradient",
      "stock-like",
      "vague AI sparkle"
    ],
    "composition": {
      "density": "balanced",
      "informationHierarchy": [
        "project/run status",
        "current preview",
        "event timeline",
        "build/review evidence",
        "actions"
      ],
      "layoutPreference": [
        "workbench layout",
        "split timeline and preview",
        "docs sidebar for documentation projects",
        "full-width sections instead of nested cards"
      ],
      "navigation": "side-nav",
      "cardUsage": "moderate",
      "sectionRhythm": "Use clear bands or panels for tool areas; avoid card-in-card nesting."
    },
    "imagery": {
      "allowed": [
        "real preview screenshots",
        "artifact snapshots",
        "runtime event timelines",
        "source/build evidence"
      ],
      "avoid": [
        "abstract AI art",
        "dark blurred hero images",
        "decorative gradient orbs",
        "uninspectable mockups"
      ],
      "productSignal": "The first viewport should show a concrete generated website/docs preview or runtime lifecycle state.",
      "iconStyle": "simple line icons from the app icon set; prefer familiar symbols for actions",
      "screenshotPolicy": "Screenshots must be nonblank and tied to preview evidence."
    },
    "motion": {
      "level": "subtle",
      "allowed": [
        "timeline event arrival",
        "loading/progress state",
        "preview refreshed indication"
      ],
      "avoid": [
        "large decorative animation",
        "motion that hides status",
        "continuous distraction"
      ],
      "reducedMotionBehavior": "Disable nonessential animation and keep state changes visible."
    }
  },
  "tokens": {
    "color": {
      "background": "#f8fafc",
      "surface": "#ffffff",
      "surfaceStrong": "#eef2f7",
      "text": "#0f172a",
      "muted": "#64748b",
      "primary": "#f97316",
      "primaryContrast": "#ffffff",
      "border": "#d8dee8",
      "success": "#16a34a",
      "warning": "#d97706",
      "danger": "#dc2626",
      "info": "#2563eb"
    },
    "typography": {
      "fontSans": "Inter, ui-sans-serif, system-ui, sans-serif",
      "fontMono": "ui-monospace, SFMono-Regular, Menlo, monospace",
      "scale": "default",
      "headingWeight": "700",
      "bodyWeight": "400",
      "lineHeight": "1.55"
    },
    "radius": {
      "card": "8px",
      "control": "6px",
      "pill": "999px"
    },
    "shadow": {
      "soft": "0 12px 32px rgba(15, 23, 42, 0.08)",
      "focus": "0 0 0 3px rgba(249, 115, 22, 0.22)"
    },
    "spacing": {
      "density": "default",
      "pageMaxWidth": "1180px",
      "sectionGap": "48px",
      "controlHeight": "36px"
    }
  },
  "runtimeTokenMapping": {
    "color.background": "#f8fafc",
    "color.surface": "#ffffff",
    "color.surfaceStrong": "#eef2f7",
    "color.text": "#0f172a",
    "color.muted": "#64748b",
    "color.primary": "#f97316",
    "color.primaryContrast": "#ffffff",
    "color.border": "#d8dee8",
    "radius.card": "8px",
    "radius.control": "6px",
    "font.sans": "Inter, ui-sans-serif, system-ui, sans-serif",
    "shadow.soft": "0 12px 32px rgba(15, 23, 42, 0.08)"
  },
  "components": {
    "primitives": {
      "button": {
        "role": "primary commands and scoped secondary actions",
        "anatomy": ["icon", "label", "optional loading state"],
        "variants": ["primary", "secondary", "ghost", "danger"],
        "usage": [
          "Use primary for the next lifecycle action.",
          "Use icon-only buttons for familiar tool actions when space is constrained.",
          "Keep labels short and action-oriented."
        ],
        "avoid": [
          "oversized marketing buttons inside dense workbench panels",
          "ambiguous verbs like Submit when a more specific command exists"
        ]
      },
      "input": {
        "role": "structured project/run configuration",
        "anatomy": ["label", "control", "helper or validation text"],
        "variants": ["text", "textarea", "select", "toggle"],
        "usage": [
          "Use explicit labels.",
          "Prefer selects for known option sets.",
          "Show validation near the control."
        ],
        "avoid": ["placeholder-only labels", "hidden required fields"]
      },
      "card": {
        "role": "repeated entities such as versions, findings, or evidence",
        "anatomy": ["title", "metadata", "status", "actions"],
        "variants": ["default", "selected", "warning"],
        "usage": [
          "Use cards for repeated items only.",
          "Keep radius at 8px or less.",
          "Do not nest cards inside cards."
        ],
        "avoid": ["page sections styled as floating cards", "pure decoration"]
      },
      "badge": {
        "role": "status and category labels",
        "anatomy": ["status color", "short label"],
        "variants": ["queued", "running", "blocked", "completed", "failed"],
        "usage": ["Use semantic status names from runtime events."],
        "avoid": ["using color alone without text"]
      }
    },
    "patterns": {
      "hero": {
        "purpose": "Introduce generated artifact or runtime capability",
        "requiredElements": ["product name or literal offer", "supporting copy", "primary action"],
        "layoutRules": [
          "For product pages, make the product visible in the first viewport.",
          "Do not put hero text inside a card.",
          "Leave a hint of the next section visible."
        ],
        "contentRules": [
          "Headline should be product name or literal category.",
          "Supporting copy can describe value and evidence."
        ],
        "avoid": ["split text/media card hero", "gradient-only hero"]
      },
      "timeline": {
        "purpose": "Show live runtime progress",
        "requiredElements": ["event type", "timestamp", "summary", "status"],
        "layoutRules": ["Newest meaningful state should be easy to scan."],
        "contentRules": ["Collapse noisy tool output but keep failures visible."],
        "avoid": ["hiding recoverable errors", "unbounded auto-scrolling without user control"]
      },
      "previewPanel": {
        "purpose": "Inspect promoted or candidate artifact",
        "requiredElements": ["preview url", "version id", "screenshot/evidence state"],
        "layoutRules": ["Preview should be large enough to inspect real output."],
        "contentRules": ["Only switch current preview after preview.updated."],
        "avoid": ["switching on build start", "showing blank preview as success"]
      },
      "docsPage": {
        "purpose": "Readable technical documentation",
        "requiredElements": ["title", "description", "body", "navigation"],
        "layoutRules": ["Use docs sidebar and readable content width."],
        "contentRules": ["Prefer plain markdown, headings, lists, and code blocks."],
        "avoid": ["unregistered MDX components", "marketing layout inside docs body"]
      }
    }
  },
  "content": {
    "website": {
      "requiredSections": [
        "hero",
        "runtime lifecycle",
        "preview promotion",
        "repair and recovery",
        "evidence or audit"
      ],
      "optionalSections": ["integrations", "security", "FAQ"],
      "ctaRules": [
        "Use one primary CTA per viewport band.",
        "CTA labels should map to runtime actions when possible."
      ],
      "proofRules": [
        "Show build status, preview version, or event timeline as proof.",
        "Avoid generic customer-logo filler."
      ]
    },
    "docs": {
      "navigationRules": [
        "Keep overview first.",
        "Group guides, recipes, and advanced topics separately.",
        "Keep labels short."
      ],
      "pageRules": [
        "Start with what the user can do.",
        "Use concrete steps.",
        "Call out runtime constraints and recovery behavior."
      ],
      "codeBlockRules": [
        "Use runnable commands from repo scripts.",
        "Avoid placeholder secrets in examples."
      ],
      "calloutRules": [
        "Use simple blockquotes or supported components only.",
        "Do not invent MDX components."
      ]
    },
    "microcopy": {
      "emptyStates": [
        "No events yet. The run will stream updates as soon as it starts.",
        "No promoted preview yet. Build a candidate before promoting."
      ],
      "errors": [
        "The run needs input before it can continue.",
        "The preview is not ready yet. Check build evidence."
      ],
      "loading": [
        "Preparing sandbox",
        "Generating project",
        "Building preview",
        "Waiting for runtime events"
      ],
      "confirmations": [
        "Brief confirmed",
        "Preview promoted",
        "Run completed"
      ]
    }
  },
  "accessibility": {
    "wcag": "AA",
    "contrast": "Text/background and primary controls must meet AA contrast.",
    "keyboard": [
      "All controls reachable by keyboard.",
      "Focus order follows visual order."
    ],
    "focus": [
      "Visible focus ring on interactive elements.",
      "Do not remove outlines without replacement."
    ],
    "motion": [
      "Respect prefers-reduced-motion.",
      "Keep live event updates understandable without animation."
    ],
    "images": [
      "Generated screenshots need descriptive alt text.",
      "Decorative images should be ignored by assistive tech."
    ],
    "semantics": [
      "Use headings in order.",
      "Use buttons for actions and links for navigation."
    ]
  },
  "technical": {
    "allowedTemplates": ["astro-website", "fumadocs-docs"],
    "preferredTemplates": {
      "website": "astro-website",
      "docs": "fumadocs-docs"
    },
    "cssStrategy": "runtime-style-contract",
    "dependencyPolicy": {
      "preferExisting": true,
      "allowNewRuntimeDeps": false,
      "notes": [
        "Use template-provided Tailwind/CSS variable setup.",
        "Avoid adding UI libraries unless the brief requires them."
      ]
    },
    "filePolicy": {
      "designProfilePath": "/workspace/inputs/design-profile.json",
      "designMarkdownPath": "/workspace/inputs/design.md",
      "styleContractPath": "/workspace/state/style-contract.json"
    }
  },
  "governance": {
    "strictness": "preferred",
    "conflictBehavior": "ask",
    "versioning": {
      "strategy": "append-version",
      "keepHistory": true
    },
    "review": {
      "requireVisualReview": false,
      "checks": [
        "token usage matches style contract",
        "preview is nonblank",
        "primary CTA and navigation are visible",
        "content follows brand voice",
        "docs avoid unsupported MDX components"
      ]
    }
  },
  "createdAt": "2026-07-09T00:00:00.000Z",
  "updatedAt": "2026-07-09T00:00:00.000Z"
}
```

### 14.1 外部风格参考整理为 DesignProfile Presets

下面两个 preset 来自本地参考文档：

- `/Users/carlos/Downloads/DESIGN (1).md`
- `/Users/carlos/Downloads/DESIGN (2).md`

它们不是 runtime 默认 profile，而是可作为产品侧创建 DesignProfile 时的示例输入。整理原则：

- 保留能直接影响生成结果的品牌、视觉、token、组件和内容规则。
- 将原始 token 降级映射到当前 runtime style contract 的 12 个可编辑 token。
- 将品牌特有的“Do / Don't”整理到 `visual.principles`、`visual.avoidKeywords`、`components`、`content`、`governance.review.checks`。
- 不把外部参考当成通用默认值；它们应由用户显式选择或导入。

#### 14.1.1 AuthKit Frosted Glass Cathedral

适用场景：

- 暗色、高质感、产品发布页。
- 登录、认证、安全、开发者工具类网站。
- 需要强 hero 视觉、玻璃层、发光边缘和单一紫色 CTA 的界面。

```json
{
  "id": "design-profile-authkit-frosted-glass",
  "name": "AuthKit Frosted Glass Cathedral",
  "status": "active",
  "version": 1,
  "scope": {
    "projectId": "authkit-style-reference"
  },
  "source": {
    "kind": "imported",
    "sourceIds": ["DESIGN (1).md"],
    "notes": "Derived from AuthKit style reference: dark frosted glass, midnight canvas, luminous text, single violet CTA."
  },
  "product": {
    "name": "AuthKit Style Reference",
    "category": "dark product-launch website",
    "audience": ["product designers", "frontend engineers", "developer-tool marketers"],
    "primaryUseCases": [
      "generate dark authentication product landing pages",
      "create frosted glass login and signup mockups",
      "produce security/developer-tool websites with premium launch aesthetics"
    ],
    "productQualities": ["luminous", "precise", "premium", "focused", "atmospheric"]
  },
  "brand": {
    "voice": {
      "tone": ["calm", "premium", "technical", "minimal"],
      "sentenceStyle": "balanced",
      "vocabulary": {
        "prefer": ["frosted", "secure", "glass", "midnight", "auth", "identity", "workspace"],
        "avoid": ["playful", "busy", "cartoon", "consumer lifestyle", "rainbow"]
      },
      "writingRules": [
        "Use sparse product-launch copy with short, confident claims.",
        "Let the visual system carry atmosphere; keep body text restrained.",
        "Use concrete auth product language for forms, providers, SSO, MFA, RBAC, and passwordless flows."
      ]
    },
    "messaging": {
      "headlineStyle": "single memorable product name or capability with luminous display treatment",
      "bodyStyle": "short technical support copy in cool muted text",
      "ctaStyle": "one vivid violet submit or continue action in auth contexts",
      "proofStyle": "show auth form states, provider buttons, theme controls, and customization panels",
      "forbiddenClaims": ["passwordless solves all security", "perfect security by default", "replaces identity review"]
    }
  },
  "visual": {
    "direction": "Frosted glass cathedral at midnight: a near-black canvas with translucent glass layers, cool blueprint grid lines, luminous text, and one vivid violet CTA.",
    "principles": [
      "Use a near-black full-bleed canvas as the base.",
      "Build surfaces as translucent glass plates with inset frost highlights.",
      "Keep the palette monochrome blue-white plus one functional violet action.",
      "Use generous cathedral-like spacing and centered section rhythm.",
      "Use auth-form mockups, provider buttons, and customization controls as the product signal."
    ],
    "moodKeywords": ["midnight", "frosted", "luminous", "premium", "glass", "blueprint", "quiet"],
    "avoidKeywords": [
      "extra chromatic accents",
      "solid borders",
      "conventional drop shadows",
      "heavy display weights",
      "light theme as primary",
      "photography",
      "lifestyle imagery"
    ],
    "composition": {
      "density": "spacious",
      "informationHierarchy": ["hero wordmark", "floating auth cards", "theme toggle", "feature icons", "customization workspace"],
      "layoutPreference": ["full-bleed dark canvas", "centered hero", "floating glass card fan", "wide section gaps"],
      "navigation": "top-nav",
      "cardUsage": "moderate",
      "sectionRhythm": "Every section opens with centered all-caps eyebrow label, fading divider lines, large luminous heading, and short muted copy."
    },
    "imagery": {
      "allowed": ["glass auth-form mockups", "line-art auth icons", "blueprint grid", "conic spotlight halo", "customization swatches"],
      "avoid": ["photography", "lifestyle images", "stock illustrations", "multi-color decorative art"],
      "productSignal": "The first viewport should show the AuthKit-style wordmark and floating auth form cards.",
      "iconStyle": "mono line-art icons in cool frost foreground inside circular frosted tiles",
      "screenshotPolicy": "Use generated UI mockups rather than external screenshots."
    },
    "motion": {
      "level": "subtle",
      "allowed": ["soft card float", "frost hover brightening", "theme toggle transition"],
      "avoid": ["large kinetic hero motion", "sparkle effects", "continuous distraction"],
      "reducedMotionBehavior": "Disable floating motion and keep static glass layers visible."
    }
  },
  "tokens": {
    "color": {
      "background": "#05060f",
      "surface": "rgba(186,214,247,0.03)",
      "surfaceStrong": "rgba(5,6,15,0.97)",
      "text": "#d1e4fa",
      "muted": "#9da7ba",
      "primary": "#663af3",
      "primaryContrast": "#ffffff",
      "border": "rgba(186,215,247,0.12)",
      "info": "#b6d9fc"
    },
    "typography": {
      "fontSans": "Inter, ui-sans-serif, system-ui, sans-serif",
      "fontMono": "JetBrains Mono, ui-monospace, monospace",
      "scale": "editorial",
      "headingWeight": "500",
      "bodyWeight": "400",
      "lineHeight": "1.5"
    },
    "radius": {
      "card": "16px",
      "control": "999px",
      "pill": "999px"
    },
    "shadow": {
      "soft": "inset 0 1px 1px rgba(216,236,248,0.2), inset 0 24px 48px rgba(168,216,245,0.06), 0 16px 32px rgba(0,0,0,0.3)",
      "focus": "0 0 0 1px rgba(186,215,247,0.24)"
    },
    "spacing": {
      "density": "comfortable",
      "pageMaxWidth": "1200px",
      "sectionGap": "120px",
      "controlHeight": "40px"
    }
  },
  "runtimeTokenMapping": {
    "color.background": "#05060f",
    "color.surface": "rgba(186,214,247,0.03)",
    "color.surfaceStrong": "rgba(5,6,15,0.97)",
    "color.text": "#d1e4fa",
    "color.muted": "#9da7ba",
    "color.primary": "#663af3",
    "color.primaryContrast": "#ffffff",
    "color.border": "rgba(186,215,247,0.12)",
    "radius.card": "16px",
    "radius.control": "999px",
    "font.sans": "Inter, ui-sans-serif, system-ui, sans-serif",
    "shadow.soft": "inset 0 1px 1px rgba(216,236,248,0.2), inset 0 24px 48px rgba(168,216,245,0.06), 0 16px 32px rgba(0,0,0,0.3)"
  },
  "components": {
    "primitives": {
      "button": {
        "role": "pill-shaped navigation and auth actions",
        "anatomy": ["label", "optional provider icon", "frosted inset edge"],
        "variants": ["primary-ghost", "outlined", "violet-auth-submit"],
        "usage": [
          "Use pill buttons for navigation and secondary actions.",
          "Use violet filled CTA only for auth submit/continue actions.",
          "Hover should brighten the frost wash, not introduce new colors."
        ],
        "avoid": ["square buttons", "extra colored CTAs", "solid heavy borders"]
      },
      "input": {
        "role": "auth form fields",
        "anatomy": ["label", "field", "placeholder", "focus edge"],
        "variants": ["email", "password", "text"],
        "usage": ["Use 6px radius and translucent cool-white fill.", "Increase inset border opacity on focus."],
        "avoid": ["plain white fields", "large colored focus rings"]
      },
      "card": {
        "role": "floating glass feature and auth-form surfaces",
        "anatomy": ["frosted surface", "inset highlight", "soft halo", "content"],
        "variants": ["feature-glass", "auth-modal", "floating-hero-card"],
        "usage": ["Use 16px radius and layered inset highlights.", "Keep surfaces translucent or midnight-opaque."],
        "avoid": ["paper cards", "flat white panels", "conventional shadow-only elevation"]
      },
      "badge": {
        "role": "integration and capability tags",
        "anatomy": ["short label", "soft luminous fill"],
        "variants": ["default", "integration"],
        "usage": ["Use 6px radius and cool-blue translucent background."],
        "avoid": ["bright solid badges", "multi-color tag systems"]
      }
    },
    "patterns": {
      "hero": {
        "purpose": "Create a premium auth product launch first viewport",
        "requiredElements": ["eyebrow", "large illuminated wordmark", "floating auth form cards", "spotlight halo"],
        "layoutRules": ["Center the hero.", "Use overlapping glass cards below or behind the wordmark.", "Keep the background full-bleed midnight."],
        "contentRules": ["Use short product name or capability as headline.", "Keep supporting copy under two lines."],
        "avoid": ["split card/text hero", "photographic hero", "gradient-only hero without product UI"]
      },
      "featureGrid": {
        "purpose": "Show auth capabilities",
        "requiredElements": ["six mono line icons", "circular frosted icon tiles", "thin connector lines"],
        "layoutRules": ["Use horizontal timeline-like feature row when space allows."],
        "contentRules": ["Labels should be concrete auth capabilities such as SSO, MFA, RBAC, passwordless."],
        "avoid": ["colorful icon sets", "dense card grids"]
      }
    }
  },
  "content": {
    "website": {
      "requiredSections": ["hero", "auth form showcase", "capability row", "customization workspace", "theme support"],
      "optionalSections": ["integrations", "developer notes"],
      "ctaRules": ["Use one violet submit/continue CTA inside auth forms.", "Use ghost/outlined pills outside auth forms."],
      "proofRules": ["Show login/signup form states and provider buttons instead of abstract claims."]
    },
    "docs": {
      "navigationRules": ["Use dark docs only when the selected project is explicitly docs."],
      "pageRules": ["Keep docs readable; do not over-apply hero glass treatments inside docs content."],
      "codeBlockRules": ["Use dark code blocks that match midnight canvas."],
      "calloutRules": ["Use frosted subtle callouts, not bright alert boxes."]
    },
    "microcopy": {
      "emptyStates": ["No providers connected yet.", "Add your first auth method."],
      "errors": ["Check the identity provider configuration."],
      "loading": ["Preparing secure session"],
      "confirmations": ["Authentication method enabled"]
    }
  },
  "accessibility": {
    "wcag": "AA",
    "contrast": "Luminous text must maintain AA contrast on midnight and glass surfaces.",
    "keyboard": ["All auth form controls reachable by keyboard."],
    "focus": ["Focus uses brighter frosted inset edge plus visible outline where needed."],
    "motion": ["Respect reduced motion for floating cards."],
    "images": ["Auth form mockups need semantic labels when rendered as images."],
    "semantics": ["Use real form semantics for auth UI."]
  },
  "technical": {
    "allowedTemplates": ["astro-website", "fumadocs-docs"],
    "preferredTemplates": {
      "website": "astro-website",
      "docs": "fumadocs-docs"
    },
    "cssStrategy": "runtime-style-contract",
    "dependencyPolicy": {
      "preferExisting": true,
      "allowNewRuntimeDeps": false,
      "notes": ["Prefer CSS variables and template components over adding a glassmorphism UI library."]
    },
    "filePolicy": {
      "designProfilePath": "/workspace/inputs/design-profile.json",
      "designMarkdownPath": "/workspace/inputs/design.md",
      "styleContractPath": "/workspace/state/style-contract.json"
    }
  },
  "governance": {
    "strictness": "preferred",
    "conflictBehavior": "ask",
    "versioning": {
      "strategy": "append-version",
      "keepHistory": true
    },
    "review": {
      "requireVisualReview": true,
      "checks": [
        "dark canvas is near-black",
        "violet is used only for primary auth submit actions",
        "glass surfaces use inset frost edges",
        "hero shows concrete auth-form product signal",
        "no photography or extra chromatic accents"
      ]
    }
  },
  "createdAt": "2026-07-09T00:00:00.000Z",
  "updatedAt": "2026-07-09T00:00:00.000Z"
}
```

#### 14.1.2 ElevenLabs Warm Editorial

适用场景：

- 暖白、编辑感、AI/voice/product 平台网站。
- 需要克制品牌表达、强排版、少量产品视觉火花的产品页。
- 适合信任、案例、产品能力、AI agent / voice agent 叙事。

```json
{
  "id": "design-profile-elevenlabs-warm-editorial",
  "name": "ElevenLabs Warm Editorial",
  "status": "active",
  "version": 1,
  "scope": {
    "projectId": "elevenlabs-style-reference"
  },
  "source": {
    "kind": "imported",
    "sourceIds": ["DESIGN (2).md"],
    "notes": "Derived from ElevenLabs style reference: warm cream editorial, black ink, taupe surfaces, violet/orange accents reserved for product visuals."
  },
  "product": {
    "name": "ElevenLabs Style Reference",
    "category": "warm editorial AI product website",
    "audience": ["AI product teams", "creative tooling teams", "platform marketers", "frontend engineers"],
    "primaryUseCases": [
      "generate warm editorial AI platform pages",
      "build voice/audio product showcase sections",
      "create restrained product marketing pages with strong typography"
    ],
    "productQualities": ["editorial", "restrained", "warm", "precise", "confident"]
  },
  "brand": {
    "voice": {
      "tone": ["quiet", "confident", "editorial", "technical"],
      "sentenceStyle": "editorial",
      "vocabulary": {
        "prefer": ["voice", "agent", "studio", "creative", "platform", "audio", "research"],
        "avoid": ["loud", "flashy", "neon UI", "over-designed", "hype"]
      },
      "writingRules": [
        "Use calm editorial headlines with strong typographic restraint.",
        "Prefer proof through product visuals, partner logos, and capability sections.",
        "Keep accent color out of UI chrome; reserve it for product visuals."
      ]
    },
    "messaging": {
      "headlineStyle": "short editorial statement in whisper-weight display type",
      "bodyStyle": "warm neutral body copy with relaxed line-height",
      "ctaStyle": "black filled pill paired with off-white outline pill",
      "proofStyle": "trust logos, product visuals, and quiet feature panels",
      "forbiddenClaims": ["human voice indistinguishable in every case", "fully replaces creative teams", "guaranteed production safety"]
    }
  },
  "visual": {
    "direction": "Warm cream editorial system: eggshell paper canvas, black ink typography, taupe surfaces, pill buttons, hairline borders, and violet/orange sparks only inside product visuals.",
    "principles": [
      "Use warm off-white instead of pure white.",
      "Let whisper-weight display typography carry the brand.",
      "Reserve violet and orange for product visuals, not UI controls.",
      "Use flat taupe feature cards and 1px warm hairline dividers.",
      "Keep layout editorial, spacious, and vertically flowing."
    ],
    "moodKeywords": ["warm", "editorial", "paper", "Bauhaus", "restrained", "precise", "quiet"],
    "avoidKeywords": [
      "dark glass",
      "heavy shadow",
      "colored UI buttons",
      "sharp card corners",
      "pure white background",
      "bold display headlines",
      "extra accent colors"
    ],
    "composition": {
      "density": "spacious",
      "informationHierarchy": ["editorial headline", "short body copy", "pill CTAs", "large product visual", "trust evidence"],
      "layoutPreference": ["single centered max-width column", "asymmetric hero", "full-width feature panels", "taupe section bands"],
      "navigation": "top-nav",
      "cardUsage": "minimal",
      "sectionRhythm": "Generous whitespace with 96-125px vertical gaps; prefer one major visual per section over dense card grids."
    },
    "imagery": {
      "allowed": ["audio sphere gradients", "monochrome product icons", "grayscale trust logos", "warm editorial panels"],
      "avoid": ["lifestyle photography", "colorful icon sets", "heavy 3D scenes", "dark blurred heroes"],
      "productSignal": "The first viewport should pair editorial headline typography with product/voice visual evidence.",
      "iconStyle": "sparse monochrome black icons; chromatic accents only inside product artwork",
      "screenshotPolicy": "Use product visuals and logo grids over generic screenshots unless the brief requests UI evidence."
    },
    "motion": {
      "level": "subtle",
      "allowed": ["audio sphere shimmer", "tab switch", "gentle product visual transition"],
      "avoid": ["large decorative motion", "bouncy UI", "continuous background animation"],
      "reducedMotionBehavior": "Keep product visuals static and preserve editorial layout."
    }
  },
  "tokens": {
    "color": {
      "background": "#fdfcfc",
      "surface": "#f5f3f1",
      "surfaceStrong": "#ebe8e4",
      "text": "#000000",
      "muted": "#777169",
      "primary": "#000000",
      "primaryContrast": "#ffffff",
      "border": "#ebe8e4",
      "info": "#0447ff",
      "warning": "#ff4704"
    },
    "typography": {
      "fontSans": "Inter, ui-sans-serif, system-ui, sans-serif",
      "fontMono": "Geist Mono, JetBrains Mono, ui-monospace, monospace",
      "scale": "editorial",
      "headingWeight": "300",
      "bodyWeight": "400",
      "lineHeight": "1.5"
    },
    "radius": {
      "card": "20px",
      "control": "9999px",
      "pill": "9999px"
    },
    "shadow": {
      "soft": "0 0 1px rgba(0,0,0,0.4), 0 1px 1px rgba(0,0,0,0.04), 0 2px 4px rgba(0,0,0,0.04)",
      "focus": "0 0 0 1px rgba(0,0,0,0.1)"
    },
    "spacing": {
      "density": "comfortable",
      "pageMaxWidth": "1280px",
      "sectionGap": "96px",
      "controlHeight": "40px"
    }
  },
  "runtimeTokenMapping": {
    "color.background": "#fdfcfc",
    "color.surface": "#f5f3f1",
    "color.surfaceStrong": "#ebe8e4",
    "color.text": "#000000",
    "color.muted": "#777169",
    "color.primary": "#000000",
    "color.primaryContrast": "#ffffff",
    "color.border": "#ebe8e4",
    "radius.card": "20px",
    "radius.control": "9999px",
    "font.sans": "Inter, ui-sans-serif, system-ui, sans-serif",
    "shadow.soft": "0 0 1px rgba(0,0,0,0.4), 0 1px 1px rgba(0,0,0,0.04), 0 2px 4px rgba(0,0,0,0.04)"
  },
  "components": {
    "primitives": {
      "button": {
        "role": "primary, secondary, and tertiary pill actions",
        "anatomy": ["label", "pill shell", "hairline border"],
        "variants": ["filled-pill", "outline-pill", "ghost-link-pill"],
        "usage": [
          "Use black filled pills for primary actions.",
          "Pair with off-white outline pills for secondary actions.",
          "Use Inter 14px/500 for button labels."
        ],
        "avoid": ["colored CTA fills", "square buttons", "large icon-heavy buttons"]
      },
      "input": {
        "role": "minimal form controls",
        "anatomy": ["label", "control", "hairline border"],
        "variants": ["text", "email", "search"],
        "usage": ["Use warm surfaces and restrained 1px borders.", "Keep input corners small when used inside editorial forms."],
        "avoid": ["bright focus colors", "heavy inset shadows"]
      },
      "card": {
        "role": "taupe feature and editorial content panels",
        "anatomy": ["surface", "heading", "body", "optional product visual"],
        "variants": ["taupe-feature", "large-feature", "white-whisper-card"],
        "usage": ["Use 20-24px radii and flat warm taupe surfaces.", "Prefer one major product visual per section."],
        "avoid": ["heavy elevation", "sharp corners", "dense card grids"]
      },
      "badge": {
        "role": "tags and product switcher labels",
        "anatomy": ["short label", "pill outline"],
        "variants": ["tag", "tab-pill"],
        "usage": ["Use 9999px radius with warm hairline border.", "Active tab may use a tiny colored dot tied to product visual category."],
        "avoid": ["filled colored badges", "large status chips"]
      }
    },
    "patterns": {
      "hero": {
        "purpose": "Open with editorial authority and product evidence",
        "requiredElements": ["left-aligned display headline", "right or adjacent body copy", "pill CTAs", "audio/product visual"],
        "layoutRules": ["Use asymmetric composition on wide screens.", "Keep canvas eggshell and whitespace generous."],
        "contentRules": ["Headline should be short, calm, and literal enough to understand."],
        "avoid": ["marketing-card hero", "gradient-only hero", "colored CTA hero"]
      },
      "featureGrid": {
        "purpose": "Show product capabilities without visual noise",
        "requiredElements": ["large taupe feature panel", "tab pills", "one product visual"],
        "layoutRules": ["Prefer one large panel over many small cards."],
        "contentRules": ["Use concise feature labels and proof-oriented copy."],
        "avoid": ["multi-color icon grids", "heavy shadows"]
      }
    }
  },
  "content": {
    "website": {
      "requiredSections": ["hero", "product showcase", "feature panel", "trust logo grid", "use cases"],
      "optionalSections": ["stories", "research", "API", "pricing"],
      "ctaRules": ["Use black filled pill for the primary action.", "Use off-white outline pill for secondary actions.", "Never use violet or orange as CTA fill."],
      "proofRules": ["Use grayscale trust logos and product visuals rather than generic social proof filler."]
    },
    "docs": {
      "navigationRules": ["Use simple top or docs navigation without decorative color."],
      "pageRules": ["Keep documentation warmer and more typographic than dashboard-like."],
      "codeBlockRules": ["Use mono labels sparingly and keep code readable on warm surfaces."],
      "calloutRules": ["Use warm taupe callouts with hairline border."]
    },
    "microcopy": {
      "emptyStates": ["No voices yet.", "Create your first agent."],
      "errors": ["Review the voice configuration and try again."],
      "loading": ["Preparing voice preview"],
      "confirmations": ["Voice preview generated", "Agent created"]
    }
  },
  "accessibility": {
    "wcag": "AA",
    "contrast": "Black and graphite text must maintain AA contrast against eggshell and taupe surfaces.",
    "keyboard": ["All pill buttons and tabs reachable by keyboard."],
    "focus": ["Focus should use warm hairline outline, not bright accent fills."],
    "motion": ["Respect reduced motion for audio sphere shimmer."],
    "images": ["Product visuals and audio spheres need descriptive alt text."],
    "semantics": ["Trust logos should be grouped semantically and not replace textual proof."]
  },
  "technical": {
    "allowedTemplates": ["astro-website", "fumadocs-docs"],
    "preferredTemplates": {
      "website": "astro-website",
      "docs": "fumadocs-docs"
    },
    "cssStrategy": "runtime-style-contract",
    "dependencyPolicy": {
      "preferExisting": true,
      "allowNewRuntimeDeps": false,
      "notes": ["Prefer CSS gradients for audio spheres and simple CSS variables for editorial surfaces."]
    },
    "filePolicy": {
      "designProfilePath": "/workspace/inputs/design-profile.json",
      "designMarkdownPath": "/workspace/inputs/design.md",
      "styleContractPath": "/workspace/state/style-contract.json"
    }
  },
  "governance": {
    "strictness": "preferred",
    "conflictBehavior": "ask",
    "versioning": {
      "strategy": "append-version",
      "keepHistory": true
    },
    "review": {
      "requireVisualReview": true,
      "checks": [
        "background is warm eggshell, not pure white",
        "violet and orange are used only in product visuals",
        "primary CTA is black filled pill",
        "display headlines remain light-weight and editorial",
        "cards use flat taupe surfaces with minimal shadow"
      ]
    }
  },
  "createdAt": "2026-07-09T00:00:00.000Z",
  "updatedAt": "2026-07-09T00:00:00.000Z"
}
```

## 15. Validation Rules

MVP 必须校验：

- `name` 非空。
- 至少一个 `scope` 字段存在。
- `product.name`、`product.category`、`visual.direction` 非空。
- `runtimeTokenMapping` 必须包含当前 runtime style contract 的 12 个 token。
- 所有 `runtimeTokenMapping` 值必须复用 runtime `style.update_tokens` 的安全 CSS value validator：禁止空值、分号、花括号、换行和超长值。
- color token 应进一步限制为 hex/rgb/hsl/oklch/color var 等安全颜色表达；radius/shadow/font token 后续再做类型化 lint。
- `technical.allowedTemplates` 至少包含一个 runtime 已支持模板。
- `governance.conflictBehavior` 不得默认为静默覆盖。
- scope 解析后必须唯一；显式 `designProfileId` 不存在或不可见时不得 fallback。
- `design.md` 必须能由 `design-profile.json` 稳定重新生成。

建议后续校验：

- contrast 自动检查；
- token semantic lint；
- component rules completeness；
- docs MDX component allowlist。

## 16. 实施切片

### Phase D0: Spec Only

- 保存本文档。
- 对齐产品和 runtime 约束。

### Phase D1: Runtime Design Context MVP

- shared 增加 `DesignProfileSchema`。
- `StartRunInputContext` 增加 `designProfileId`。
- RuntimeStore 增加 design profile 存取、project active profile 绑定、scope fallback 解析。
- 增加 D1 runtime API：
  - `POST /design-profiles`
  - `GET /design-profiles/{id}`
  - `POST /projects/{projectId}/design-profile`
  - `GET /projects/{projectId}/design-profile`
- Build/Edit run bootstrap 写入：
  - `/workspace/inputs/design-profile.json`
  - `/workspace/inputs/design.md`
- Agent prompt 明确读取 DesignProfile。
- 首次 build 可初始化 tokens；edit run 默认不自动 apply profile tokens。
- 冲突时进入 `needs_user_input:design_profile_conflict`，并阻止 sandbox mutation。
- 测试 build/edit 能读取 profile。

### Phase D2: Minimal Product CRUD UI

- 产品 UI 用 D1 runtime API 创建、读取、绑定 active profile。
- `PUT /design-profiles/{id}` 用于编辑 profile。
- 列表页只展示当前 project active profile 和可选 workspace/org defaults。
- 产品 UI 只做基础表单，不做复杂设计系统管理。

### Phase D3: Review and Drift Detection

- Review Agent 按 DesignProfile 生成 visual/content findings。
- Edit Agent 发现冲突时进入 `needs_user_input`。
- `context.md` 记录本次采用/偏离的 profile 决策。

### Phase D4: Advanced Design System Management

- token editor；
- component inventory；
- profile version diff；
- import/export；
- Figma/design-system integration。

## 17. Done Criteria

DesignProfile MVP 完成标准：

- 可以创建并读取一个 active DesignProfile。
- 可以把 active DesignProfile 绑定到 project。
- StartRun 可携带 `designProfileId`。
- 未显式传 `designProfileId` 时，可按 project -> workspace -> organization fallback 解析。
- Build/Edit sandbox 中出现 `design-profile.json` 和 `design.md`。
- `design.md` 包含 profile id/version/hash，并可由 JSON 稳定重建。
- Build Agent 生成的 style tokens 能映射到 runtime style contract。
- 首次 build 可初始化 tokens；edit run 不会无请求地重置 tokens。
- Edit Agent 修改颜色/字体/半径时优先使用 `style.update_tokens`。
- 如果 profile 不存在，run 启动失败并返回明确错误。
- 如果 Brief 和 DesignProfile 冲突，系统进入 `needs_user_input:design_profile_conflict`，并且不启动会修改 sandbox 的 agent session。
- Website 和 Docs 各有一条测试覆盖。

## 18. 推荐下一步

先实现 Phase D1，不急着做完整 CRUD UI。

推荐 commit 切法：

```text
feat(shared): add design profile contract
feat(runtime): inject design profiles into run context
test(runtime): cover design profile build and edit context
```

产品 UI 的 CRUD 可以等 D1 通过 provider gate 后再做。
