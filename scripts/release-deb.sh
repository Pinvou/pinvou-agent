#!/bin/bash
# pinvou3 发布脚本：构建 deb → 生成 latest.json → rsync 上传到更新源。
# 用法: ./scripts/release-deb.sh "本次更新说明"
#
# 发版三步:
#   1. bump 版本号(tauri.conf.json / Cargo.toml / package.json 三处,本脚本会校验一致)
#   2. ./scripts/release-deb.sh "修了 xxx"
#   3. 客户端 App 启动/手动检查即可看到新版
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_DIR="$REPO_ROOT/pinvou3-app"
SERVER="admin@8.218.49.20"
REMOTE_DIR="/var/www/pinvou3"
BASE_URL="https://pinvou.com/pinvou3"

NOTES="${1:-}"
if [ -z "$NOTES" ]; then
  echo "用法: $0 \"本次更新说明\"" >&2
  exit 1
fi

# ── 1. 三处版本号一致校验（防版本漂移发出错包）────────────────────
V_TAURI=$(jq -r .version "$APP_DIR/src-tauri/tauri.conf.json")
V_CARGO=$(grep -m1 '^version = ' "$APP_DIR/src-tauri/Cargo.toml" | sed 's/version = "\(.*\)"/\1/')
V_NPM=$(jq -r .version "$APP_DIR/package.json")
if [ "$V_TAURI" != "$V_CARGO" ] || [ "$V_TAURI" != "$V_NPM" ]; then
  echo "版本号不一致: tauri.conf.json=$V_TAURI Cargo.toml=$V_CARGO package.json=$V_NPM" >&2
  exit 1
fi
VERSION="$V_TAURI"
echo "=== 发布 pinvou3 v$VERSION ==="

# ── 1.5 embedding 模型不再随 deb 打包（deb 瘦 ~559MB）──────
# 模型(bge-m3)改由客户端在知识库页**按需下载**,见 scripts/release-kb-model.sh(一次性发布到
# OTA 源 kb-model/bge-m3.tar.gz,变更模型才跑)。这里不再拷模型进 resources/bge-m3。

# ── 内置工具共享 key 注入(编译期 option_env! 读,见 marketplace resolve_secret_placeholder 兜底)──
# key 不进 git:放 gitignored 的 scripts/.builtin-secrets.env(模板见同名 .example)。
# **硬检查**:缺文件 / key 为空直接报错退出 —— 防止静默发出「内置工具对新用户不可用」的坏包
# (option_env! 取不到值编译不报错,漏填会悄悄发坏版本)。确要发不带内置 key 的版本:
# 设 PINVOU3_SKIP_BUILTIN_SECRETS=1 显式跳过。
SECRETS_ENV="$REPO_ROOT/scripts/.builtin-secrets.env"
if [ "${PINVOU3_SKIP_BUILTIN_SECRETS:-0}" = "1" ]; then
  echo "⚠️  PINVOU3_SKIP_BUILTIN_SECRETS=1 → 本版不含内置共享 key(新用户装天气/问财/企查查需自填)" >&2
elif [ -f "$SECRETS_ENV" ]; then
  set -a; . "$SECRETS_ENV"; set +a
  missing=""
  [ -z "${PINVOU3_BUILTIN_AMAP_KEY:-}" ]    && missing="$missing AMAP"
  [ -z "${PINVOU3_BUILTIN_IWENCAI_KEY:-}" ] && missing="$missing IWENCAI"
  [ -z "${PINVOU3_BUILTIN_QCC_KEY:-}" ]     && missing="$missing QCC"
  if [ -n "$missing" ]; then
    echo "❌ $SECRETS_ENV 里这些 key 为空:$missing" >&2
    echo "   填好三个 key,或设 PINVOU3_SKIP_BUILTIN_SECRETS=1 显式发不带内置 key 的版本。" >&2
    exit 1
  fi
  echo "✓ 已加载内置共享 key(AMAP/IWENCAI/QCC),将编译进二进制"
else
  echo "❌ $SECRETS_ENV 不存在 —— 直接发版会静默发出「内置工具对新用户不可用」的坏包。" >&2
  echo "   从 scripts/.builtin-secrets.env.example 复制并填 key,或设 PINVOU3_SKIP_BUILTIN_SECRETS=1 显式跳过。" >&2
  exit 1
fi

# ── 2. 按 lockfile 安装前端工具链并构建 deb ───────────────────────
# Vite/React 已进入正式构建链；每次发布先 npm ci，避免旧 worktree 只有 Tauri CLI、
# 缺少 vite/react 等新增依赖时生成坏包或直接在 beforeBuildCommand 失败。
(cd "$APP_DIR" && npm ci --prefer-offline --no-audit && npm run build)

ARCH=$(dpkg --print-architecture 2>/dev/null || echo "amd64")
DEB="$APP_DIR/src-tauri/target/release/bundle/deb/pinvou3_${VERSION}_${ARCH}.deb"
if [ ! -f "$DEB" ]; then
  echo "deb 产物不存在: $DEB" >&2
  exit 1
fi

# ── 3. 生成 latest.json ───────────────────────────────────────────
SHA256=$(sha256sum "$DEB" | awk '{print $1}')
SIZE=$(stat -c%s "$DEB")
PUB_DATE=$(date -u +%FT%TZ)
TMP_JSON=$(mktemp)
jq -n \
  --arg version "$VERSION" --arg notes "$NOTES" --arg pub_date "$PUB_DATE" \
  --arg url "$BASE_URL/pinvou3_${VERSION}_${ARCH}.deb" --arg sha256 "$SHA256" \
  --argjson size "$SIZE" \
  '{version: $version, notes: $notes, pub_date: $pub_date, url: $url, sha256: $sha256, size: $size}' \
  > "$TMP_JSON"
echo "--- latest.json ---"
cat "$TMP_JSON"

# ── 4. 上传：先 deb 后 latest.json ────────────────────────────────
# 顺序关键:清单最后传,避免清单已指向新版而 deb 还没传完,客户端 404。
rsync -avz --progress "$DEB" "$SERVER:$REMOTE_DIR/"
rsync -avz "$TMP_JSON" "$SERVER:$REMOTE_DIR/latest.json"
ssh "$SERVER" "chmod 644 $REMOTE_DIR/latest.json"
rm -f "$TMP_JSON"

echo "=== 发布完成 ==="
echo "清单: $BASE_URL/latest.json"
curl -fsS "$BASE_URL/latest.json" | jq .version || echo "(线上验证失败,检查 nginx)"
