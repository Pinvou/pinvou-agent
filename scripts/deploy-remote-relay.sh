#!/usr/bin/env bash
# 部署 PINVOU 手机远控 relay：本地验证 → 远端备份/原子替换 → 重启 → 公网验证。
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

node --check "$RELAY_DIR/server.js"
(cd "$RELAY_DIR" && npm test)

scp "$RELAY_DIR/server.js" "$SERVER:$REMOTE_SERVER_TMP"
scp "$RELAY_DIR/web/index.html" "$SERVER:$REMOTE_WEB_TMP"
scp "$RELAY_DIR/10-hardening.conf" "$SERVER:$REMOTE_HARDENING_TMP"

ssh "$SERVER" bash -s -- "$REMOTE_DIR" "$SERVICE" "$STAMP" "$REMOTE_SERVER_TMP" "$REMOTE_WEB_TMP" "$REMOTE_HARDENING_TMP" <<'REMOTE'
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

health="$(curl -fsS "$PUBLIC_URL/healthz")"
node -e 'const h=JSON.parse(process.argv[1]); if(!h.ok || !("room_count" in h) || "rooms" in h) process.exit(1)' "$health"
curl -fsSL "$PUBLIC_URL/r/deploy-check" | grep -q '远程连接已结束'
if curl --noproxy '*' -fsS --max-time 3 "$DIRECT_URL/healthz" >/dev/null 2>&1; then
  echo "部署失败：Relay 仍可通过公网端口直接访问：$DIRECT_URL" >&2
  exit 1
fi

echo "部署完成：$PUBLIC_URL"
echo "$health"
