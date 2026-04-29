# 向量存储功能文档

## 概述

SoulBook 向量存储功能为文档系统提供了语义搜索和 AI 应用支持。系统作为一个向量化文档数据库，专注于向量的存储和检索，而向量的生成由调用方负责。

## 架构设计

### 核心理念

1. **职责分离**: SoulBook 专注于向量存储和检索，不负责向量生成
2. **灵活性**: 调用者可以使用任何嵌入模型（OpenAI、文心、通义等）
3. **简单性**: 提供清晰的 RESTful API 接口
4. **独立性**: 向量功能与现有关键词搜索完全独立

### 数据模型

```sql
-- 文档向量存储表
document_vector
├── id: record(document_vector)          -- 向量记录ID
├── document_id: record(document)        -- 关联的文档ID
├── space_id: record(space)              -- 所属空间ID
├── embedding: array<float>              -- 向量数组
├── embedding_model: string              -- 使用的嵌入模型
├── dimension: int                       -- 向量维度
├── metadata: object                     -- 额外元数据
├── created_at: datetime                 -- 创建时间
└── updated_at: datetime                 -- 更新时间
```

## API 接口

### 1. 存储文档向量

**端点**: `POST /api/docs/documents/{document_id}/vectors`

**请求体**:
```json
{
    "embedding": [0.1, 0.2, 0.3, ...],  // 向量数组
    "model": "text-embedding-ada-002",   // 模型名称
    "dimension": 1536,                   // 向量维度
    "metadata": {                        // 可选元数据
        "version": "1.0",
        "generated_at": "2024-01-01T00:00:00Z"
    }
}
```

**响应**:
```json
{
    "success": true,
    "vector_id": "document_vector:xyz123",
    "document_id": "document:abc456"
}
```

### 2. 向量相似度搜索

**端点**: `POST /api/docs/search/vector`

**请求体**:
```json
{
    "query_vector": [0.1, 0.2, 0.3, ...],  // 查询向量
    "space_id": "space_id_optional",       // 可选：限定搜索空间
    "limit": 10,                           // 返回结果数量
    "threshold": 0.7,                      // 相似度阈值
    "include_content": true                // 是否返回文档内容
}
```

**响应**:
```json
{
    "results": [
        {
            "document_id": "document:abc456",
            "title": "文档标题",
            "content": "文档内容...",      // 如果 include_content 为 true
            "similarity": 0.95,            // 相似度分数
            "space_id": "space:xyz789"
        }
    ],
    "total": 10,
    "query_dimension": 1536
}
```

### 3. 获取文档向量

**端点**: `GET /api/docs/documents/{document_id}/vectors`

**响应**:
```json
{
    "document_id": "document:abc456",
    "vectors": [
        {
            "vector_id": "document_vector:xyz123",
            "embedding": [0.1, 0.2, ...],
            "model": "text-embedding-ada-002",
            "dimension": 1536,
            "created_at": "2024-01-01T00:00:00Z"
        }
    ]
}
```

### 4. 删除文档向量

**端点**: `DELETE /api/docs/documents/{document_id}/vectors/{vector_id}`

**响应**:
```json
{
    "success": true,
    "deleted_vector_id": "document_vector:xyz123"
}
```

### 5. 批量获取文档内容

**端点**: `POST /api/docs/documents/batch`

**请求体**:
```json
{
    "document_ids": ["id1", "id2", "id3"],
    "fields": ["title", "content", "excerpt"]
}
```

**响应**:
```json
{
    "documents": [
        {
            "id": "document:id1",
            "title": "标题1",
            "content": "内容1...",
            "excerpt": "摘要1..."
        }
    ]
}
```

### 6. 批量更新向量

**端点**: `POST /api/docs/vectors/batch`

**请求体**:
```json
{
    "vectors": [
        {
            "document_id": "id1",
            "embedding": [0.1, 0.2, ...],
            "model": "text-embedding-ada-002",
            "dimension": 1536
        }
    ]
}
```

**响应**:
```json
{
    "success": true,
    "processed": 10,
    "failed": 0,
    "vector_ids": ["document_vector:id1", ...]
}
```

## 使用示例

### JavaScript/TypeScript

```javascript
// 1. 创建文档
const doc = await api.post('/api/docs/documents', {
    title: 'AI技术指南',
    content: '这是一篇关于AI技术的详细指南...'
});

// 2. 生成向量（使用 OpenAI）
const embedding = await openai.createEmbedding({
    model: 'text-embedding-ada-002',
    input: doc.content
});

// 3. 存储向量
await api.post(`/api/docs/documents/${doc.id}/vectors`, {
    embedding: embedding.data[0].embedding,
    model: 'text-embedding-ada-002',
    dimension: 1536
});

// 4. 语义搜索
const userQuery = "如何使用深度学习";
const queryEmbedding = await openai.createEmbedding({
    model: 'text-embedding-ada-002',
    input: userQuery
});

const searchResults = await api.post('/api/docs/search/vector', {
    query_vector: queryEmbedding.data[0].embedding,
    limit: 10,
    threshold: 0.7,
    include_content: true
});
```

### Python

```python
import requests
import openai

# 1. 创建文档
doc = requests.post('http://localhost:3000/api/docs/documents', json={
    'title': 'AI技术指南',
    'content': '这是一篇关于AI技术的详细指南...'
}).json()

# 2. 生成向量
response = openai.Embedding.create(
    model="text-embedding-ada-002",
    input=doc['content']
)
embedding = response['data'][0]['embedding']

# 3. 存储向量
requests.post(
    f'http://localhost:3000/api/docs/documents/{doc["id"]}/vectors',
    json={
        'embedding': embedding,
        'model': 'text-embedding-ada-002',
        'dimension': 1536
    }
)

# 4. 语义搜索
query = "如何使用深度学习"
query_response = openai.Embedding.create(
    model="text-embedding-ada-002",
    input=query
)
query_embedding = query_response['data'][0]['embedding']

results = requests.post(
    'http://localhost:3000/api/docs/search/vector',
    json={
        'query_vector': query_embedding,
        'limit': 10,
        'threshold': 0.7,
        'include_content': True
    }
).json()
```

## 集成指南

### 1. 选择嵌入模型

SoulBook 不限制嵌入模型的选择，你可以使用：

- **OpenAI**: text-embedding-ada-002, text-embedding-3-small, text-embedding-3-large
- **文心一言**: Embedding-V1
- **通义千问**: text-embedding-v1
- **本地模型**: Sentence-BERT, BGE, M3E 等

### 2. 向量化策略

**全量向量化**:
```javascript
// 获取所有文档
const documents = await api.get('/api/docs/documents?limit=1000');

// 批量生成和存储向量
for (const batch of chunks(documents, 100)) {
    const vectors = await generateEmbeddings(batch);
    await api.post('/api/docs/vectors/batch', { vectors });
}
```

**增量向量化**:
```javascript
// 监听文档创建/更新事件
onDocumentChange(async (doc) => {
    const embedding = await generateEmbedding(doc.content);
    await storeVector(doc.id, embedding);
});
```

### 3. 混合搜索实现

虽然 SoulBook 的向量搜索和关键词搜索是独立的，但你可以在应用层实现混合搜索：

```javascript
async function hybridSearch(query, options = {}) {
    // 并行执行两种搜索
    const [keywordResults, vectorResults] = await Promise.all([
        // 关键词搜索
        api.post('/api/docs/search', { 
            query, 
            limit: options.limit || 20 
        }),
        
        // 向量搜索
        (async () => {
            const embedding = await generateEmbedding(query);
            return api.post('/api/docs/search/vector', {
                query_vector: embedding,
                limit: options.limit || 20,
                threshold: options.threshold || 0.7
            });
        })()
    ]);
    
    // 合并和排序结果
    return mergeSearchResults(keywordResults, vectorResults);
}
```

## 性能优化

### 1. 批量操作

尽可能使用批量接口以提高性能：

```javascript
// 好的做法
await api.post('/api/docs/vectors/batch', {
    vectors: documentsToVectorize
});

// 避免
for (const doc of documents) {
    await api.post(`/api/docs/documents/${doc.id}/vectors`, {...});
}
```

### 2. 向量维度

选择合适的向量维度平衡精度和性能：

- **低维度 (384-768)**: 更快的搜索，较低的存储成本
- **中维度 (1024-1536)**: 平衡的选择
- **高维度 (2048-4096)**: 更高的精度，但性能开销更大

### 3. 相似度阈值

根据应用需求调整相似度阈值：

- **0.9+**: 非常相似的内容
- **0.7-0.9**: 相关内容
- **0.5-0.7**: 松散相关
- **<0.5**: 通常不相关

## 常见问题

### Q: 为什么 SoulBook 不内置向量生成功能？

A: 这样设计有几个优势：
- 调用者可以自由选择任何嵌入模型
- 避免 API 密钥管理的复杂性
- 调用者完全控制成本
- 系统更加简洁和专注

### Q: 如何处理不同维度的向量？

A: SoulBook 支持存储不同维度的向量，但建议在同一个空间内使用相同维度的向量以获得最佳搜索效果。

### Q: 向量搜索的性能如何？

A: 性能取决于多个因素：
- 向量维度
- 文档数量
- SurrealDB 的配置和硬件

对于大规模应用，建议：
- 使用适当的向量维度
- 实施缓存策略
- 考虑向量索引优化

### Q: 如何更新已存在的向量？

A: 目前需要先删除旧向量，然后创建新向量：

```javascript
// 删除旧向量
await api.delete(`/api/docs/documents/${docId}/vectors/${vectorId}`);

// 创建新向量
await api.post(`/api/docs/documents/${docId}/vectors`, newVectorData);
```

## 错误处理

### 常见错误码

- `400 Bad Request`: 向量维度不匹配或请求格式错误
- `404 Not Found`: 文档或向量不存在
- `500 Internal Server Error`: 服务器内部错误

### 错误处理示例

```javascript
try {
    const response = await api.post('/api/docs/search/vector', {
        query_vector: embedding,
        limit: 10
    });
    // 处理结果
} catch (error) {
    if (error.response?.status === 404) {
        console.error('文档不存在');
    } else if (error.response?.status === 400) {
        console.error('请求参数错误:', error.response.data);
    } else {
        console.error('未知错误:', error);
    }
}
```

## 未来规划

- [ ] 支持更多向量相似度算法（欧氏距离、点积等）
- [ ] 向量压缩和量化支持
- [ ] 向量版本管理
- [ ] 自动向量更新策略
- [ ] 向量索引优化（HNSW、IVF等）

## 相关资源

- [向量方案设计文档](./向量方案.md)
- [SurrealDB 向量函数文档](https://docs.surrealdb.com/docs/surrealql/functions/vector)
- [OpenAI Embeddings API](https://platform.openai.com/docs/guides/embeddings)
