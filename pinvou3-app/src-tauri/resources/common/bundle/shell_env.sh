#!/usr/bin/env bash
# PINVOU exec_shell 的 CLI 兼容环境 hook。
#
# CodeWhale 的 shell_env hook 会把本脚本 stdout 中的 KEY=VALUE 合并到
# exec_shell 子进程。这里读取用户登录 shell 的环境，让桌面启动的 deb 也能获得
# 终端里的 PATH、桌面 IPC、代理、证书、SDK 和包管理器配置；同时过滤凭证、
# credential agent 与自动代码加载入口，避免模型通过 `env` 读取 PINVOU/MCP 密钥。

set -uo pipefail

login_shell="${SHELL:-/bin/sh}"
if [[ ! -x "$login_shell" ]]; then
    login_shell=/bin/sh
fi

# profile 偶尔会输出欢迎语；先写一个唯一 marker，让过滤器丢弃 marker 前噪声。
# env -0 避免普通空格、引号和等号破坏解析。最终只输出单行 KEY=VALUE，因为
# CodeWhale 的 shell_env 契约就是逐行解析。
"$login_shell" -lc 'printf "\0PINVOU3_SHELL_ENV_START\0"; env -0' | python3 -c '
import os
import re
import sys

marker = b"\0PINVOU3_SHELL_ENV_START\0"
raw = sys.stdin.buffer.read()
_, found, payload = raw.partition(marker)
if not found:
    raise SystemExit(0)

blocked_exact = {
    # 已登录凭据代理、cookie 与凭据配置入口
    "SSH_AUTH_SOCK", "SSH_AGENT_PID", "GPG_AGENT_INFO",
    "GNOME_KEYRING_CONTROL", "GNOME_KEYRING_PID", "KRB5CCNAME",
    "PULSE_COOKIE", "WAYLAND_SOCKET", "DOCKER_CONFIG", "KUBECONFIG",
    "AWS_SHARED_CREDENTIALS_FILE", "AWS_CONFIG_FILE", "NETRC",
    # shell/runtime 自动加载、命令替换和 askpass 入口
    "LD_PRELOAD", "LD_AUDIT", "DYLD_INSERT_LIBRARIES", "DYLD_LIBRARY_PATH",
    "BASH_ENV", "ENV", "PROMPT_COMMAND", "GIT_ASKPASS", "SSH_ASKPASS",
    "SUDO_ASKPASS", "GIT_SSH_COMMAND", "PYTHONPATH", "PYTHONHOME",
    "NODE_OPTIONS", "RUBYOPT", "PERL5OPT", "JAVA_TOOL_OPTIONS", "_JAVA_OPTIONS",
    # 常见可内嵌账号密码的包索引配置
    "PIP_INDEX_URL", "PIP_EXTRA_INDEX_URL", "UV_INDEX_URL",
    "UV_EXTRA_INDEX_URL", "NPM_TOKEN",
}

def secret_shaped(name: str) -> bool:
    upper = name.upper()
    return (
        upper.startswith("PINVOU3_MCP_SECRET_")
        or "API_KEY" in upper
        or "PRIVATE_KEY" in upper
        or "ACCESS_KEY" in upper
        or "SECRET" in upper
        or "PASSWORD" in upper
        or "PASSWD" in upper
        or "CREDENTIAL" in upper
        or upper == "TOKEN"
        or upper.startswith("TOKEN_")
        or upper.endswith("_TOKEN")
        or upper.endswith("_KEY")
    )

for entry in payload.split(b"\0"):
    if b"=" not in entry:
        continue
    raw_key, raw_value = entry.split(b"=", 1)
    key = os.fsdecode(raw_key)
    if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", key):
        continue
    upper = key.upper()
    if upper in blocked_exact or secret_shaped(upper):
        continue
    value = os.fsdecode(raw_value)
    if "\n" in value or "\r" in value:
        continue
    # URL userinfo 往往是代理/私有源密码；变量名不敏感也不能透传。
    if re.search(r"://[^/\s]*@", value):
        continue
    print(f"{key}={value}")
'
