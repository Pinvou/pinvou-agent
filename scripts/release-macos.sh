#!/bin/bash
# pinvou3 macOS 发布脚本：构建 dmg → 合并 latest.json platforms.macos-arm64 → rsync 上传。
# 用法: ./scripts/release-macos.sh "本次更新说明"
#
# 发版前置:
#   1. bump 版本号(tauri.conf.json / Cargo.toml / package.json 三处,本脚本会校验一致)
#   2. resources/asr/sense-voice-darwin-arm64 已入库(Task 3.1 本机编译产物)
#   3. (可选)export MACOS_SIGNING_IDENTITY / APPLE_ID / APPLE_APP_SPECIFIC_PASSWORD / APPLE_TEAM_ID
#      未设则 Tauri bundler 用 tauri.conf.json 的 signingIdentity="-" 做 ad-hoc 签(可运行,
#      但用户首次需 xattr -dr com.apple.quarantine 才能过 Gatekeeper)。
#   4. ./scripts/release-macos.sh "修了 xxx"
#   5. 客户端 App 启动/手动检查即可看到新版
set -euo pipefail

NOTES="${1:-}"
if [ -z "$NOTES" ]; then
  echo "用法: $0 \"本次更新说明\"" >&2
  exit 1
fi

# ── 0. 平台 guard(脚本用 BSD stat/xattr/codesign,只在 macOS arm64 跑) ────
# 架构也校验:构建钉死 aarch64-apple-darwin(本机即 arm64),在 Intel Mac 上跑会
# 因 rustup 未装该 target 报错或走交叉编译产出意外结果。
if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
  echo "❌ 本脚本仅支持 macOS arm64(Apple Silicon)运行(用到 stat -f%z / xattr / codesign / notarytool + aarch64 target)" >&2
  exit 1
fi

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_DIR="$REPO_ROOT/pinvou3-app"
SERVER="admin@8.218.49.20"
REMOTE_DIR="/var/www/pinvou3"
BASE_URL="https://pinvou.com/pinvou3"
ARCH="aarch64"

# 部署目标:与 tauri.conf.json minimumSystemVersion / Info.plist LSMinimumSystemVersion
# 一致(14.0)。build.rs 不注入此变量;不显式 export 会用 rustc/链接器默认值(往往当前
# SDK 版本),产出要求更高 macOS 的二进制,与声明的 14.0 不符 → 14.x 用户打不开。
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"

# ── 1. 三处版本号一致校验（防版本漂移发出错包）────────────────────
# 版本提取加 `|| true`:源文件缺该行时 grep/jq 退出非 0,在 set -e 下会直接退出并
# 打印原始报错,而非下面更友好的"版本号不一致"信息(让校验逻辑接管)。
V_TAURI=$(jq -r .version "$APP_DIR/src-tauri/tauri.conf.json" || true)
V_CARGO=$(grep -m1 '^version = ' "$APP_DIR/src-tauri/Cargo.toml" | sed 's/version = "\(.*\)"/\1/' || true)
V_NPM=$(jq -r .version "$APP_DIR/package.json" || true)
if [ -z "$V_TAURI" ] || [ -z "$V_CARGO" ] || [ -z "$V_NPM" ]; then
  echo "❌ 版本号读取失败(源文件缺失或格式异常): tauri=$V_TAURI cargo=$V_CARGO npm=$V_NPM" >&2
  exit 1
fi
if [ "$V_TAURI" != "$V_CARGO" ] || [ "$V_TAURI" != "$V_NPM" ]; then
  echo "版本号不一致: tauri.conf.json=$V_TAURI Cargo.toml=$V_CARGO package.json=$V_NPM" >&2
  exit 1
fi
VERSION="$V_TAURI"
echo "=== 发布 pinvou3 v$VERSION (macOS arm64) ==="

# 签名/公证就绪判断(4 个凭证齐全),构建与公证步骤复用,避免条件判断重复导致不一致。
SIGN_READY=0
if [ -n "${MACOS_SIGNING_IDENTITY:-}" ] && [ -n "${APPLE_ID:-}" ] \
   && [ -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
  SIGN_READY=1
fi

# ── 2. SenseVoice darwin-arm64 前置校验 ────────────────────────────
ASR_BIN="$APP_DIR/src-tauri/resources/asr/sense-voice-darwin-arm64"
ASR_LICENSE="$APP_DIR/src-tauri/resources/asr/LICENSE-sense-voice-darwin-arm64"
if [ ! -f "$ASR_BIN" ]; then
  echo "❌ SenseVoice darwin-arm64 缺失: $ASR_BIN" >&2
  echo "   需本机交叉/本地编译 SenseVoice.cpp(-DBUILD_SHARED_LIBS=OFF,Metal 着色器内嵌),"
  echo "   产物为静态链接的 Mach-O arm64,放至上述路径并配 LICENSE。" >&2
  exit 1
fi
if ! file "$ASR_BIN" | grep -q "Mach-O.*arm64"; then
  echo "❌ $ASR_BIN 不是 Mach-O arm64" >&2
  exit 1
fi
if [ ! -f "$ASR_LICENSE" ]; then
  echo "❌ SenseVoice LICENSE 缺失: $ASR_LICENSE" >&2
  exit 1
fi
# 预清 quarantine xattr(防子进程被 Gatekeeper 拦)
xattr -dr com.apple.quarantine "$ASR_BIN" 2>/dev/null || true
xattr -dr com.apple.quarantine "$APP_DIR/src-tauri/resources/asr/" 2>/dev/null || true
echo "✓ SenseVoice darwin-arm64 校验通过"

# ── 3. 内置工具共享 key 注入（同 release-deb.sh）─────────────────
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

# ── 4. 构建 dmg ────────────────────────────────────────────────────
# 与 release-deb.sh 对齐:每次发布先 npm ci,避免新增前端依赖时生成坏包或 beforeBuildCommand 失败。
# Mac 构建钉死 aarch64-apple-darwin(本机即 arm64)。
# 若设了 MACOS_SIGNING_IDENTITY,转发给 Tauri bundler 让它在打包内部签 .app
# (codesign dmg 包装层不能让内部 .app 通过 notarytool)。
if [ "$SIGN_READY" = 1 ]; then
  export APPLE_SIGNING_IDENTITY="$MACOS_SIGNING_IDENTITY"
  echo "=== 将以 $MACOS_SIGNING_IDENTITY 签 .app(Tauri bundler 内联)==="
fi
(
  cd "$APP_DIR"
  npm ci --prefer-offline --no-audit
  npx tauri build --target aarch64-apple-darwin
)

DMG="$APP_DIR/src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/pinvou3_${VERSION}_${ARCH}.dmg"
if [ ! -f "$DMG" ]; then
  echo "dmg 产物不存在: $DMG" >&2
  exit 1
fi

# ── 5. 公证 + staple（条件触发；签名已在 build 阶段内联完成）──────
# 凭证就绪时 export 这 4 个 env 即可自动 notarytool + staple。
# 未设则发未签名 dmg,文档说明用户首次执行 xattr -dr com.apple.quarantine。
if [ "$SIGN_READY" = 1 ]; then
  echo "=== 公证 + staple (Developer ID: $MACOS_SIGNING_IDENTITY) ==="
  # notarytool 密码经 stdin 传入(避免 --password 明文出现在 ps 进程列表)。
  # --wait 可能因 Apple 服务异常无限期挂起;用 timeout 包一层(1 小时上限,
  # 正常公证 5-15 分钟,超时说明服务端异常)。macOS 自带无 timeout 命令(GNU coreutils),
  # 探测 timeout → gtimeout → 都没有则去掉超时(notarytool 自身也有重试/超时)。
  if command -v timeout >/dev/null 2>&1; then
    TIMEOUT_CMD="timeout 3600"
  elif command -v gtimeout >/dev/null 2>&1; then
    TIMEOUT_CMD="gtimeout 3600"
  else
    TIMEOUT_CMD=""
    echo "⚠️  未找到 timeout/gtimeout(brew install coreutils),公证无超时上限" >&2
  fi
  printf '%s' "$APPLE_APP_SPECIFIC_PASSWORD" | $TIMEOUT_CMD xcrun notarytool submit "$DMG" \
    --apple-id "$APPLE_ID" \
    --password - \
    --team-id "$APPLE_TEAM_ID" \
    --wait
  xcrun stapler staple "$DMG"
else
  echo "⚠️  未签名(缺 MACOS_SIGNING_IDENTITY/APPLE_ID/APPLE_APP_SPECIFIC_PASSWORD/APPLE_TEAM_ID env)" >&2
  echo "    dmg 用户首次需执行: xattr -dr com.apple.quarantine '/Applications/pinvou3.app'" >&2
fi

# ── 6. 合并 latest.json 的 platforms.macos-arm64 条目(保留顶层 Linux 字段) ──
# 字段策略(修复「Mac 发版破坏 Linux 客户端」):
# - 顶层 .version / .url / .sha256 / .size **一律不动**:顶层代表「最近一次 Linux 发版」,
#   旧 Linux 客户端(只读顶层)据此判断/下载。若 Mac 发版去 bump 顶层 .version 却保留旧
#   Linux deb 的 url,Linux 客户端会「看到新版 → 重复下载旧 deb」无限循环。
# - Mac 自己的版本写进 platforms["macos-arm64"].version;macos_update.rs 的 is_newer 读
#   本平台 version(为空才退顶层),Mac 客户端据此看到自己的新版,与 Linux 互不干扰。
# - platforms.linux-arm64/windows-x86_64 等其他平台字段不覆盖。
SHA256=$(shasum -a 256 "$DMG" | awk '{print $1}')
SIZE=$(stat -f%z "$DMG")
PUB_DATE=$(date -u +%FT%TZ)
DMG_NAME=$(basename "$DMG")
DMG_URL="$BASE_URL/$DMG_NAME"

# 拉远端 latest.json,合并 macos-arm64 条目,推回(不覆盖其他平台字段)。
TMP_JSON=$(mktemp)
TMP_JSON_NEW=$(mktemp)
TMP_ERR=$(mktemp)
trap 'rm -f "$TMP_JSON" "$TMP_JSON_NEW" "$TMP_ERR"' EXIT

# ⚠️ 此前的 `ssh cat ... || echo '{}'` 会把任何拉取失败(网络抖动/部分写出/ssh 中断/
# 权限不足)静默回退成空对象 {},随后 jq 合并只写 macos-arm64 → 顶层 url/sha256 与其它
# 平台(linux-arm64 等)全丢 → scp 推回 → Linux 客户端自动更新 404 直到下次 Linux 发版。
# 改为:先用 ssh test -f 判断文件是否存在;仅当文件确实不存在时用 {} 首发;cat 失败但
# 文件存在(权限/网络)→ 中止。
REMOTE_EXISTS=$(ssh "$SERVER" "test -f $REMOTE_DIR/latest.json && echo yes" 2>/dev/null || true)
if [ "$REMOTE_EXISTS" = "yes" ]; then
  if ! ssh "$SERVER" "cat $REMOTE_DIR/latest.json" >"$TMP_JSON" 2>"$TMP_ERR"; then
    echo "❌ 远端 latest.json 存在但读取失败(权限不足/网络中断?),中止以免破坏清单:" >&2
    cat "$TMP_ERR" >&2
    exit 1
  fi
  # 拉取成功:校验是合法 JSON 且保留顶层 url/sha256(旧 Linux 客户端依赖)。
  if ! jq -e . "$TMP_JSON" >/dev/null 2>&1; then
    echo "❌ 远端 latest.json 非合法 JSON,中止以免破坏清单:" >&2
    head -c 200 "$TMP_JSON" >&2
    exit 1
  fi
  if ! jq -e '.url and .sha256' "$TMP_JSON" >/dev/null 2>&1; then
    echo "⚠️  远端 latest.json 缺顶层 url/sha256(旧 Linux 客户端会无法下载 deb)" >&2
    echo "    若有意停发顶层字段请确认,否则中止并修复远端清单。" >&2
  fi
else
  echo "⚠️  远端 latest.json 不存在(首发场景),用空对象 {} 起步" >&2
  echo '{}' > "$TMP_JSON"
fi
jq --arg ver "$VERSION" --arg url "$DMG_URL" --arg sha "$SHA256" --arg size "$SIZE" \
   --arg date "$PUB_DATE" --arg notes "$NOTES" '
  # 顶层 .pub_date / .version / .url / .sha256 / .size **一律不动**:
  # 顶层代表最近一次 Linux 发版,旧 Linux 客户端据此判断/下载。Mac 发版只写 platforms 节。
  .platforms = (.platforms // {}) |
  .platforms["macos-arm64"] = {
    "version": $ver,
    "url": $url,
    "format": "dmg",
    "sha256": $sha,
    "size": ($size | tonumber),
    "restart_after_install": false,
    "notes": $notes,
    "pub_date": $date
  }' "$TMP_JSON" > "$TMP_JSON_NEW"

echo "--- latest.json (顶层 + macos-arm64 节) ---"
jq '{version, url, sha256, size, macos_arm64: .platforms["macos-arm64"], linux_arm64: .platforms["linux-arm64"]}' "$TMP_JSON_NEW"

# ── 7. 上传：先 dmg 后 latest.json ────────────────────────────────
# 顺序关键:清单最后传,避免清单已指向新版而 dmg 还没传完,客户端 404。
rsync -avz --progress "$DMG" "$SERVER:$REMOTE_DIR/"
# 原子上传清单:先传临时文件名 → 远端 mv 原子重命名,避免网络中断导致远端
# latest.json 被截断成半份坏 JSON(此时它已指向新 dmg)。
rsync -avz "$TMP_JSON_NEW" "$SERVER:$REMOTE_DIR/latest.json.new"
ssh "$SERVER" "mv '$REMOTE_DIR/latest.json.new' '$REMOTE_DIR/latest.json' && chmod 644 '$REMOTE_DIR/latest.json'"

echo "=== 发布完成 ==="
echo "DMG: $DMG_URL"
echo "清单: $BASE_URL/latest.json"
curl -fsS "$BASE_URL/latest.json" | jq '{version, macos_arm64: .platforms["macos-arm64"] | {url, sha256}}' || echo "(线上验证失败,检查 nginx)"
