#!/bin/bash
# pinvou3 启动脚本 — Web UI 模式

export DEEPSEEK_API_KEY="ark-8533779c-9b24-4e9c-9d80-85f72d84b9e5-e60db"
export DEEPSEEK_BASE_URL="https://ark.cn-beijing.volces.com/api/coding/v3"
export DEEPSEEK_MODEL="deepseek-v3-2-251201"

cd "$(dirname "$0")"
exec cargo run --manifest-path pinvou-platform/Cargo.toml -- --apps-dir apps/ "$@"
