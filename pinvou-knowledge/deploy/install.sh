#!/usr/bin/env bash

set -Eeuo pipefail

SERVICE_NAME="pinvou-knowledge.service"
SERVICE_USER="pinvou-knowledge"
SERVICE_GROUP="pinvou-knowledge"
DATA_DIR="/var/lib/pinvou-knowledge"
DATA_LOCK="/var/lib/.pinvou-knowledge.data.lock"
INSTALL_BIN="/usr/local/bin/pinvou-knowledge-server"
INSTALL_UNIT="/etc/systemd/system/${SERVICE_NAME}"
BUILD_JOBS="${PINVOU_KNOWLEDGE_BUILD_JOBS:-2}"

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
KNOWLEDGE_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd -P)"
BINARY="${KNOWLEDGE_DIR}/target/release/pinvou-knowledge-server"
UNIT_FILE="${SCRIPT_DIR}/pinvou-knowledge.service"

log() {
  printf '\n==> %s\n' "$*"
}

fail() {
  printf '\n部署失败：%s\n' "$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "缺少命令：$1"
}

run_root() {
  if ((EUID == 0)); then
    "$@"
  else
    sudo "$@"
  fi
}

if [[ "$(uname -s)" != "Linux" ]]; then
  fail "该脚本仅用于 Linux systemd 主机"
fi

if [[ -d "${HOME}/.cargo/bin" ]]; then
  export PATH="${HOME}/.cargo/bin:${PATH}"
fi

for command_name in cargo rustc systemctl install getent; do
  require_command "${command_name}"
done
if ((EUID != 0)); then
  require_command sudo
fi

[[ "${BUILD_JOBS}" =~ ^[1-9][0-9]*$ ]] ||
  fail "PINVOU_KNOWLEDGE_BUILD_JOBS 必须是正整数"

rust_version="$(rustc --version | awk '{print $2}')"
rust_major="${rust_version%%.*}"
rust_remainder="${rust_version#*.}"
rust_minor="${rust_remainder%%.*}"
if ((rust_major < 1 || (rust_major == 1 && rust_minor < 89))); then
  fail "需要 Rust 1.89 或更高版本，当前为 ${rust_version}。请先执行 source ~/.cargo/env"
fi

log "使用 $(rustc --version) / $(cargo --version)"
log "编译 PINVOU Knowledge（并行任务：${BUILD_JOBS}）"
cargo build \
  --locked \
  -j "${BUILD_JOBS}" \
  --manifest-path "${KNOWLEDGE_DIR}/Cargo.toml" \
  --release

[[ -x "${BINARY}" ]] || fail "编译完成但未找到服务端二进制：${BINARY}"
[[ -f "${UNIT_FILE}" ]] || fail "未找到 systemd 模板：${UNIT_FILE}"

log "申请管理员权限并安装系统服务"
run_root true

if ! getent group "${SERVICE_GROUP}" >/dev/null; then
  run_root groupadd --system "${SERVICE_GROUP}"
fi

if ! getent passwd "${SERVICE_USER}" >/dev/null; then
  run_root useradd \
    --system \
    --gid "${SERVICE_GROUP}" \
    --home-dir "${DATA_DIR}" \
    --shell /usr/sbin/nologin \
    "${SERVICE_USER}"
fi

run_root install -d \
  -m 0700 \
  -o "${SERVICE_USER}" \
  -g "${SERVICE_GROUP}" \
  "${DATA_DIR}"
if [[ -e "${DATA_LOCK}" || -L "${DATA_LOCK}" ]]; then
  [[ -f "${DATA_LOCK}" && ! -L "${DATA_LOCK}" ]] || fail "共享知识库数据锁不是安全的普通文件"
  data_lock_uid="$(stat -c %u "${DATA_LOCK}")"
  [[ "${data_lock_uid}" == "$(id -u "${SERVICE_USER}")" ]] || fail "共享知识库数据锁所有者不安全"
  run_root chmod 0600 "${DATA_LOCK}"
else
  run_root install -m 0600 -o "${SERVICE_USER}" -g "${SERVICE_GROUP}" /dev/null "${DATA_LOCK}"
fi
run_root install -m 0755 "${BINARY}" "${INSTALL_BIN}.new"
run_root mv -f "${INSTALL_BIN}.new" "${INSTALL_BIN}"
run_root install -m 0644 "${UNIT_FILE}" "${INSTALL_UNIT}.new"
run_root mv -f "${INSTALL_UNIT}.new" "${INSTALL_UNIT}"

run_root systemctl daemon-reload
run_root systemctl enable "${SERVICE_NAME}" >/dev/null
run_root systemctl restart "${SERVICE_NAME}"

log "等待服务启动"
for _ in $(seq 1 30); do
  if systemctl is-active --quiet "${SERVICE_NAME}" &&
    "${INSTALL_BIN}" --health-check https://127.0.0.1:3210 >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

if ! systemctl is-active --quiet "${SERVICE_NAME}"; then
  run_root journalctl -u "${SERVICE_NAME}" -n 50 --no-pager >&2 || true
  fail "systemd 服务未能启动"
fi

if ! "${INSTALL_BIN}" --health-check https://127.0.0.1:3210 >/dev/null 2>&1; then
  run_root journalctl -u "${SERVICE_NAME}" -n 50 --no-pager >&2 || true
  fail "服务已运行，但健康检查失败"
fi

tailscale_ip=""
if command -v tailscale >/dev/null 2>&1; then
  tailscale_ip="$(tailscale ip -4 2>/dev/null | head -n 1 || true)"
fi

log "部署完成（仅供开发调试）"
printf '服务状态：%s\n' "$(systemctl is-active "${SERVICE_NAME}")"
printf '本机 API：https://127.0.0.1:3210/api/v1/info\n'
if [[ -n "${tailscale_ip}" ]]; then
  printf 'Tailscale API：https://%s:3210/api/v1/info\n' "${tailscale_ip}"
fi

if run_root test -f "${DATA_DIR}/host-owner.claim"; then
  printf '\n待 Pinvou 安全领取的本机所有者凭据：%s\n' "${DATA_DIR}/host-owner.claim"
  printf '请优先通过 Linux 版 Pinvou 的“在本机创建”流程安装，不要手工复制该文件。\n'
else
  printf '\n本机所有者凭据已被领取。\n'
fi
