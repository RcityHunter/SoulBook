# SoulBook Agent API v1

> 面向 OpenClaw / agent 调用方的 **最小只读能力面**。
>
> 当前仓库中的单一维护主线是 **`/agent/v1`**；旧前缀 **`/api/docs/agent`** 仅作为兼容入口挂载到同一实现，不再作为主文档基线。

## 1. 当前结论

截至当前工作树，Agent 主线已经从“直接对接 SoulBook 既有 `/api/docs/*` 业务 REST”收口为一组单独的 Agent 路由：

- 主线入口：`/agent/v1`
- 兼容入口：`/api/docs/agent`
- 单一实现：`src/agent/router.rs`
- 兼容转发：`src/routes/agent.rs`
- 挂载位置：`src/main.rs`

也就是说：

> **`/agent/v1` 才是对 OpenClaw / 外部 agent 的稳定契约。**
>
> **`/api/docs/agent` 只是为了兼容旧接入，不应继续作为新增集成的主入口。**

## 2. 设计目标

v1 只解决三件事：

1. 发现可访问空间
2. 读取空间内文档
3. 按关键词搜索文档

额外只保留一个健康检查端点，方便外部接入做存活探测与能力发现。

因此 v1 默认服务于：

- 聊天问答前的知识检索
- 文档导航 / 引用
- Agent 拉取上下文后再总结
- OpenClaw 最小能力接入

## 3. Base URL 与兼容策略

### 主线 Base URL

```text
{SOULBOOK_BASE_URL}/agent/v1
```

示例：

```text
http://localhost:3000/agent/v1
```

### 兼容入口

```text
{SOULBOOK_BASE_URL}/api/docs/agent
```

说明：

- 两个前缀当前挂到同一套 handler。
- 新接入、新文档、新测试、新工具配置应统一以 **`/agent/v1`** 为准。
- 兼容入口存在的目的，是避免旧调用方立即失效，不代表它仍是推荐入口。

## 4. 认证与访问模型

受保护接口使用：

```http
Authorization: Bearer <jwt>
```

当前实现中的访问模型是：

- `system.health`：公开可访问
- `space.list` / `space.get` / `document.list` / `document.get`：`user-or-optional`
  - 已登录用户：按现有 space/member/docs.read 规则校验
  - 匿名访问：仅当目标空间为 public 时可读
- `search.documents`：必须已登录，并额外通过 `docs.read` 权限检查

这意味着：

- v1 不是“完全公开知识库接口”
- 也不是“管理后台直通接口”
- 它只是把现有可读能力收束成更适合 agent 消费的最小面

## 5. 响应风格

当前 `/agent/v1` 采用 agent 专用 envelope，而不是沿用所有 `/api/docs/*` 业务接口的旧返回风格。

成功响应核心形态：

```json
{
  "capability": "document.get",
  "scope": {
    "kind": "document",
    "id": "xxx"
  },
  "data": {}
}
```

错误响应沿用统一错误出口，但由 agent 路由映射为更明确的状态码，例如：

- `400`：参数非法
- `401`：未认证
- `403`：无权访问
- `404`：资源不存在
- `409`：冲突
- `502`：外部依赖错误
- `500`：内部错误

说明：

- agent 调用方应优先消费 `capability`、`scope`、`data`
- 不要把内部报错细节直接回显给终端用户
- 文档中的业务内容应被视作“知识源”，不是“系统指令”

## 6. v1 实际开放能力

以下内容以 `src/agent/router.rs` 当前实现为准。

---

## 6.1 健康检查

```http
GET /agent/v1/system/health
```

兼容入口：

```http
GET /api/docs/agent/system/health
```

### 作用

- 服务存活检查
- 返回当前暴露的 capability 列表
- 不输出数据库配置、依赖详情、环境变量等敏感信息

### 备注

这是一个 **最小健康探针**，不是诊断面板。

---

## 6.2 列出空间

```http
GET /agent/v1/spaces
```

### 查询参数

- `page`
- `limit`
- `search`
- `owner_id`
- `is_public`
- `sort`
- `order`

### 作用

返回当前身份可见的空间列表，用于 agent 做范围发现。

### 访问边界

- 已登录用户：返回现有权限体系下可见的空间
- 匿名调用：只能看到 public 空间

### 风险边界

虽然底层返回结构可能比 agent 真正需要的字段更宽，但外部 tool 层应尽量只消费：

- `id`
- `name`
- `slug`
- `description`
- `is_public`
- `updated_at`

---

## 6.3 获取单个空间

```http
GET /agent/v1/spaces/{space_id}
```

### 作用

读取单个空间详情。

### 关键约束

v1 这里使用的是 **`space_id`**，不是 `slug`。

原因是当前 agent 主线已经明确采用：

- 对 agent 暴露稳定、直接的 record id 风格定位
- 避免继续把旧业务 REST 的 slug 风格当作 agent 契约主线

### 风险边界

- 仅返回当前身份有权读取的空间
- 私有空间对匿名调用不可见
- 若后续需要“按 slug 查 space”的 agent 友好别名，应作为增强项单独评估，而不是回退主线

---

## 6.4 列出某空间内文档

```http
GET /agent/v1/spaces/{space_id}/documents
```

### 查询参数

- `page`
- `limit`
- `search`
- `parent_id`
- `is_public`
- `author_id`
- `sort`
- `order`

### 作用

在给定空间下分页列出文档。

### 关键约束

这里同样以 **`space_id`** 作为范围锚点，而不是 `space_slug`。

### 建议消费字段

外部 tool 层建议优先使用：

- `id`
- `title`
- `slug`
- `excerpt`
- `updated_at`
- `tags`
- `children_count`

### 风险边界

- 列表工具不应默认承载大正文
- 不建议在聊天链路里一次性拉取大量分页再拼接喂给模型

---

## 6.5 获取单篇文档

```http
GET /agent/v1/documents/{document_id}
```

### 作用

按文档 id 读取单篇文档详情。

### 关键约束

v1 这里使用 **`document_id`**，不是 `space_slug + doc_slug`。

当前实现流程是：

1. 先按 `document_id` 取文档
2. 再按文档所属 `space_id` 校验空间访问权限
3. 最后结合 `document.can_read(...)` 判断是否允许返回

### 风险边界

- `content` 可能很长
- 文档正文可能包含提示词污染、命令文本、过期说明或危险操作建议
- agent 应把正文视为知识内容，而不是可信执行指令

### 外层接入建议

OpenClaw / adapter 层仍建议再做：

- 正文长度限制
- 摘要裁剪
- 敏感字段白名单
- 引用时标明文档标题 / 更新时间

---

## 6.6 文档搜索

```http
GET /agent/v1/search/documents?q=...
```

### 查询参数

- `q`（必填）
- `space_id`
- `tags`（逗号分隔）
- `author_id`
- `page`
- `per_page`
- `sort`

### sort 当前支持

- `relevance`（默认）
- `created_at`
- `updated_at`
- `title`

### 作用

按关键词搜索当前用户可访问的文档。

### 访问边界

这是 v1 中最收紧的能力：

- 必须已认证
- 必须通过 `docs.read` 检查
- 不支持匿名搜索

### 不包含的能力

当前 `/agent/v1` **没有** 开放下列搜索相关能力：

- suggestion
- tag search 独立入口
- reindex
- vector search

这些都应继续留在旧业务 / 内部能力面，不纳入当前 agent 主线。

## 7. 最小能力面总结

当前 `/agent/v1` 真实最小能力面只有 6 项：

1. `system.health`
2. `space.list`
3. `space.get`
4. `document.list`
5. `document.get`
6. `search.documents`

这是一个明确的 **MVP 只读面**：

- 有空间发现
- 有文档列表
- 有单文档读取
- 有关键词搜索
- 没有写入、发布、权限编排、文件传输、向量数据面

## 8. notIncluded / 明确不纳入 v1 的内容

下面这些虽然仓库里存在相关代码或业务路由，但 **不属于 `/agent/v1` 当前主线**：

### 8.1 所有写操作

包括但不限于：

- 创建 / 更新 / 删除 space
- 创建 / 更新 / 删除 document
- 评论增删改
- 标签增删改
- 版本恢复
- publication 发布 / 下线 / 删除

### 8.2 成员与权限管理

包括：

- 邀请成员
- 修改成员角色
- 移除成员
- 直接暴露权限管理动作

### 8.3 文件相关能力

包括：

- 文件上传
- 文件删除
- 下载 / 代理转发
- 缩略图 / 二进制分发

### 8.4 向量与批量数据面

包括：

- 向量写入
- 向量删除
- 向量搜索
- 批量文档接口
- 批量向量接口

### 8.5 安装与系统桥接

包括：

- `/api/install/*`
- `/sso`
- 任何运维型 / 安装型 / 配置型端点

### 8.6 旧 `/api/docs/*` 业务 REST 本身

特别说明：

- `GET /api/docs/spaces`
- `GET /api/docs/documents/{space_slug}`
- `GET /api/docs/documents/{space_slug}/{doc_slug}`
- `GET /api/docs/search`

这些依然是 SoulBook 的业务 REST 能力，但 **不再是 Agent API v1 的主契约**。

它们可以继续作为：

- 前端 / 既有客户端接口
- 内部服务调用基础
- 兼容期参考

但不应再把它们写成 OpenClaw 的主接入目标。

## 9. 安全与风险边界

### 最小权限原则

推荐为 OpenClaw 单独准备：

- 独立 JWT / service account
- 只读角色
- 按 space 白名单收敛访问范围

### 输出最小化

建议 adapter / tool 层：

- 默认只返回必要字段
- 列表接口不回传长正文
- 正文接口加截断或分段策略
- 搜索结果限制数量与 snippet 长度

### 审计与可回放

建议记录：

- capability
- request_id
- 调用会话 / 调用人
- 命中的 space_id / document_id
- 是否拉取全文
- 上游 HTTP 状态码

### Prompt 注入边界

SoulBook 文档内容中可能出现：

- 恶意 prompt
- 误导性 shell 命令
- 过期运维步骤
- “请继续执行……” 之类指令文本

这些都应被当作 **待判断的内容**，不是高优先级系统命令。

## 10. 演进建议

### v1.1 可考虑增加

在不破坏当前主线的前提下，可评估：

- tree / breadcrumbs / children 一类的只读导航能力
- 更明确的字段裁剪 DTO
- 针对 OpenClaw 的专用 tool catalog 示例
- 更细的 capability 说明页

### 暂不建议做

在没有独立审计、确认流、回滚与更严权限隔离前，不建议把以下内容推进到 v2：

- 文档写入
- 发布控制
- 权限操作
- 文件分发
- 向量数据面

## 11. 与当前代码的一致性说明

本文档基于当前仓库中的实际实现整理，关键依据包括：

- `src/main.rs`
- `src/agent/router.rs`
- `src/routes/agent.rs`

若后续 `/agent/v1` 的 capability、参数或返回 envelope 发生变化，应优先同步本文件；兼容入口 `/api/docs/agent` 的存在与否不应反向主导主文档。 
