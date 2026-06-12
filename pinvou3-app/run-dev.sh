#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

# ── 同步开发源 workflows/ → bundle 编译快照 ──────────────────────
# 开发源(手写编辑都在这)与 resources/bundle/workflow/<wf>(include_dir! 编译嵌入源)
# 是两份;靠手动同步极易漂移(改完跑了还是旧 prompt)。这里每次启动自动同步,
# 配合 build.rs 的 BUNDLE_WORKFLOW_HASH_* 保证嵌入刷新(见 build.rs 注释)。
SANSHENG_WORKFLOW_SRC="../workflows/sansheng-liubu"
SANSHENG_ENGINE_SCRIPTS_SRC="../workflows/_engine/scripts"
SANSHENG_BUNDLE_WF_DST="src-tauri/resources/bundle/workflow/sansheng-liubu"
if [[ -d "$SANSHENG_WORKFLOW_SRC" && -d "$SANSHENG_BUNDLE_WF_DST" ]]; then
    if command -v rsync &>/dev/null; then
        echo "[run-dev] 同步 workflows/sansheng-liubu/ → bundle 快照"
        rsync -rc \
            --exclude='.git/' --exclude='.gitignore' \
            --exclude='*.env' --exclude='__pycache__/' \
            "$SANSHENG_WORKFLOW_SRC/" "$SANSHENG_BUNDLE_WF_DST/"
        mkdir -p "$SANSHENG_BUNDLE_WF_DST/scripts"
        # 引擎拷贝排除 test_*.py:测试不进发布包(include_dir 会原样嵌进二进制)
        for f in "$SANSHENG_ENGINE_SCRIPTS_SRC"/*.py; do
            case "$(basename "$f")" in test_*) ;; *) cp "$f" "$SANSHENG_BUNDLE_WF_DST/scripts/";; esac
        done
    else
        echo "[run-dev] rsync not found, skipping workflow sync (Windows)"
    fi
fi

# ── 工作流预检开关 ───────────────────────────────────────────────
# warmup_check.py 的 REQUIRED_ENVS 检查面向 env 配置;模型/搜索配置走
# settings.json 的部署下会误判 blocked,默认跳过。要启用预检 export 0。
export PINVOU3_SKIP_WARMUP="${PINVOU3_SKIP_WARMUP:-1}"

exec npx tauri dev "$@"
