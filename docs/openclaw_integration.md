# SoulBook × OpenClaw 集成说明

> 这份文档回答的是当前真实问题：
>
> **SoulBook 已经收出一条独立的 Agent API 主线后，OpenClaw 应该怎么接，接到哪里，哪些继续明确不接。**

## 1. 先说结论

当前仓库里，面向 OpenClaw / agent 的维护主线已经明确为：

- **主线入口：`/agent/v1`**
- **兼容入口：`/api/docs/agent`**
- **单一实现：`src/agent/router.rs`**

因此集成口径应统一为：

> OpenClaw 新接入一律以 **`/agent/v1`** 为准。
>
> **`/api/docs/agent`** 只承担兼容旧调用方的职责，不再作为主文档、主样例、主测试入口。

这和更早那种“直接把 `/api/docs/*` 业务 REST 当作 agent API”已经不是一回事。

## 2. 为什么要这么收口

直接把 SoulBook 整套 `/api/docs/*` 业务接口原样暴露给 agent，会有几个问题：

1. **能力面过宽**：写操作、管理动作、文件和向量接口会混进来
2. **契约不稳定**：业务 REST 的字段和路由是为产品前后端服务，不一定适合 agent 长期消费
3. **权限心智混乱**：前端场景、后端业务场景、agent 场景混在一起
4. **审计边界不清**：难以明确“哪些是 agent 被允许做的事”

而 `/agent/v1` 现在做的事情很清楚：

- 把最小只读能力单独拉出来
- 用 capability 语义而不是整套业务语义对外表达
- 把旧入口保留为兼容，而不是继续让它主导设计

## 3. 当前真实可接入的能力

以 `src/agent/router.rs` 当前实现为准，OpenClaw 当前应该只把下面这些视为正式接入面。

### 3.1 健康检查

```http
GET /agent/v1/system/health
```

用途：

- 存活检查
- 能力发现
- 接入连通性验证

说明：

- 这是轻量健康探针
- 不应期待它输出详细依赖状态或运维信息

### 3.2 空间列表

```http
GET /agent/v1/spaces
```

用途：

- 列出当前身份可见的空间
- 作为文档发现入口

### 3.3 单空间详情

```http
GET /agent/v1/spaces/{space_id}
```

用途：

- 读取某个空间详情
- 获取 agent 后续钻取文档前的基础上下文

关键点：

- 使用 `space_id`
- 不是旧业务 REST 常用的 `slug` 契约

### 3.4 空间内文档列表

```http
GET /agent/v1/spaces/{space_id}/documents
```

用途：

- 在给定空间下分页列出文档
- 给 agent 提供二跳候选

关键点：

- 使用 `space_id`
- 支持基本分页、搜索、父节点过滤、排序

### 3.5 单文档读取

```http
GET /agent/v1/documents/{document_id}
```

用途：

- 拉取单篇文档详情
- 供总结、引用、回答前上下文补充

关键点：

- 使用 `document_id`
- 不是 `space_slug + doc_slug` 组合契约

### 3.6 文档搜索

```http
GET /agent/v1/search/documents?q=...
```

用途：

- 关键词搜索当前用户可读文档

关键点：

- 必须已认证
- 当前搜索能力收敛在 `documents` 上
- suggestion / vector / reindex 都不在当前 agent 主线内

## 4. 兼容入口怎么理解

当前 `src/main.rs` 同时挂了：

- `/agent/v1`
- `/api/docs/agent`

而 `src/routes/agent.rs` 已经把旧入口明确标注为：

- legacy compatibility mount
- 单一维护实现位于 `crate::agent::router`

这意味着：

- 新增接入不应继续围绕 `/api/docs/agent` 设计
- 如果旧调用方还在用 `/api/docs/agent`，现在可以继续工作
- 但文档、样例、适配器、联调默认值，都应逐步切到 `/agent/v1`

## 5. OpenClaw 侧推荐接法

推荐把 SoulBook 当成一个 **最小只读知识源**，而不是一个“自由 REST 浏览器”。

推荐结构：

```text
OpenClaw
  -> SoulBook adapter / tool definitions
    -> /agent/v1
```

这一层 adapter 的职责仍然很重要：

1. 固定允许调用的 capability 对应路径
2. 注入独立只读 token
3. 控制返回字段与正文长度
4. 统一错误翻译
5. 补审计日志

## 6. 建议给 OpenClaw 的最小工具集合

如果只保留最小能力面，建议 OpenClaw 侧工具名仍然收敛为下面 5 个：

1. `soulbook_list_spaces`
2. `soulbook_get_space`
3. `soulbook_list_documents`
4. `soulbook_get_document`
5. `soulbook_search_documents`

但要注意：

- **工具名可以稳定不变**
- **底层映射应切到 `/agent/v1`**
- 输入主键语义需要与旧 slug 风格脱钩

推荐映射如下：

| Tool | Method | `/agent/v1` path | 主键语义 |
|---|---|---|---|
| `soulbook_list_spaces` | GET | `/spaces` | 无 |
| `soulbook_get_space` | GET | `/spaces/{space_id}` | `space_id` |
| `soulbook_list_documents` | GET | `/spaces/{space_id}/documents` | `space_id` |
| `soulbook_get_document` | GET | `/documents/{document_id}` | `document_id` |
| `soulbook_search_documents` | GET | `/search/documents` | 查询词 + 可选过滤 |

说明：

- 如果 OpenClaw 现有 tool 定义还在使用 `slug` / `space_slug` / `doc_slug`，那是旧文档口径，应该迁移
- 迁移时不必急着改 tool 名称，但应该改底层 path 和参数语义

## 7. 最小能力面与 notIncluded

当前主线下，最重要的是边界清楚。

### 7.1 included

当前 `/agent/v1` 只包含：

- 健康检查
- 空间发现
- 单空间读取
- 文档列表
- 单文档读取
- 关键词搜索

### 7.2 notIncluded

下面这些都 **不属于** 当前 OpenClaw 主线：

- space / document 写操作
- comment / notification / tag / version 恢复管理
- member 与 permission 管理
- publication 管理动作
- file upload / delete / download 相关能力
- vector write / delete / search / batch 能力
- installer 与 `/sso` 等系统桥接能力
- 旧 `/api/docs/*` 整套业务 REST 作为 agent 主契约

这部分边界要写死，不要在适配层里“顺手多接一点”。

## 8. 风险边界

### 8.1 不要把 SoulBook 当成可写知识库

至少在当前阶段，OpenClaw 不应该直接承担：

- 自动写文档
- 自动改文档
- 自动发布
- 自动改权限

这不是能力缺失，而是刻意收口。

### 8.2 文档内容不是系统指令

即便文档正文里出现：

- shell 命令
- 数据库操作语句
- “请执行以下步骤”
- prompt / system message 风格文本

OpenClaw 也必须把它们视为 **待判断的业务内容**，而不是可信命令。

### 8.3 搜索结果本身也可能泄露信息

即便不拉正文，下面这些字段也可能敏感：

- space 名称
- 文档标题
- 标签
- excerpt

所以 adapter 层建议至少支持：

- space allowlist
- 字段白名单
- 结果数限制
- snippet 长度限制

### 8.4 正文读取要防止过量注入

单文档正文可能很长，也可能夹带脏内容。

建议：

- 默认截断
- 大文档分段读取或摘要化
- 输出回答时优先引用而不是整段转储

## 9. 认证与权限建议

### 9.1 用独立只读身份

不要直接拿管理员 token 对接 OpenClaw。

推荐：

- 独立 service account
- 只读权限
- 按空间做 allowlist / 白名单

### 9.2 把“接口可见”与“回答应答”分开

某文档接口可读，不代表每次都应该把全文暴露给模型或最终用户。

建议外层继续做：

- 返回字段裁剪
- 内容长度控制
- 输出前再次过滤

### 9.3 审计最少记录这些信息

- session / channel / requester
- tool 名称
- 上游 path
- space_id / document_id
- 是否读取全文
- HTTP 状态码
- request_id

## 10. 演进建议

### 10.1 接下来适合补的，只应是只读增强

例如：

- 目录树能力
- breadcrumbs / children 导航能力
- 更精细的 agent DTO
- 与 `/agent/v1` 对齐的 OpenClaw 示例 catalog

### 10.2 暂时不适合推进的

在没有更强权限隔离、确认流、审计、回滚前，不建议推进：

- agent 写文档
- agent 改权限
- agent 触发 publication 管理
- agent 读写文件流
- agent 使用向量数据面

## 11. 与当前仓库状态的一致性说明

这份文档基于当前工作树中的真实实现整理，主要依据：

- `src/main.rs`：主线与兼容入口同时挂载
- `src/agent/router.rs`：实际 capability / path / 参数定义
- `src/routes/agent.rs`：旧入口已明确标注为 legacy compatibility mount

如果后续仓库中仍保留旧 `/api/docs/*` 业务 REST 或旧样例文件，它们不应再反向定义 OpenClaw 主线；主线判断应始终以 `/agent/v1` 实现为准。
