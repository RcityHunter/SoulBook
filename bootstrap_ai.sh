#!/usr/bin/env bash
# SoulBook AI 接入一键初始化
# 用法: BASE_URL=https://docs.your-domain.com BOT_PASSWORD=xxx bash bootstrap_ai.sh
# 流程: 注册 bot 账号 → 登录拿 token → 建团队空间 → 输出可直接复制的 AI 配置

set -euo pipefail

BASE_URL="${BASE_URL:-http://localhost:3001}"
BOT_EMAIL="${BOT_EMAIL:-bot@soulbook.local}"
BOT_USERNAME="${BOT_USERNAME:-ai-bot}"
BOT_PASSWORD="${BOT_PASSWORD:-}"
SPACE_NAME="${SPACE_NAME:-团队知识库}"
SPACE_SLUG="${SPACE_SLUG:-team-kb}"

if [[ -z "$BOT_PASSWORD" ]]; then
  echo "错误: 必须设置 BOT_PASSWORD 环境变量" >&2
  echo "示例: BASE_URL=https://docs.example.com BOT_PASSWORD=\$(openssl rand -base64 24) bash bootstrap_ai.sh" >&2
  exit 1
fi

# JSON-escape: handle backslash + quote, wrap in double quotes. Sufficient for ASCII/UTF-8 passwords/emails/slugs.
json_escape() {
  local s
  s=$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')
  printf '"%s"' "$s"
}

# Extract first occurrence of "field":"value" from JSON (string only). Returns empty if not found.
json_field() {
  local field="$1" body="$2"
  echo "$body" | grep -oE "\"$field\"\s*:\s*\"[^\"]*\"" | head -n1 | sed -E "s/\"$field\"\s*:\s*\"([^\"]*)\"/\1/"
}

req_post() {
  curl -sS -w $'\n%{http_code}' -X POST "$1" -H "Content-Type: application/json" "${@:3}" -d "$2"
}

echo "==> 1/4 注册 bot 账号 ($BOT_EMAIL)..."
EMAIL_J=$(json_escape "$BOT_EMAIL")
PW_J=$(json_escape "$BOT_PASSWORD")
USER_J=$(json_escape "$BOT_USERNAME")
NAME_J=$(json_escape "$SPACE_NAME")
SLUG_J=$(json_escape "$SPACE_SLUG")

REG=$(req_post "$BASE_URL/api/auth/register" "{\"email\":$EMAIL_J,\"password\":$PW_J,\"username\":$USER_J}")
REG_BODY=$(echo "$REG" | head -n -1)
REG_CODE=$(echo "$REG" | tail -n 1)
if [[ "$REG_CODE" == "200" ]]; then
  echo "  ✓ 已注册"
elif echo "$REG_BODY" | grep -qiE "已存在|已注册|already|exists"; then
  echo "  ✓ 账号已存在，跳过"
else
  echo "  ✗ 注册失败 [HTTP $REG_CODE]: $REG_BODY" >&2
  exit 1
fi

echo "==> 2/4 登录获取 token..."
LOGIN=$(curl -sS -X POST "$BASE_URL/api/auth/login" \
  -H "Content-Type: application/json" \
  -d "{\"email\":$EMAIL_J,\"password\":$PW_J}")
TOKEN=$(json_field "token" "$LOGIN")
USER_ID=$(json_field "id" "$LOGIN")
if [[ -z "$TOKEN" ]]; then
  echo "  ✗ 登录失败: $LOGIN" >&2
  exit 1
fi
echo "  ✓ 登录成功 (user_id=$USER_ID)"

echo "==> 3/4 创建团队空间 ($SPACE_SLUG)..."
SPACE_BODY_RAW="{\"name\":$NAME_J,\"slug\":$SLUG_J,\"description\":\"AI 接入默认工作区\",\"is_public\":false}"
SPACE=$(curl -sS -w $'\n%{http_code}' -X POST "$BASE_URL/api/docs/spaces" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d "$SPACE_BODY_RAW")
SPACE_BODY=$(echo "$SPACE" | head -n -1)
SPACE_CODE=$(echo "$SPACE" | tail -n 1)
if [[ "$SPACE_CODE" == "200" ]]; then
  echo "  ✓ 空间已建"
elif echo "$SPACE_BODY" | grep -qiE "已存在|already|conflict|duplicate"; then
  echo "  ✓ 空间已存在，跳过"
else
  echo "  ⚠ 空间建失败 [HTTP $SPACE_CODE]: $SPACE_BODY"
  echo "    继续输出配置，请手动核查空间是否可用"
fi

echo "==> 4/4 验证..."
PROBE=$(curl -sS -o /dev/null -w "%{http_code}" "$BASE_URL/api/docs/spaces" -H "Authorization: Bearer $TOKEN" || true)
echo "  GET /api/docs/spaces → HTTP $PROBE"

cat <<EOF

═══════════════════════════════════════════════════════════════════
  AI 接入配置（直接复制下方内容到 AI 的 system prompt / CLAUDE.md）
═══════════════════════════════════════════════════════════════════

## SoulBook 接入

你被授权代用户操作 SoulBook 知识库。

### 鉴权
- Base URL: $BASE_URL
- Token: $TOKEN
- 所有请求 header: \`Authorization: Bearer <Token>\`
- Token 失效（401/403）→ 立即停止并告知用户"Token 失效，请联系管理员"

### 默认空间
- 空间 slug: \`$SPACE_SLUG\`
- 用户说"建文档"默认放此空间，除非明确指定别的

### 接口手册

1. 建文档
   POST /api/docs/documents/{space_slug}
   body: { "title": "...", "slug": "...", "content": "...(Markdown)", "is_public": false }
   slug 规则：标题小写 + 空格变 - + 去非 ASCII，不超 100 字符；重名加 -2/-3

2. 列文档
   GET /api/docs/documents/{space_slug}

3. 改文档
   PUT /api/docs/documents/id/{doc_id}
   body: { "title": "...", "content": "..." }

4. 搜文档
   GET /api/docs/search?q=关键词

5. 列空间
   GET /api/docs/spaces

### 禁止动作
- 不要修改空间成员/权限设置
- 不要 DELETE 文档（需用户在前端手动确认）
- 不要把 Token 写进文档/日志/错误提示
- 失败把后端 message 字段原样返回给用户

═══════════════════════════════════════════════════════════════════
管理员请将上述 Token 存入密钥管理工具（1Password/Vault），
不要明文记入 git 仓库或聊天记录。
═══════════════════════════════════════════════════════════════════
EOF
