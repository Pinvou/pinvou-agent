#!/usr/bin/env bash
# pinvou3 敏感目录硬拦截 hook
#
# DeepSeek-TUI 在 ToolCallBefore 事件 spawn 这个脚本，通过环境变量传入工具
# 调用参数。命中敏感关键词 → exit 1 → 上游拒绝该 tool 调用。
#
# 软引导（bundle/instructions.md）已经在 system prompt 里告诉 AI 不要碰这些
# 目录，但 prompt 不是 100% 可靠（Qwen3.6 偶尔会忽略）。这里是兜底硬墙。
#
# 触发命中后：上游将 tool 调用标记为失败，向 AI 回传错误，AI 收到反馈后
# 通常会改用别的路径或告诉用户。

set -uo pipefail

ARGS="${DEEPSEEK_TOOL_ARGS:-}"
TOOL="${DEEPSEEK_TOOL_NAME:-unknown}"

# 1) 路径关键词：直接命中 ~/.ssh/ 等目录路径
SENSITIVE_DIRS=(
    "/.ssh/"
    "/.gnupg/"
    "/.aws/"
    "/.docker/"
    "/.kube/"
    "/.config/google-chrome/"
    "/.mozilla/firefox/"
    "/.password-store/"
)

for pat in "${SENSITIVE_DIRS[@]}"; do
    if [[ "$ARGS" == *"$pat"* ]]; then
        echo "pinvou3-deny: tool '$TOOL' attempted to touch sensitive directory ($pat) — blocked" >&2
        exit 1
    fi
done

# 2) 文件名关键词：密钥/凭证常见命名
SENSITIVE_NAMES=(
    "id_rsa"
    "id_ed25519"
    "id_ecdsa"
    "id_dsa"
    "authorized_keys"
    ".pgp"
    ".gpg"
    "credentials"
    "secrets"
    "/.netrc"
    "/.git-credentials"
)

for kw in "${SENSITIVE_NAMES[@]}"; do
    if [[ "$ARGS" == *"$kw"* ]]; then
        echo "pinvou3-deny: tool '$TOOL' attempted to touch sensitive file ($kw) — blocked" >&2
        exit 1
    fi
done

# 3) 命令关键词：exec_shell 类工具的命令体里包含敏感操作
if [[ "$TOOL" == "exec_shell"* || "$TOOL" == "code_execution" ]]; then
    DANGEROUS_CMDS=(
        "cat ~/.ssh"
        "cat /etc/shadow"
        "cat /etc/sudoers"
        "ssh-keygen"
        "gpg --export-secret"
        "cat ~/.aws/credentials"
    )
    for dc in "${DANGEROUS_CMDS[@]}"; do
        if [[ "$ARGS" == *"$dc"* ]]; then
            echo "pinvou3-deny: '$TOOL' contains dangerous command pattern ($dc) — blocked" >&2
            exit 1
        fi
    done
fi

# 4) 超级权限关闭态拦 sudo：sudo 无免密会阻塞读密码，直到 exec_shell 超时（最长
#    10 分钟）才被 SIGKILL，体感卡死。源真相 = /etc/sudoers.d/pinvou3 是否存在
#    （超级权限开关用 pkexec 写/删它）。关闭态命中 sudo → 即时拒绝，让 AI 引导
#    用户去开开关，而不是卡到超时。开启态（NOPASSWD）放行，sudo 不会阻塞。
#    用词边界匹配 sudo 命令 token，避免误伤 "pseudo" 等含子串的词。
if [[ "$TOOL" == "exec_shell"* ]]; then
    if [[ "$ARGS" =~ (^|[^[:alnum:]_])sudo([^[:alnum:]_]|$) ]]; then
        if [[ ! -e /etc/sudoers.d/pinvou3 ]]; then
            echo "pinvou3-deny: 超级权限未开启，sudo 会阻塞到超时（不是真在执行）。已拦截。请去【设置 → 系统权限】打开开关后重试，或把命令贴出来自己跑。" >&2
            exit 1
        fi
    fi
fi

exit 0
