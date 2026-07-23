# zeronDesign Provider 与模型服务简化优化方案

> 日期：2026-07-22
> 状态：产品与架构实施稿（按最新代码复审结果收敛）
> 适用范围：Web、BFF、Runtime、Provider Gateway
> 目标：管理员接入 Provider 并发布可用模型；用户为 Build、Edit、Repair 选择模型；平台按 Run 记录已知 Token 用量。

## 1. 背景与问题

当前 Provider Gateway 已具备 Model Resource revision、Model Selection Policy、显式模型 allowlist、自动选择、执行快照、配额、熔断、幂等和审计等基础能力。最新 Runtime 和 Shared Contract 也已经支持在创建 Run 时传入 `inputContext.modelResourceId`，并把显式模型选择传递到 Gateway。

当前真正缺少的是完整的产品闭环：

1. 管理员不能以清晰的 Provider Connection、Model Service 方式维护模型；
2. 普通用户没有可用模型列表；
3. Web/BFF 创建 Build、Edit 时没有接收和传递模型选择；
4. Edit 没有默认继承基准版本所用模型；
5. Run 已有 Token 事件和聚合能力，但缺少面向用户的稳定口径与幂等去重；
6. Gateway 配置缓存在单个 Pod 内，管理变更不能可靠同步到其他 Pod。

本方案优先补齐这些缺口，不重建已经存在的模型路由和 Run 执行体系。

## 2. 已确认的用户行为

### 2.1 管理员

1. 配置一条可调用的模型 Provider 连接；
2. 在 Provider 下配置一个或多个模型；
3. 选择哪些模型发布给普通用户；
4. 可以修改连接、凭证、模型名称、物理模型和运行参数；
5. 可以测试连接、发布、下架或停用模型。

### 2.2 普通用户

1. 创建作品时，从可用模型列表中选择一个模型；
2. 修改作品时，默认使用基准版本最近一次生成所用模型；
3. 修改前可以重新选择另一个可用模型；
4. 用户只看到模型服务，不接触 Provider endpoint、Secret、Policy 或 revision。

### 2.3 平台

1. 每个 Build、Edit、Repair 都是独立的 Runtime Run；
2. Run 保存本次选择的模型服务 ID；
3. 每个成功模型 Turn 保存实际执行快照和 Token usage；
4. Run 详情汇总展示已知 Token 用量；
5. Provider 或模型配置变更不要求滚动升级 Pod。

## 3. 核心设计决策

### 3.1 产品只暴露三个概念

| 概念 | 面向角色 | 说明 |
| --- | --- | --- |
| Provider Connection | 平台管理员 | Provider 类型、Base URL、凭证和启用状态 |
| Model Service | 管理员、普通用户 | 管理员发布、用户选择的逻辑模型 |
| Generation Run | 普通用户、平台 | 现有 Runtime Run，记录模型和 Token |

`Model Selection Policy`、revision、candidate、熔断、并发限制等继续作为内部治理机制，不进入普通用户产品模型。

### 3.2 复用现有执行链路

一期继续使用现有内部字段和路由：

```text
用户 modelServiceId
  -> BFF 映射为 inputContext.modelResourceId
  -> Runtime 保存到 Run.model
  -> Runtime 每个 Turn 发送 routing.modelResourceId
  -> Gateway 完成 Policy、能力和 enabled 校验
  -> Gateway 返回 ModelExecutionSnapshot 和 usage
```

不新增以下机制：

- Run Model Binding 聚合；
- `PendingModelBinding -> Accepted` Saga；
- Binding 创建、查询、撤销 API；
- 每 Turn 携带 binding ID；
- 面向用户的模型 revision 或 generation。

### 3.3 模型 ID 稳定，revision 仅供内部使用

- 普通用户只提交稳定的 `modelServiceId`；
- BFF 与产品 API 使用 `modelServiceId`；
- Runtime 与 Gateway 一期保留内部名称 `modelResourceId`，避免跨服务破坏性重命名；
- revision 只用于管理员乐观锁、配置历史和执行快照；
- 用户不能选择或激活某个 revision；
- 历史 Run 展示自己的执行快照，不读取模型当前名称或当前配置覆盖历史结果。

管理员可以修改 Model Service 的 `providerConnectionId`、`physicalModel`、能力和运行参数。修改产生新的内部 revision，新 Run 使用新配置，历史 Run 保留旧执行快照。产品不要求通过创建“V2 模型服务”表达普通配置变更。

### 3.4 Run 只固定逻辑模型，不引入独立 Binding

一期保证同一个 Run 的所有 Turn 使用同一个 `modelServiceId`。每个 Turn 保存 Gateway 实际返回的 `modelResourceRevision` 和物理模型快照。

一期不承诺管理员在 Run 执行中修改模型配置后，后续 Turn 仍冻结为旧 revision。管理配置变更是低频操作，Run 是短生命周期任务；当前产品需求不足以支撑跨服务 Binding Saga 的成本。

如果未来明确要求“多 Turn Run 必须冻结完整配置”，可以在现有 Run 上增加可空的 `pinnedModelResourceRevision`：第一次 Gateway 调用解析并返回 revision，Runtime 保存后在后续 Turn 携带该 revision。仍不需要新建 Binding 资源。

### 3.5 Policy 保留为内部安全机制

一期不删除现有 Model Selection Policy，因为它仍承担：

- 显式模型 allowlist；
- Workspace、Project、phase 和 agent profile 的授权边界；
- 项目并发限制；
- 每日输入 Token 安全限额；
- 自动选模兼容路径。

产品层以 `Model Service.published` 为发布事实。Gateway 内部把已发布模型投影到现有有效 Policy 的 `directSelection.allowedModelResourceIds`。不能简单新增一个更宽的全局 Policy，因为现有 Workspace、Project、phase 或 agent profile Policy 会按优先级覆盖它；投影器必须保留遗留非产品模型授权，并在每个现有 Policy 中增删产品 Model Service ID。Policy 不提供给普通用户，也不要求管理员直接编辑。

发布状态是产品权威源，Policy 是可重建的运行时投影。发布时先把稳定模型 ID 写入现有 Policy，再把 Model Service 标记为已发布；投影失败时资源仍保持未发布。下架时顺序相反，先隐藏并拒绝资源，再清理 Policy 投影。即使部分 Policy 已完成更新，未发布模型本身仍会被 Gateway 拒绝，不能出现产品目录展示已发布但 Gateway 必然拒绝的状态。

用户可用模型列表必须返回“已发布且 Provider 启用的模型”与该请求作用域有效 Policy 的交集，确保列表结果与 Gateway 最终授权一致。未来若环境收敛为单一平台默认 Policy，可以简化投影器，但这不是首期上线前提。

### 3.6 Token 是运行观测，不是计费账本

一期 Token 口径为：

```text
Run Known Usage = Run 下每个唯一 Turn 的最新 ModelUsage 之和
totalTokens = inputTokens + outputTokens
```

- 优先保存 Provider 响应中的 usage；
- Provider 成功响应不包含 usage 时允许本地估算，并标记 `estimated=true`；
- `cachedInputTokens` 是 input 的组成信息，不重复加入 `totalTokens`；
- 失败、超时或结果不确定的 Provider Attempt 如果没有 usage，不虚构 Token；
- 一期展示的是“已知用量”，不宣称是 Provider 账单的完整对账结果。

一期不建设 Provider Attempt Ledger。只有未来出现计费、成本结算或失败 Attempt 对账需求时，才单独设计 attempt 级事实表。

## 4. 目标用户流程

### 4.1 管理员配置 Provider

1. 进入“模型服务”管理区；
2. 创建 Provider Connection；
3. 填写名称、Provider 类型、Base URL 和 API Key；
4. 保存后 API Key 只写不读；
5. 执行连接测试；
6. 测试结果展示成功、失败原因和测试时间。

连接测试应区分两类：

- Provider Connection 测试：验证配置、URL、DNS 和凭证可解析性，不发送生成请求；若某类 Provider 后续有统一且廉价的认证探针，可由对应 Adapter 扩展网络认证检查；
- Model Service 测试：使用固定最小 Prompt 验证指定物理模型，可能产生少量 Token，并记录管理员审计事件。

### 4.2 管理员发布模型

1. 在 Provider Connection 下新增 Model Service；
2. 填写用户可见名称、物理模型名、能力和运行参数；
3. 执行模型测试；
4. 设置为已发布；
5. 系统更新现有有效 Policy 的可重建投影；
6. 模型出现在用户选择器中。

下架后：

- 新 Run 不能再选择该模型；
- 已运行中的 Run 不被主动终止，但后续模型 Turn 会被 Gateway 拒绝并以稳定错误结束；
- 历史 Run 继续显示当时的模型快照；
- Edit 如果默认模型已下架，必须要求用户重新选择。

一期选择上述 fail-closed 语义，是因为当前 Gateway 按 Turn 授权且没有 Run Binding。若未来业务明确要求“下架后已有 Run 必须继续”，再通过 Run 上的 pinned revision 或轻量授权记录实现，不在一期引入 Saga。

### 4.3 用户创建作品

1. Web 获取当前可用 Model Service 列表；
2. 用户选择模型；
3. Web 提交 `modelServiceId`；
4. BFF 映射为 Runtime `inputContext.modelResourceId`；
5. Runtime 创建 Build Run；
6. Gateway 在第一次 Turn 最终校验模型授权与能力；
7. Run 完成后展示模型和已知 Token 用量。

Build 的 `modelServiceId` 在产品 API 中必填。Runtime 内部字段在兼容期仍可选，以支持旧测试、内部任务和滚动部署；BFF 必须执行产品级必填校验。

### 4.4 用户修改作品

1. 用户从当前 Version 发起 Edit；
2. BFF 查找该 Version 对应生成 Run 的模型快照；
3. 如果模型仍已发布，UI 默认选中该模型；
4. 用户可以保持或切换模型；
5. Web 创建独立 Edit Run；
6. Edit Run 单独记录自己的模型和 Token。

Repair 默认继承父 Run 的模型；如果存在人工 Repair 入口，可以允许显式切换。

## 5. 资源模型

### 5.1 Provider Connection

建议字段：

```text
id
name
providerType
baseUrl
credentialRef
enabled
version
lastTestStatus
lastTestedAt
createdAt
updatedAt
```

约束：

- `credentialRef` 只在 Provider Gateway 内部可见；
- API Key 加密保存，查询 API 只返回 `credentialConfigured`；
- 空字符串不能覆盖已有凭证；
- 凭证轮换使用独立操作；
- `version` 只用于管理员并发控制；
- 已被 Model Service 引用的连接不能硬删除；
- disabled 的 Provider 不再出现在新 Run 的可选目录中，并拒绝后续模型 Turn。

`lastTestStatus` 只用于管理页面提示，不作为必须定期续期的租约。测试结果过期可以显示“待重新测试”，但不能仅因 TTL 到期自动隐藏所有已发布模型。

### 5.2 Model Service

建议字段：

```text
id
providerConnectionId
displayName
description
physicalModel
capabilities
defaults
published
sortOrder
version
createdAt
updatedAt
```

可供用户选择的基本条件：

```text
selectable =
  model.published
  && provider.enabled
  && model configuration valid
  && model is allowed by the effective internal Policy
```

实时故障、熔断和限流属于运行状态。用户列表可以返回简化的 `availability` 提示，但不建立 Provider、Model 两套 `desiredState / observedState / readinessExpiresAt` 状态机。

Model Service 测试结果随测试响应返回，并写管理员审计；首期不把它持久化为可影响发布资格的租约。若管理界面后续需要展示“最近测试”，可以增加诊断投影，但不得把测试 TTL 变成用户目录的隐藏条件。

### 5.3 Generation Run

复用现有 Runtime Run，不新增第二套生成任务表。产品返回模型与用量投影：

```json
{
  "id": "run-123",
  "phase": "edit",
  "model": {
    "id": "deepseek-v4-pro",
    "displayName": "DeepSeek V4 Pro",
    "physicalModel": "deepseek-v4-pro"
  },
  "usage": {
    "inputTokens": 12800,
    "outputTokens": 3200,
    "cachedInputTokens": 900,
    "totalTokens": 16000,
    "estimated": false
  }
}
```

为保证模型改名后历史 Run 展示不漂移，Gateway 的低敏感度 `ModelExecutionSnapshot` 需要补充执行时的 `displayName`。当前已有的 model resource ID、revision、Provider ID 和 physical model 继续保留。

执行快照不得包含 API Key、Authorization Header、Secret 引用、完整 endpoint、Prompt 或工具参数。

## 6. API 设计

### 6.1 管理员 Provider API

```text
GET    /internal/provider-gateway/admin/v1/provider-connections
POST   /internal/provider-gateway/admin/v1/provider-connections
GET    /internal/provider-gateway/admin/v1/provider-connections/{id}
PATCH  /internal/provider-gateway/admin/v1/provider-connections/{id}
POST   /internal/provider-gateway/admin/v1/provider-connections/{id}/test
POST   /internal/provider-gateway/admin/v1/provider-connections/{id}/enable
POST   /internal/provider-gateway/admin/v1/provider-connections/{id}/disable
POST   /internal/provider-gateway/admin/v1/provider-connections/{id}/rotate-credential
```

管理写操作继续保留：管理员认证、幂等键、乐观锁、审计和变更原因。

### 6.2 管理员 Model Service API

```text
GET    /internal/provider-gateway/admin/v1/model-services
POST   /internal/provider-gateway/admin/v1/model-services
GET    /internal/provider-gateway/admin/v1/model-services/{id}
PATCH  /internal/provider-gateway/admin/v1/model-services/{id}
POST   /internal/provider-gateway/admin/v1/model-services/{id}/test
POST   /internal/provider-gateway/admin/v1/model-services/{id}/publish
POST   /internal/provider-gateway/admin/v1/model-services/{id}/unpublish
```

### 6.3 用户可用模型 API

```text
Gateway: GET /v1/model-services?workspaceId=...&projectId=...&phase=...&agentProfile=...
Runtime: GET /projects/{projectId}/model-services?phase=...&agentProfile=...
BFF:     GET /api/projects/{projectId}/model-services?phase=...
```

响应只包含：

```json
{
  "items": [
    {
      "id": "deepseek-v4-pro",
      "displayName": "DeepSeek V4 Pro",
      "description": "适合高质量网站生成",
      "capabilities": {
        "toolCalls": true,
        "vision": true
      },
      "availability": "available"
    }
  ]
}
```

不得返回 Provider Connection、endpoint、credential、Policy 或 revision。

### 6.4 Build 与 Edit API

产品 BFF 请求：

```json
{
  "briefId": "brief-123",
  "modelServiceId": "deepseek-v4-pro"
}
```

BFF 调用 Runtime 时映射为：

```json
{
  "phase": "build",
  "inputContext": {
    "briefId": "brief-123",
    "modelResourceId": "deepseek-v4-pro"
  }
}
```

一期不要求 Runtime、Gateway 和数据库字段同步改名。

### 6.5 Run Usage API

可复用 Run 详情，也可以新增聚焦接口：

```text
Runtime: GET /runs/{runId}/model-usage
BFF:     GET /api/projects/{projectId}/runs/{runId}/model-usage
```

聚合必须按 `turn` 去重：同一 Turn 因恢复、事件重放或重复写入出现多个 `model.usage` 时，只保留最后一条有效记录。实现可以使用查询投影，也可以增加唯一键为 `(run_id, turn)` 的派生表。

## 7. Provider Gateway 实现收敛

### 7.1 保留

- `ModelResource` 的稳定 ID 和 revision 历史；
- `ModelSelectionPolicy` 及其授权、安全限额；
- 显式 `modelResourceId` 路由；
- Provider Adapter；
- capability 校验；
- deadline、重试、幂等；
- 熔断和 bulkhead；
- `ModelExecutionSnapshot`；
- Provider usage；
- 管理审计和 Secret 安全边界。

### 7.2 新增或调整

- 新增 Provider Connection 持久化和管理 API；
- Model Resource 在产品 API 中投影为 Model Service；
- Model Resource 增加或解析 `providerConnectionId`；
- 发布操作维护现有有效 Policy 的可重建投影；
- 增加普通用户可访问的低敏感度模型列表；
- 在现有 `ModelExecutionSnapshot` 中补充执行时的模型显示名；
- 增加全 Pod 配置版本刷新；
- Runtime 用量聚合按 Turn 去重并暴露给 BFF。

### 7.3 不做

- 不删除 Policy 表或授权逻辑；
- 不删除现有 revision 和执行快照；
- 不建设 Run Model Binding；
- 不建设 Attempt Ledger；
- 不建设价格、币种、账单和成本中心；
- 不要求所有服务原子切换字段名；
- 不通过 Pod rollout 传播数据库模型配置。

## 8. 多 Pod 配置一致性

### 8.1 改造前问题

Gateway 启动时从数据库加载 Model Resource 和 Policy 到本地内存。管理 API 成功后只刷新处理该请求的 Pod，其他 Pod 可能继续使用旧配置。

改造前，Provider 或模型修改后虽然不一定立即需要 rollout，但没有机制保证所有 Pod 自动收敛，存在 stale cache 风险。

### 8.2 目标机制

PostgreSQL 是唯一权威源。增加单调递增的 `configuration_version`：

1. Provider、Model Service 或 Policy 变更时，在同一数据库事务中递增 version；
2. PostgreSQL 事务提交时发送 `NOTIFY`，作为可选的低延迟刷新提示；
3. 每个 Pod 以短周期轮询 version 作为正确性基线，因此通知丢失、未启用 LISTEN 或连接重建都不会造成永久陈旧；
4. 若后续增加常驻 LISTEN 消费者，通知只负责提前唤醒同一套刷新逻辑，不能成为唯一一致性机制；
5. Pod 发现 version 变化后重新加载完整配置并原子替换内存快照；
6. 加载或校验失败时保留上一份有效快照并上报告警，不发布半完成配置；
7. 新 Pod 启动时始终从数据库加载当前配置。

建议默认收敛 SLA 为 5 秒。安全紧急停用如果要求立即生效，应增加每 Turn 的轻量 version 校验或独立紧急拒绝表，而不是依赖普通缓存刷新。

### 8.3 是否需要滚动升级 Pod

- 新增或修改 Provider Connection、Model Service、发布状态、物理模型、Base URL、凭证：不需要滚动升级；
- 修改 Gateway 代码、静态启动参数、环境变量结构、数据库连接或部署资源：需要正常滚动升级；
- 首次上线本方案的新代码和 Schema：需要一次兼容性滚动发布。

## 9. 数据与兼容策略

### 9.1 最小新增数据

```text
provider_connections
  id
  name
  provider_type
  base_url
  credential_ref
  enabled
  version
  last_test_status
  last_tested_at
  created_at
  updated_at

gateway_configuration_state
  singleton_id
  configuration_version
  updated_at
```

Model Service 首期继续复用现有 `model_resource_revisions`。在 Resource 配置中增加 `providerConnectionId`，并保留 revision、capabilities、defaults、physical model 和 enabled/published 信息。

Run Token 首期继续以现有 `model.usage` 事件为事实输入；如果事件查询成本过高，再增加可重建的 `(run_id, turn)` 用量投影表，不直接引入 attempt 表。

### 9.2 兼容读取

滚动迁移期间：

1. 新 Gateway 优先读取 `providerConnectionId`；
2. 旧 Resource 仍可回退读取内嵌 endpoint/auth；
3. 新管理 API 只创建 Provider Connection 引用；
4. 所有 Pod 升级并完成数据验证后，再停止创建旧结构；
5. 删除旧字段属于后续清理，不是本次产品上线的阻塞项。

产品 API 可以立即使用 `modelServiceId`，由 BFF 映射到内部 `modelResourceId`。不要求 Runtime 与 Gateway 同一时刻改名。

## 10. 分阶段实施计划

### 阶段一：后台兼容与一致性

1. 新增 Provider Connection 表、Secret 引用和管理 API；
2. 为 Model Resource 增加 Provider Connection 解析能力；
3. 保留旧 endpoint/auth 兼容读取；
4. 增加已发布 Model Service 到现有有效 Policy 的投影；
5. 增加 `configuration_version + NOTIFY emission + polling`；
6. 增加双 Pod 配置更新与通知丢失测试。

阶段完成后，管理员修改 Provider 或模型不需要滚动升级 Pod。

### 阶段二：用户模型选择闭环

1. 新增项目作用域的低敏感度 Model Service 目录；
2. Web 增加模型选择器；
3. Build BFF 要求 `modelServiceId`；
4. Edit BFF 默认读取基准版本模型，并允许切换；
5. BFF 将 `modelServiceId` 映射为 Runtime `modelResourceId`；
6. 增加 Build、Edit 选择不同模型的 E2E。

### 阶段三：Run 用量展示

1. 按 `(runId, turn)` 去重现有 `model.usage`；
2. 聚合 input、output、cached input 和 total；
3. 保留并返回 estimated 标记；
4. Run 详情展示逻辑模型和实际执行快照；
5. 增加 Runtime 重启、事件重放、多 Turn 聚合测试。

### 阶段四：清理与观察

1. 验证所有 Model Resource 都已引用 Provider Connection；
2. 停止旧管理入口写入内嵌 endpoint/auth；
3. 根据实际运营情况决定是否清理旧字段；
4. Policy、revision 和 execution snapshot 继续保留；
5. Attempt Ledger、配置冻结和 Workspace 模型授权仅在出现明确需求时另立方案。

## 11. 验收标准

### 11.1 管理员

- 可以创建、测试、修改、启停 Provider Connection；
- 可以安全轮换凭证，API Key 不出现在查询、日志和错误正文；
- 可以创建、测试、修改、发布和下架 Model Service；
- 修改 Provider 或模型配置后不需要滚动升级 Pod；
- 两个 Gateway Pod 在约定 SLA 内观察到相同配置 version；
- Policy 和 revision 不出现在普通产品界面。

### 11.2 普通用户

- Build 前可以看到所有可选择模型；
- Build 必须选择模型；
- Edit 默认选中基准版本所用模型；
- Edit 可以切换到另一个模型；
- 已下架模型不能创建新 Run；
- 用户看不到 Provider endpoint、Secret、Policy 或 revision。

### 11.3 Run 与 Token

- Build、Edit、Repair 分别保存自己的模型选择；
- 每个成功 Turn 保存实际执行模型快照；
- 同一个 Run 的多个 Turn 可以正确聚合；
- 同一 Turn 的重复 usage 事件不会重复累计；
- Provider usage 与本地估算可以区分；
- `totalTokens = inputTokens + outputTokens`；
- 历史 Run 不因模型改名、修改或下架而改变展示；
- API 明确说明数据是已知运行用量，不是账单级完整对账。

### 11.4 兼容与可靠性

- 新旧 Gateway Pod 在滚动期间均能读取兼容 Resource；
- notification 丢失后轮询仍能完成配置收敛；
- 配置刷新失败不会把半完成配置投入流量；
- Provider Connection、Model Service、Policy 投影更新具有稳定的失败语义；
- 现有限流、熔断、deadline、幂等和 Token 安全预算不因产品简化而失效。

## 12. 非目标与升级条件

一期不包含：

- 用户选择模型 revision；
- 历史 Policy 激活界面；
- Workspace/Project 自定义模型目录；
- 模型价格和成本结算；
- 失败 Provider Attempt 的精确账单对账；
- 跨 Provider 自动切换产品；
- 长生命周期 Run 的不可变配置 Binding；
- 多地域配置复制。

只有满足以下条件才升级设计：

| 新需求 | 对应增强 |
| --- | --- |
| 需要账单级 Token 对账 | Provider Attempt Ledger |
| 多 Turn Run 必须冻结底层配置 | Run 上增加 pinned revision |
| 不同租户看到不同模型 | `workspace_model_service_access` |
| 需要自动故障切换 | 保留并产品化 Policy candidate/switch |
| 配置规模导致全量刷新昂贵 | 增量配置事件和分片缓存 |

## 13. 最终产品边界

> 管理员负责 Provider 从哪里调用，以及哪些 Model Service 可以提供给用户；用户负责为每次作品生成或修改选择模型；平台复用现有 Run 和 Gateway 安全执行调用，并记录该 Run 的已知 Token 用量。

本方案的核心不是删除所有内部治理能力，而是把这些能力留在基础设施层，只向用户暴露完成任务所需的最小模型概念。
