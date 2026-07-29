#!/bin/bash
# Mac dev 验证脚本：编译 + 单测 + brew 依赖 + connector CLI 探测 + macOS bundle/plist 校验。
# 不做 GUI 端到端(需手动)。集成到 mac-build.yml CI 末尾。
#
# 用法:
#   ./scripts/run-mac-verify.sh                 # 完整跑
#   ./scripts/run-mac-verify.sh --skip-test     # 跳过 cargo check/test(只跑探测 + plist/bundle 校验)
#
# 退出码:核心校验(plist/bundle targets)失败 → exit 1;
# 非核心探测(brew/connector CLI 缺失)只 warn 不 fail。
set -uo pipefail

SKIP_TEST=0
for arg in "$@"; do
  case "$arg" in
    --skip-test) SKIP_TEST=1 ;;
    *) echo "未知参数: $arg" >&2; exit 2 ;;
  esac
done

# 积累错误:核心校验失败时置 1,脚本末尾 exit。
VERIFY_FAIL=0

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_SRC_TAURI="$REPO_ROOT/pinvou3-app/src-tauri"

# Mac 编译需要正确的部署目标(同 release-macos.sh)
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"

if [ "$SKIP_TEST" -eq 0 ]; then
  # cargo check/test 用 host 原生 aarch64(macos-15 runner 即 arm64)。
  # universal-apple-darwin 不是合法 cargo target(tauri build 内部双切片 + lipo 合成),
  # 这里只验 Rust 代码在 macOS 编译/测试通过;universal 打包产物的双切片校验见下方 section 7。
  echo "=== 1. cargo check aarch64-apple-darwin ==="
  cd "$APP_SRC_TAURI"
  if ! cargo check --target aarch64-apple-darwin --lib; then
    VERIFY_FAIL=1
  fi

  echo "=== 2. cargo test --lib (Mac native) ==="
  if ! cargo test --target aarch64-apple-darwin --lib -- --test-threads=1; then
    VERIFY_FAIL=1
  fi
else
  # --skip-test:上游 mac-build.yml 已跑 cargo check/test,这里不重复(省 ~10min)。
  # 后续 step 3-7 用绝对路径,不依赖此处 cwd。
  echo "=== 1/2. cargo check/test (skipped via --skip-test;上游 CI 已跑) ==="
fi

echo "=== 3. brew 依赖探测(文件 ingestion) ==="
# 与 docs/macos-requirements.md 外部依赖表对齐:poppler(pdftotext)/pandoc/tesseract/
# p7zip/python3/libreoffice。macOS 二期语音改走系统 Speech,已移除 ffmpeg(不再探测)。
# python3 接受任意 python@X.Y(brew list --formula | grep -q '^python@')
for pkg in poppler pandoc tesseract p7zip libreoffice; do
    if brew list "$pkg" >/dev/null 2>&1; then
        echo "  ✓ $pkg"
    else
        echo "  ⚠ $pkg 未安装(brew install $pkg)"
    fi
done
if brew list --formula 2>/dev/null | grep -q '^python@'; then
    PY_VER="$(brew list --formula 2>/dev/null | grep '^python@' | head -1)"
    echo "  ✓ $PY_VER"
elif command -v python3 >/dev/null 2>&1; then
    echo "  ✓ python3 ($(python3 --version 2>&1))"
else
    echo "  ⚠ python3 未安装(brew install python@3.13)"
fi

echo "=== 4. connector CLI 探测(npm 全局,Mac 与 Windows 同路径) ==="
for cli in lark-cli wecom-cli dws; do
    if command -v "$cli" >/dev/null 2>&1; then
        echo "  ✓ $cli"
    else
        echo "  ⚠ $cli 未安装(npm i -g @larksuite/cli @wecom/cli dingtalk-workspace-cli)"
    fi
done

echo "=== 5. tauri.conf.json macOS bundle 校验 ==="
# index() 返回 0(第一个元素)在 jq 中是 truthy(0 非 null/false),但用 != null 更明确。
MAC_OVERLAY="$APP_SRC_TAURI/config/platforms/macos/tauri.conf.json"
if jq -e '(.bundle.targets | index("dmg") != null) and (.bundle.targets | index("app") != null)' "$MAC_OVERLAY" >/dev/null; then
    echo "  ✓ tauri.conf.json 已声明 app/dmg targets"
else
    echo "  ❌ tauri.conf.json 缺少 app/dmg targets" >&2
    VERIFY_FAIL=1
fi
if jq -e '.bundle.macOS.minimumSystemVersion == "11.0"' "$MAC_OVERLAY" >/dev/null; then
    echo "  ✓ minimumSystemVersion=11.0"
else
    echo "  ⚠ macOS.minimumSystemVersion 不是 11.0"
fi
# Info.plist 的 LSMinimumSystemVersion 必须与 tauri.conf.json 一致(防二者漂移)。
PLIST_MIN_SYS="$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$APP_SRC_TAURI/packaging/macos/Info.plist" 2>/dev/null || true)"
if [ "$PLIST_MIN_SYS" = "11.0" ]; then
    echo "  ✓ Info.plist LSMinimumSystemVersion=11.0"
else
    echo "  ⚠ Info.plist LSMinimumSystemVersion=$PLIST_MIN_SYS(期望 11.0,与 tauri.conf.json 可能漂移)"
fi

echo "=== 6. Info.plist / entitlements.plist 存在校验 ==="
for f in Info.plist entitlements.plist; do
    p="$APP_SRC_TAURI/packaging/macos/$f"
    if [ -f "$p" ] && plutil -lint "$p" >/dev/null 2>&1; then
        echo "  ✓ $f"
    else
        echo "  ❌ $f 缺失或格式错误" >&2
        VERIFY_FAIL=1
    fi
done

# macOS 二期语音改走系统 Speech 框架,SFSpeechRecognizer 首次调用需 Info.plist 提供
# NSSpeechRecognitionUsageDescription(缺则 app 被 macOS 直接 terminate)。此处做静态
# 断言(CI/headless 友好,无需麦克风/授权),守住二期核心交付不回归。
# NSMicrophoneUsageDescription 同理:前端录音(WKWebView getUserMedia)首次需此 key。
for usage_key in NSSpeechRecognitionUsageDescription NSMicrophoneUsageDescription; do
    if /usr/libexec/PlistBuddy -c "Print :$usage_key" "$APP_SRC_TAURI/packaging/macos/Info.plist" >/dev/null 2>&1; then
        echo "  ✓ Info.plist 含 $usage_key"
    else
        echo "  ❌ Info.plist 缺 $usage_key(首次语音/录音会崩溃)" >&2
        VERIFY_FAIL=1
    fi
done

# CFBundleIdentifier 值必须与 tauri.conf.json identifier 一致(防 OTA 通道 bundle id 不匹配)。
BID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP_SRC_TAURI/packaging/macos/Info.plist" 2>/dev/null || true)"
TID="$(jq -r .identifier "$APP_SRC_TAURI/tauri.conf.json" 2>/dev/null || true)"
if [ -z "$BID" ] || [ -z "$TID" ] || [ "$BID" != "$TID" ]; then
  echo "❌ CFBundleIdentifier($BID) ≠ tauri.conf.json identifier($TID)" >&2
  VERIFY_FAIL=1
fi

echo "=== 7. universal 二进制双切片校验 (arm64 + x86_64) ==="
# 产物由 npx tauri build --target universal-apple-darwin 产出(tauri 内部双切片 + lipo 合成,
# 非 cargo target)。校验策略:产物存在时验两个切片齐全(核心硬失败);产物不存在只 warn ——
# verify --skip-test 常在未打包场景跑(本地 dev / 非 main 分支),硬失败会挡住所有未构建
# universal 的正常流程。main push 时 mac-build.yml 的 bundle smoke 产 universal 产物,
# 本校验在 verify 步骤即时激活;非 main/未打包场景 warn-only。
APP_BIN="$APP_SRC_TAURI/target/universal-apple-darwin/release/bundle/macos/pinvou3.app/Contents/MacOS/pinvou3-tauri"
if [ -f "$APP_BIN" ]; then
    LIPO_OUT="$(lipo -info "$APP_BIN" 2>&1 || true)"
    # fat 二进制输出形如 "Architectures in the fat file: ... are: arm64 x86_64"。
    if echo "$LIPO_OUT" | grep -q 'arm64' && echo "$LIPO_OUT" | grep -q 'x86_64'; then
        echo "  ✓ universal 双切片就绪 (arm64 + x86_64)"
        file "$APP_BIN"
    else
        echo "  ❌ 二进制架构不全(期望 arm64 + x86_64):$LIPO_OUT" >&2
        VERIFY_FAIL=1
    fi
else
    echo "  ⚠ universal 产物未构建: $APP_BIN"
    echo "    (需 npx tauri build --target universal-apple-darwin;main push 时 CI bundle smoke 产出后此校验自动激活)"
fi

if [ "$VERIFY_FAIL" -ne 0 ]; then
    echo "=== ❌ 核心校验失败(详见上方 ❌ 标记)===" >&2
    exit 1
fi
echo "=== ✓ 完成 ==="
