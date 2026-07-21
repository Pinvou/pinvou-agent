#!/usr/bin/env bash
# LEGACY：仅把已经退役的 v1 页面临时放入 Relay 当前实际服务的 web/dist/index.html。
# 完整 WebUI 的正式发布方案尚待项目发起者确认；在此之前不要用本脚本发布 WebUI。
# 旧流程：公网基线 → 本地验证 → 远端备份/原子替换 → 公网验证/失败回滚。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [[ "${PINVOU_ALLOW_LEGACY_REMOTE_DEPLOY:-0}" != "1" ]]; then
  echo "Refusing to deploy the retired v1 remote page." >&2
  echo "The WebUI v2 production release plan is intentionally not enabled yet." >&2
  echo "Set PINVOU_ALLOW_LEGACY_REMOTE_DEPLOY=1 only for an explicitly approved legacy rollback." >&2
  exit 64
fi

RELAY_DIR="$ROOT/remote-control-relay"
SERVER="${PINVOU_REMOTE_DEPLOY_SERVER:-root@47.120.8.237}"
REMOTE_DIR="${PINVOU_REMOTE_DEPLOY_DIR:-/opt/pinvou-remote-relay}"
SERVICE="${PINVOU_REMOTE_DEPLOY_SERVICE:-pinvou-remote-relay.service}"
PUBLIC_URL="${PINVOU_REMOTE_PUBLIC_URL:-https://pinvou.com/pinvou3/remote}"
DIRECT_URL="${PINVOU_REMOTE_DIRECT_URL:-http://47.120.8.237:8787/pinvou3/remote}"
STAMP="$(date +%Y%m%d-%H%M%S)"
REMOTE_SERVER_TMP="/tmp/pinvou-remote-server-$STAMP.js"
REMOTE_TELEMETRY_TMP="/tmp/pinvou-telemetry-service-$STAMP.js"
REMOTE_WEB_TMP="/tmp/pinvou-remote-web-$STAMP.html"
REMOTE_STATS_TMP="/tmp/pinvou-stats-$STAMP.html"
REMOTE_HARDENING_TMP="/tmp/pinvou-remote-hardening-$STAMP.conf"
REMOTE_PACKAGE_TMP="/tmp/pinvou-remote-package-$STAMP.json"
REMOTE_LOCK_TMP="/tmp/pinvou-remote-package-lock-$STAMP.json"
REMOTE_IPV4_DB_TMP="/tmp/pinvou-ip2region-v4-$STAMP.xdb"
REMOTE_IPV6_DB_TMP="/tmp/pinvou-ip2region-v6-$STAMP.xdb"
IP_DB_CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/pinvou-deploy"
IPV4_DB_CACHE="$IP_DB_CACHE_DIR/ip2region-v4-v3.17.0.xdb"
IPV6_DB_CACHE="$IP_DB_CACHE_DIR/ip2region-v6-v3.17.0.xdb"
IPV4_DB_URL="https://raw.githubusercontent.com/lionsoul2014/ip2region/v3.17.0/data/ip2region_v4.xdb"
IPV6_DB_URL="https://raw.githubusercontent.com/lionsoul2014/ip2region/v3.17.0/data/ip2region_v6.xdb"
IPV4_DB_SHA256="6307a9696f5711f84bcb8b25f07894de68a64a0ed4a1cc7e990562dd3084f210"
IPV6_DB_SHA256="5b93da35ac28bc316dccc54a758381f7a874ae0461dd51ff5df5e34815586f11"
VERIFY_ERROR=""
LAST_HEALTH=""

ensure_cached_db() {
  local path="$1"
  local url="$2"
  local expected="$3"
  local temporary="${path}.download"
  if [[ -f "$path" ]] && echo "$expected  $path" | sha256sum -c --status; then
    return
  fi
  mkdir -p "$IP_DB_CACHE_DIR"
  rm -f "$temporary"
  curl -fL --retry 3 --connect-timeout 10 --max-time 180 -o "$temporary" "$url"
  echo "$expected  $temporary" | sha256sum -c --status
  mv "$temporary" "$path"
}

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
ensure_cached_db "$IPV4_DB_CACHE" "$IPV4_DB_URL" "$IPV4_DB_SHA256"
ensure_cached_db "$IPV6_DB_CACHE" "$IPV6_DB_URL" "$IPV6_DB_SHA256"

scp "$RELAY_DIR/server.js" "$SERVER:$REMOTE_SERVER_TMP"
scp "$RELAY_DIR/telemetry-service.js" "$SERVER:$REMOTE_TELEMETRY_TMP"
scp "$RELAY_DIR/web/index.html" "$SERVER:$REMOTE_WEB_TMP"
scp "$RELAY_DIR/web/stats.html" "$SERVER:$REMOTE_STATS_TMP"
scp "$RELAY_DIR/10-hardening.conf" "$SERVER:$REMOTE_HARDENING_TMP"
scp "$RELAY_DIR/package.json" "$SERVER:$REMOTE_PACKAGE_TMP"
scp "$RELAY_DIR/package-lock.json" "$SERVER:$REMOTE_LOCK_TMP"
scp "$IPV4_DB_CACHE" "$SERVER:$REMOTE_IPV4_DB_TMP"
scp "$IPV6_DB_CACHE" "$SERVER:$REMOTE_IPV6_DB_TMP"

deploy_output="$(ssh "$SERVER" bash -s -- "$REMOTE_DIR" "$SERVICE" "$STAMP" "$REMOTE_SERVER_TMP" "$REMOTE_TELEMETRY_TMP" "$REMOTE_WEB_TMP" "$REMOTE_STATS_TMP" "$REMOTE_HARDENING_TMP" "$REMOTE_PACKAGE_TMP" "$REMOTE_LOCK_TMP" "$REMOTE_IPV4_DB_TMP" "$REMOTE_IPV6_DB_TMP" <<'REMOTE'
set -euo pipefail
remote_dir="$1"
service="$2"
stamp="$3"
server_tmp="$4"
telemetry_tmp="$5"
web_tmp="$6"
stats_tmp="$7"
hardening_tmp="$8"
package_tmp="$9"
lock_tmp="${10}"
ipv4_tmp="${11}"
ipv6_tmp="${12}"
backup="$remote_dir/backups/$stamp"
dropin_dir="/etc/systemd/system/${service}.d"
dropin="$dropin_dir/10-hardening.conf"
telemetry_data_dir="/var/lib/pinvou-telemetry"
ipv4_db="$telemetry_data_dir/ip2region_v4.xdb"
ipv6_db="$telemetry_data_dir/ip2region_v6.xdb"
ipv4_sha256="6307a9696f5711f84bcb8b25f07894de68a64a0ed4a1cc7e990562dd3084f210"
ipv6_sha256="5b93da35ac28bc316dccc54a758381f7a874ae0461dd51ff5df5e34815586f11"

node --check "$server_tmp"
node --check "$telemetry_tmp"
mkdir -p "$backup"
cp -a "$remote_dir/server.js" "$backup/server.js"
cp -a "$remote_dir/web/dist/index.html" "$backup/index.html"
cp -a "$remote_dir/package.json" "$backup/package.json"
cp -a "$remote_dir/package-lock.json" "$backup/package-lock.json"
if [[ -f "$remote_dir/telemetry-service.js" ]]; then
  cp -a "$remote_dir/telemetry-service.js" "$backup/telemetry-service.js"
else
  touch "$backup/no-telemetry-service"
fi
if [[ -f "$remote_dir/web/stats.html" ]]; then
  cp -a "$remote_dir/web/stats.html" "$backup/stats.html"
else
  touch "$backup/no-stats-html"
fi
mkdir -p "$dropin_dir"
if [[ -f "$dropin" ]]; then
  cp -a "$dropin" "$backup/10-hardening.conf"
else
  touch "$backup/no-hardening-dropin"
fi

rollback() {
  cp -a "$backup/server.js" "$remote_dir/server.js"
  cp -a "$backup/index.html" "$remote_dir/web/dist/index.html"
  cp -a "$backup/package.json" "$remote_dir/package.json"
  cp -a "$backup/package-lock.json" "$remote_dir/package-lock.json"
  (cd "$remote_dir" && npm ci --omit=dev)
  if [[ -f "$backup/no-telemetry-service" ]]; then rm -f "$remote_dir/telemetry-service.js"; else cp -a "$backup/telemetry-service.js" "$remote_dir/telemetry-service.js"; fi
  if [[ -f "$backup/no-stats-html" ]]; then rm -f "$remote_dir/web/stats.html"; else cp -a "$backup/stats.html" "$remote_dir/web/stats.html"; fi
  if [[ -f "$backup/no-hardening-dropin" ]]; then
    rm -f "$dropin"
  else
    cp -a "$backup/10-hardening.conf" "$dropin"
  fi
  systemctl daemon-reload
  systemctl restart "$service"
}

install_db() {
  local path="$1"
  local supplied="$2"
  local expected="$3"
  if [[ -f "$path" ]] && echo "$expected  $path" | sha256sum -c --status; then
    rm -f "$supplied"
    return
  fi
  echo "$expected  $supplied" | sha256sum -c --status
  chown root:root "$supplied"
  chmod 600 "$supplied"
  mv "$supplied" "$path"
}

mkdir -p "$telemetry_data_dir"
chown root:root "$telemetry_data_dir"
chmod 700 "$telemetry_data_dir"
install_db "$ipv4_db" "$ipv4_tmp" "$ipv4_sha256"
install_db "$ipv6_db" "$ipv6_tmp" "$ipv6_sha256"

chown root:root "$server_tmp" "$telemetry_tmp" "$web_tmp" "$stats_tmp" "$hardening_tmp" "$package_tmp" "$lock_tmp"
chmod 644 "$server_tmp" "$telemetry_tmp" "$web_tmp" "$stats_tmp" "$hardening_tmp" "$package_tmp" "$lock_tmp"
mv "$package_tmp" "$remote_dir/package.json"
mv "$lock_tmp" "$remote_dir/package-lock.json"
(cd "$remote_dir" && npm ci --omit=dev)
mv "$server_tmp" "$remote_dir/server.js"
mv "$telemetry_tmp" "$remote_dir/telemetry-service.js"
mkdir -p "$remote_dir/web/dist"
mv "$web_tmp" "$remote_dir/web/dist/index.html"
mv "$stats_tmp" "$remote_dir/web/stats.html"
mv "$hardening_tmp" "$dropin"
systemctl daemon-reload

if ! systemctl restart "$service" || ! systemctl is-active --quiet "$service"; then
  rollback
  echo "部署失败，已从 $backup 回滚" >&2
  exit 1
fi

sleep 1
if ! curl -fsS http://127.0.0.1:8787/pinvou3/remote/healthz >/dev/null \
  || ! curl -fsS http://127.0.0.1:8787/pinvou3/telemetry/healthz >/dev/null; then
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
mkdir -p "$remote_dir/web/dist"
cp -a "$backup/index.html" "$remote_dir/web/dist/index.html"
cp -a "$backup/package.json" "$remote_dir/package.json"
cp -a "$backup/package-lock.json" "$remote_dir/package-lock.json"
(cd "$remote_dir" && npm ci --omit=dev)
if [[ -f "$backup/no-telemetry-service" ]]; then rm -f "$remote_dir/telemetry-service.js"; else cp -a "$backup/telemetry-service.js" "$remote_dir/telemetry-service.js"; fi
if [[ -f "$backup/no-stats-html" ]]; then rm -f "$remote_dir/web/stats.html"; else cp -a "$backup/stats.html" "$remote_dir/web/stats.html"; fi
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
