#!/bin/bash
# pinvou3 macOS 发布脚本：构建 universal dmg → 合并 latest.json platforms.macos-universal + macos-arm64 → rsync 上传。
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

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_DIR="$REPO_ROOT/pinvou3-app"
SERVER="admin@8.218.49.20"
REMOTE_DIR="/var/www/pinvou3"
BASE_URL="https://pinvou.com/pinvou3"
ARCH="universal"

# 平台守卫:本脚本仅 macOS 可跑(universal dmg 打包依赖 macOS linker/hdiutil/codesign)。
# 在 Linux 上误跑会一路到 tauri build 才报底层链接错误,这里前置给出清晰提示。
if [ "$(uname -s)" != "Darwin" ]; then
  echo "❌ 本脚本仅 macOS 可跑(当前: $(uname -s))。Linux 发版请用 release-deb.sh。" >&2
  exit 1
fi

# universal 构建内部跑两次 cargo(aarch64 + x86_64)+ lipo 合成,故两个 rust target 都
# 必须已装。本地开发者首次发版若缺 x86_64-apple-darwin,会在构建中途以 E0463 报错退出;
# 这里前置预检 + 自动补装,给出更友好的 DX(同 CI 的双 target 安装步骤)。
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  if ! rustup target list --installed 2>/dev/null | grep -q "^$t "; then
    echo "⚠️  缺 rust target $t,自动安装(universal 构建需要双 target)..."
    rustup target add "$t"
  fi
done

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
echo "=== 发布 pinvou3 v$VERSION (macOS universal) ==="

# 签名/公证就绪判断(4 个凭证齐全),构建与公证步骤复用,避免条件判断重复导致不一致。
SIGN_READY=0
if [ -n "${MACOS_SIGNING_IDENTITY:-}" ] && [ -n "${APPLE_ID:-}" ] \
   && [ -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
  SIGN_READY=1
fi

# ── 2. SenseVoice darwin-arm64 前置校验 ────────────────────────────
ASR_BIN="$APP_DIR/src-tauri/resources/platforms/macos/aarch64/asr/sense-voice-darwin-arm64"
ASR_LICENSE="$APP_DIR/src-tauri/resources/platforms/macos/aarch64/asr/LICENSE-sense-voice-darwin-arm64"
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
xattr -dr com.apple.quarantine "$APP_DIR/src-tauri/resources/platforms/macos/aarch64/asr/" 2>/dev/null || true
echo "✓ SenseVoice darwin-arm64 校验通过"

# ── 4. 构建 dmg ────────────────────────────────────────────────────
# 与 release-deb.sh 对齐:每次发布先 npm ci,避免新增前端依赖时生成坏包或 beforeBuildCommand 失败。
# Mac 构建钉死 universal-apple-darwin(arm64 + x86_64 双切片,可在 Apple Silicon 与 Intel Mac 跑)。
# 若设了 MACOS_SIGNING_IDENTITY,转发给 Tauri bundler 让它在打包内部签 .app
# (codesign dmg 包装层不能让内部 .app 通过 notarytool)。
if [ "$SIGN_READY" = 1 ]; then
  export APPLE_SIGNING_IDENTITY="$MACOS_SIGNING_IDENTITY"
  echo "=== 将以 $MACOS_SIGNING_IDENTITY 签 .app(Tauri bundler 内联)==="
fi

# macOS 27+ 兼容 workaround(两个独立问题,均已在 macOS 27.0 arm64 实测复现并定位):
#
# 问题 A —— proc-macro dylib 无法加载(E0463 can't find crate):
#   release profile 默认 strip=debuginfo,rustc 剥离 debuginfo 后符号字符串表偏移未按
#   8 字节对齐;macOS 27 的 dyld 新增了对 LINKEDIT 字符串池 8 字节对齐的强制校验,
#   proc-macro dylib(编译在 host 侧)dlopen 被拒 → rustc 加载它报 E0463。
#   排除项:非 rustc 回归(1.96/1.97 均复现)、非链接器 bug(系统 ld / lld 均复现)。
#   修复:release profile 用 strip=none。host + target 两侧都要覆盖
#   (CARGO_PROFILE_RELEASE_STRIP 管 host 侧 proc-macro,RUSTFLAGS 管 target 侧 lib)。
#   副作用:二进制含 debuginfo 体积略大;macOS 27 上无其他绕过方式,等 rustc 上游修。
#   macOS ≤26 的 dyld 无此校验,保留默认 strip=debuginfo 以产出更小包。
MACOS_MAJOR=$(sw_vers -productVersion | cut -d. -f1)
if [ "$MACOS_MAJOR" -ge 27 ] 2>/dev/null; then
  export CARGO_PROFILE_RELEASE_STRIP=none
  # append 而非覆盖,保留调用者已设的 RUSTFLAGS
  export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C strip=none"
  echo "⚠️  macOS $MACOS_MAJOR+ → 注入 strip=none(绕过 dyld LINKEDIT 对齐校验)"
else
  echo "✓ macOS $MACOS_MAJOR,保留默认 strip=debuginfo"
fi

(
  cd "$APP_DIR"
  npm ci --prefer-offline --no-audit
  node scripts/tauri/build.js build --target universal-apple-darwin
)

DMG="$APP_DIR/src-tauri/target/universal-apple-darwin/release/bundle/dmg/pinvou3_${VERSION}_${ARCH}.dmg"
APP_BUNDLE="$APP_DIR/src-tauri/target/universal-apple-darwin/release/bundle/macos/pinvou3.app"

# dmg 打包偶发失败兜底:
#   Tauri 的 bundle_dmg.sh(基于 create-dmg)调 osascript 做 Finder 图标排版美化。
#   这一步偶尔会失败——通常是瞬时状态(残留的 rw 临时镜像未卸载干净 / Finder 进程
#   异常 / osascript 自动化权限首次未授权),而非 macOS 版本固有 bug(已在 macOS 27
#   实测:Tauri 正常构建时 osascript 美化完整生效,dmg 含 .DS_Store + .VolumeIcon.icns)。
#   策略:.app 已就绪但 dmg 缺失时,先重试 tauri 的 dmg bundling(跳过编译,只打包),
#   重试通常就能成功(osascript 偶发失败重跑一般就好);重试仍失败再退化手动 hdiutil
#   打包(丢 Finder 图标美化,但保证 dmg 可用)。
if [ ! -f "$DMG" ] && [ -d "$APP_BUNDLE" ]; then
  echo "⚠️  Tauri dmg 打包未产出 dmg,重试 dmg bundling(跳过编译,只打包)"
  (
    cd "$APP_DIR"
    node scripts/tauri/build.js build --target universal-apple-darwin --bundles dmg
  ) || echo "⚠️  重试仍失败,降级手动 hdiutil 打包"
fi

# 重试后仍无 dmg:手动 hdiutil 兜底(无 Finder 图标美化,但 dmg 功能完整)。
if [ ! -f "$DMG" ] && [ -d "$APP_BUNDLE" ]; then
  echo "⚠️  降级手动 hdiutil 打包(无 Finder 图标美化)"
  DMG_DIR="$(dirname "$DMG")"
  rm -f "$DMG_DIR"/rw.*.dmg 2>/dev/null || true
  TMP_DMG="$DMG_DIR/rw.fallback.dmg"
  if ! hdiutil create -ov -volname "pinvou3" -srcfolder "$APP_BUNDLE" \
       -fs HFS+ -format UDRW "$TMP_DMG" >/dev/null 2>&1; then
    echo "❌ hdiutil create 失败" >&2; exit 1
  fi
  DEV=$(hdiutil attach -readwrite -noverify -noautoopen "$TMP_DMG" 2>/dev/null \
        | grep -E '^/dev/' | sed '1q' | awk '{print $1}')
  [ -z "$DEV" ] && { echo "❌ hdiutil attach 失败" >&2; exit 1; }
  ln -sf /Applications "/Volumes/pinvou3/Applications" 2>/dev/null || true
  hdiutil detach "$DEV" >/dev/null 2>&1
  if ! hdiutil convert "$TMP_DMG" -format UDZO -imagekey zlib-level=9 -o "$DMG" >/dev/null 2>&1; then
    echo "❌ hdiutil convert 失败" >&2; exit 1
  fi
  rm -f "$TMP_DMG"
  echo "✓ 手动 dmg 打包完成(无 Finder 图标美化)"
fi

if [ ! -f "$DMG" ]; then
  echo "dmg 产物不存在: $DMG" >&2
  exit 1
fi

# ── 3.5 universal 双切片完整性校验 ──────────────────────────────────
# 防御性校验:确保产出的 .app 主二进制同时含 arm64 + x86_64 切片。
# 正常 tauri universal 构建在缺 target/切片时会硬失败,故此处触发概率低;但代价
# 非对称 —— 若因 bundler 异常产出 arm64-only 却退出 0,会被标 universal 上传,
# 导致 Intel 用户「校验通过地装上跑不起来的 app」。上传前硬校验,缺任一切片即中止。
APP_BIN="$APP_BUNDLE/Contents/MacOS/pinvou3"
if [ -f "$APP_BIN" ]; then
  LIPO_OUT="$(lipo -info "$APP_BIN" 2>&1 || true)"
  if echo "$LIPO_OUT" | grep -q 'arm64' && echo "$LIPO_OUT" | grep -q 'x86_64'; then
    echo "✓ universal 双切片就绪 (arm64 + x86_64)"
  else
    echo "❌ 产物非 universal(缺 arm64 或 x86_64 切片):$LIPO_OUT" >&2
    echo "   检查 aarch64-apple-darwin / x86_64-apple-darwin 两个 target 是否都已安装。" >&2
    exit 1
  fi
else
  echo "⚠️  $APP_BIN 不存在,跳过双切片校验(继续,后续公证/上传可能失败)" >&2
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

# ── 6. 合并 latest.json 的 platforms.macos-universal + macos-arm64 条目(保留顶层 Linux 字段) ──
# 字段策略(修复「Mac 发版破坏 Linux 客户端」):
# - 顶层 .version / .url / .sha256 / .size **一律不动**:顶层代表「最近一次 Linux 发版」,
#   旧 Linux 客户端(只读顶层)据此判断/下载。若 Mac 发版去 bump 顶层 .version 却保留旧
#   Linux deb 的 url,Linux 客户端会「看到新版 → 重复下载旧 deb」无限循环。
# - Mac 自己的版本写进 platforms["macos-universal"] 与 platforms["macos-arm64"].version;
#   macos_update.rs 的 is_newer 读本平台 version(为空才退顶层),Mac 客户端据此看到自己的新版,与 Linux 互不干扰。
#   macos-arm64 为旧 arm64-only 客户端向后兼容(指向同一 universal dmg,含 arm64 切片可跑)。
# - platforms.linux-arm64/windows-x86_64 等其他平台字段不覆盖。
SHA256=$(shasum -a 256 "$DMG" | awk '{print $1}')
SIZE=$(stat -f%z "$DMG")
PUB_DATE=$(date -u +%FT%TZ)
DMG_NAME=$(basename "$DMG")
DMG_URL="$BASE_URL/$DMG_NAME"

# 拉远端 latest.json,合并 macos-universal + macos-arm64 条目,推回(不覆盖其他平台字段)。
TMP_JSON=$(mktemp)
TMP_JSON_NEW=$(mktemp)
TMP_ERR=$(mktemp)
trap 'rm -f "$TMP_JSON" "$TMP_JSON_NEW" "$TMP_ERR"' EXIT

# ⚠️ 此前的 `ssh cat ... || echo '{}'` 会把任何拉取失败(网络抖动/部分写出/ssh 中断/
# 权限不足)静默回退成空对象 {},随后 jq 合并只写 macos-arm64 → 顶层 url/sha256 与其它
# 平台(linux-arm64 等)全丢 → scp 推回 → Linux 客户端自动更新 404 直到下次 Linux 发版。
# 改为:SSH 探测本身失败立即中止;仅当远端明确返回 missing 时用 {} 首发;cat 失败但
# 文件存在(权限/网络)同样中止。
if ! REMOTE_STATE=$(ssh "$SERVER" \
  "if [ -f '$REMOTE_DIR/latest.json' ]; then printf '%s\\n' exists; else printf '%s\\n' missing; fi" \
  2>"$TMP_ERR"); then
  echo "❌ 无法探测远端 latest.json(SSH/权限/网络异常),中止发布:" >&2
  cat "$TMP_ERR" >&2
  exit 1
fi
if [ "$REMOTE_STATE" = "exists" ]; then
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
elif [ "$REMOTE_STATE" = "missing" ]; then
  echo "⚠️  远端 latest.json 不存在(首发场景),用空对象 {} 起步" >&2
  echo '{}' > "$TMP_JSON"
else
  echo "❌ 远端 latest.json 探测返回异常结果: $REMOTE_STATE" >&2
  exit 1
fi
jq --arg ver "$VERSION" --arg url "$DMG_URL" --arg sha "$SHA256" --arg size "$SIZE" \
   --arg date "$PUB_DATE" --arg notes "$NOTES" '
  # 顶层 .pub_date / .version / .url / .sha256 / .size **一律不动**:
  # 顶层代表最近一次 Linux 发版,旧 Linux 客户端据此判断/下载。Mac 发版只写 platforms 节。
  .platforms = (.platforms // {}) |
  # 新 universal 客户端读 macos-universal;旧 arm64-only 客户端读 macos-arm64。
  # 两通道指向同一 universal dmg(universal 含 arm64 切片,旧 arm64 客户端装了也能跑)。
  (
    {
      "version": $ver,
      "url": $url,
      "format": "dmg",
      "sha256": $sha,
      "size": ($size | tonumber),
      "restart_after_install": false,
      "notes": $notes,
      "pub_date": $date
    }
  ) as $entry |
  .platforms["macos-universal"] = $entry |
  .platforms["macos-arm64"] = $entry' "$TMP_JSON" > "$TMP_JSON_NEW"

echo "--- latest.json (顶层 + macos-universal/macos-arm64 节) ---"
jq '{version, url, sha256, size, macos_universal: .platforms["macos-universal"], macos_arm64: .platforms["macos-arm64"], linux_arm64: .platforms["linux-arm64"]}' "$TMP_JSON_NEW"

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
curl -fsS "$BASE_URL/latest.json" | jq '{version, macos_universal: .platforms["macos-universal"] | {url, sha256}, macos_arm64: .platforms["macos-arm64"] | {url, sha256}}' || echo "(线上验证失败,检查 nginx)"
