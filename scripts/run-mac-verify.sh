#!/bin/bash
# Mac dev 验证脚本：编译 + 单测 + brew 依赖 + connector CLI 探测 + SenseVoice 入库校验。
# 不做 GUI 端到端(需手动)。集成到 mac-build.yml CI 末尾。
#
# 用法:
#   ./scripts/run-mac-verify.sh                 # 完整跑
#   ./scripts/run-mac-verify.sh --skip-test     # 跳过 cargo check/test(只跑探测 + sha256/plist/bundle 校验)
#
# 退出码:核心校验(sha256/provenance/plist/bundle targets)失败 → exit 1;
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
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"

if [ "$SKIP_TEST" -eq 0 ]; then
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
# 与 linux deb recommends 对齐:poppler(pdftotext)/pandoc/tesseract/python3/libreoffice/ffmpeg
# python3 接受任意 python@X.Y(brew list --formula | grep -q '^python@')
for pkg in poppler pandoc tesseract libreoffice ffmpeg; do
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

echo "=== 5. SenseVoice darwin-arm64 入库校验 ==="
ASR="$APP_SRC_TAURI/resources/asr/sense-voice-darwin-arm64"
ASR_LIC="$APP_SRC_TAURI/resources/asr/LICENSE-sense-voice-darwin-arm64"
# 入库二进制的 sha256(provenance 见 docs/fork-modifications.md T7 节)。若二进制重新编译,
# 同步更新此常量与文档,否则这里会报 ⚠ 不匹配 —— 这是预期:提醒重新编译要同步 provenance。
ASR_SHA256_EXPECTED="7cc7fc5c31d67b82df36d605c55db1abd685daa73180066afdc1b9d3324bd1b4"
if [ -f "$ASR" ] && file "$ASR" | grep -q "Mach-O.*arm64"; then
    echo "  ✓ SenseVoice darwin-arm64 就绪(Mach-O arm64)"
else
    echo "  ❌ SenseVoice darwin-arm64 缺失或非 Mach-O arm64" >&2
    VERIFY_FAIL=1
fi
if [ -f "$ASR" ]; then
    ASR_SHA256_ACTUAL=$(shasum -a 256 "$ASR" | awk '{print $1}')
    if [ "$ASR_SHA256_ACTUAL" = "$ASR_SHA256_EXPECTED" ]; then
        echo "  ✓ sha256 校验通过(匹配入库 provenance)"
    else
        # sha256 不匹配是核心 provenance 校验失败(二进制被篡改/重新编译未更新)→ 硬失败。
        echo "  ❌ sha256 不匹配:期望 $ASR_SHA256_EXPECTED" >&2
        echo "    实际 $ASR_SHA256_ACTUAL" >&2
        echo "    若重新编译了二进制,请同步更新本脚本 ASR_SHA256_EXPECTED 与 docs/fork-modifications.md T7 节" >&2
        VERIFY_FAIL=1
    fi
fi
if [ -f "$ASR_LIC" ]; then
    echo "  ✓ LICENSE 就绪"
else
    echo "  ⚠ LICENSE-sense-voice-darwin-arm64 缺失"
fi

echo "=== 6. tauri.conf.json macOS bundle 校验 ==="
# index() 返回 0(第一个元素)在 jq 中是 truthy(0 非 null/false),但用 != null 更明确。
if jq -e '(.bundle.targets | index("dmg") != null) and (.bundle.targets | index("app") != null)' "$APP_SRC_TAURI/tauri.conf.json" >/dev/null; then
    echo "  ✓ tauri.conf.json 已声明 app/dmg targets"
else
    echo "  ❌ tauri.conf.json 缺少 app/dmg targets" >&2
    VERIFY_FAIL=1
fi
if jq -e '.bundle.macOS.minimumSystemVersion == "14.0"' "$APP_SRC_TAURI/tauri.conf.json" >/dev/null; then
    echo "  ✓ minimumSystemVersion=14.0"
else
    echo "  ⚠ macOS.minimumSystemVersion 不是 14.0"
fi

echo "=== 7. Info.plist / entitlements.plist 存在校验 ==="
for f in Info.plist entitlements.plist; do
    p="$APP_SRC_TAURI/$f"
    if [ -f "$p" ] && plutil -lint "$p" >/dev/null 2>&1; then
        echo "  ✓ $f"
    else
        echo "  ❌ $f 缺失或格式错误" >&2
        VERIFY_FAIL=1
    fi
done

# CFBundleIdentifier 值必须与 tauri.conf.json identifier 一致(防 OTA 通道 bundle id 不匹配)。
BID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP_SRC_TAURI/Info.plist" 2>/dev/null || true)"
TID="$(jq -r .identifier "$APP_SRC_TAURI/tauri.conf.json" 2>/dev/null || true)"
if [ -z "$BID" ] || [ -z "$TID" ] || [ "$BID" != "$TID" ]; then
  echo "❌ CFBundleIdentifier($BID) ≠ tauri.conf.json identifier($TID)" >&2
  VERIFY_FAIL=1
fi

if [ "$VERIFY_FAIL" -ne 0 ]; then
    echo "=== ❌ 核心校验失败(详见上方 ❌ 标记)===" >&2
    exit 1
fi
echo "=== ✓ 完成 ==="
