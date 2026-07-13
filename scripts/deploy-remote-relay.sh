#!/usr/bin/env bash
# 部署 PINVOU 手机远控 relay：公网基线 → 本地验证 → 远端备份/原子替换 → 公网验证/失败回滚。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RELAY_DIR="$ROOT/remote-control-relay"
SERVER="${PINVOU_REMOTE_DEPLOY_SERVER:-root@47.120.8.237}"
REMOTE_DIR="${PINVOU_REMOTE_DEPLOY_DIR:-/opt/pinvou-remote-relay}"
SERVICE="${PINVOU_REMOTE_DEPLOY_SERVICE:-pinvou-remote-relay.service}"
PUBLIC_URL="${PINVOU_REMOTE_PUBLIC_URL:-https://www.ma-xiao.com/pinvou3/remote}"
DIRECT_URL="${PINVOU_REMOTE_DIRECT_URL:-http://47.120.8.237:8787/pinvou3/remote}"
STAMP="$(date +%Y%m%d-%H%M%S)"
REMOTE_SERVER_TMP="/tmp/pinvou-remote-server-$STAMP.js"
REMOTE_WEB_TMP="/tmp/pinvou-remote-web-$STAMP.html"
REMOTE_HARDENING_TMP="/tmp/pinvou-remote-hardening-$STAMP.conf"
VERIFY_ERROR=""
LAST_HEALTH=""

verify_public() {
  local expected="$1"
  local health page
  if ! health="$(curl -fsS "$PUBLIC_URL/healthz")"; then
    VERIFY_ERROR="公网健康检查失败：$PUBLIC_URL/healthz"
    return 1
  fi
  if ! node -e 'const h=JSON.parse(process.argv[1]); if(!h.ok || !("room_count" in h) || "rooms" in h) process.exit(1)' "$health"; then
    VERIFY_ERROR="公网健康检查返回格式不符合预期"
    return 1
  fi
  if ! page="$(curl -fsSL "$PUBLIC_URL/r/deploy-check")"; then
    VERIFY_ERROR="手机页面检查失败：$PUBLIC_URL/r/deploy-check"
    return 1
  fi
  if [[ "$expected" == "release" ]]; then
    if [[ "$page" != *'<meta name="pinvou-remote-client" content="1" />'* ]]; then
      VERIFY_ERROR="手机页面未命中新版本标识"
      return 1
    fi
  elif [[ "$page" != *'<title>PINVOU Remote</title>'* ]]; then
    VERIFY_ERROR="手机页面未命中基础页面标识"
    return 1
  fi
  if curl --noproxy '*' -fsS --max-time 3 "$DIRECT_URL/healthz" >/dev/null 2>&1; then
    VERIFY_ERROR="Relay 仍可通过公网端口直接访问：$DIRECT_URL"
    return 1
  fi
  LAST_HEALTH="$health"
  VERIFY_ERROR=""
}

if ! verify_public baseline; then
  echo "部署前检查失败，未修改线上版本：$VERIFY_ERROR" >&2
  exit 1
fi

node --check "$RELAY_DIR/server.js"
(cd "$RELAY_DIR" && npm test)

scp "$RELAY_DIR/server.js" "$SERVER:$REMOTE_SERVER_TMP"
scp "$RELAY_DIR/web/index.html" "$SERVER:$REMOTE_WEB_TMP"
scp "$RELAY_DIR/10-hardening.conf" "$SERVER:$REMOTE_HARDENING_TMP"

deploy_output="$(ssh "$SERVER" bash -s -- "$REMOTE_DIR" "$SERVICE" "$STAMP" "$REMOTE_SERVER_TMP" "$REMOTE_WEB_TMP" "$REMOTE_HARDENING_TMP" <<'REMOTE'
set -euo pipefail
remote_dir="$1"
service="$2"
stamp="$3"
server_tmp="$4"
web_tmp="$5"
hardening_tmp="$6"
backup="$remote_dir/backups/$stamp"
dropin_dir="/etc/systemd/system/${service}.d"
dropin="$dropin_dir/10-hardening.conf"

node --check "$server_tmp"
mkdir -p "$backup"
cp -a "$remote_dir/server.js" "$backup/server.js"
cp -a "$remote_dir/web/index.html" "$backup/index.html"
mkdir -p "$dropin_dir"
if [[ -f "$dropin" ]]; then
  cp -a "$dropin" "$backup/10-hardening.conf"
else
  touch "$backup/no-hardening-dropin"
fi

rollback() {
  cp -a "$backup/server.js" "$remote_dir/server.js"
  cp -a "$backup/index.html" "$remote_dir/web/index.html"
  if [[ -f "$backup/no-hardening-dropin" ]]; then
    rm -f "$dropin"
  else
    cp -a "$backup/10-hardening.conf" "$dropin"
  fi
  systemctl daemon-reload
  systemctl restart "$service"
}

chown root:root "$server_tmp" "$web_tmp" "$hardening_tmp"
chmod 644 "$server_tmp" "$web_tmp" "$hardening_tmp"
mv "$server_tmp" "$remote_dir/server.js"
mv "$web_tmp" "$remote_dir/web/index.html"
mv "$hardening_tmp" "$dropin"
systemctl daemon-reload

if ! systemctl restart "$service" || ! systemctl is-active --quiet "$service"; then
  rollback
  echo "部署失败，已从 $backup 回滚" >&2
  exit 1
fi

sleep 1
if ! curl -fsS http://127.0.0.1:8787/pinvou3/remote/healthz >/dev/null; then
  rollback
  echo "健康检查失败，已从 $backup 回滚" >&2
  exit 1
fi
echo "backup=$backup"
REMOTE
)"
echo "$deploy_output"

backup="$REMOTE_DIR/backups/$STAMP"

rollback_remote() {
  ssh "$SERVER" bash -s -- "$REMOTE_DIR" "$SERVICE" "$backup" <<'REMOTE'
set -euo pipefail
remote_dir="$1"
service="$2"
backup="$3"

case "$backup" in
  "$remote_dir"/backups/*) ;;
  *) echo "拒绝未知备份目录：$backup" >&2; exit 1 ;;
esac
test -f "$backup/server.js"
test -f "$backup/index.html"
cp -a "$backup/server.js" "$remote_dir/server.js"
cp -a "$backup/index.html" "$remote_dir/web/index.html"
if [[ -f "$backup/no-hardening-dropin" ]]; then
  rm -f "/etc/systemd/system/${service}.d/10-hardening.conf"
else
  test -f "$backup/10-hardening.conf"
  cp -a "$backup/10-hardening.conf" "/etc/systemd/system/${service}.d/10-hardening.conf"
fi
systemctl daemon-reload
systemctl restart "$service"
systemctl is-active --quiet "$service"
curl -fsS http://127.0.0.1:8787/pinvou3/remote/healthz >/dev/null
echo "已恢复备份：$backup"
REMOTE
}

if ! verify_public release; then
  deploy_error="$VERIFY_ERROR"
  echo "部署后检查失败，开始回滚：$deploy_error" >&2
  if ! rollback_remote; then
    echo "严重：自动回滚失败，请立即检查 $SERVER 上的 $SERVICE" >&2
    exit 1
  fi
  if ! verify_public baseline; then
    echo "严重：备份已恢复，但回滚后的公网复检失败：$VERIFY_ERROR" >&2
    exit 1
  fi
  echo "部署失败，已恢复并验证上一线上版本：$deploy_error" >&2
  exit 1
fi

echo "部署完成：$PUBLIC_URL"
echo "$LAST_HEALTH"
