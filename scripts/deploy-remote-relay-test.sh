#!/usr/bin/env bash
# 部署/拆除 PINVOU 手机远控【测试端点】(默认 https://pinvou.com/pinvou3/remote-test)。
#
# 用途:把本地 remote-control-relay/ 的当前代码部署为独立于生产的测试实例,
#       供 web 远控相关功能的真机 e2e 使用,不触碰生产 /pinvou3/remote。
#
# 架构(详见 docs/remote-e2e-test-endpoint.md):
#   手机/桌面端 → 前端代理(admin@8.218.49.20, nginx pinvou.com)
#     → 127.0.0.1:8788(SSH 反向隧道)→ relay 主机(root@47.120.8.237)
#     → pinvou-remote-relay-test.service(node, 端口 8788)
#   云安全组只放行代理→relay 主机的 8787(生产),所以测试实例走反向隧道。
#
# 用法:
#   ./scripts/deploy-remote-relay-test.sh            # 部署/更新(幂等)
#   ./scripts/deploy-remote-relay-test.sh --teardown # 拆除
#
# 可用环境变量覆盖(默认值即当前线上布局):
#   RELAY_SERVER   root@47.120.8.237     relay 主机(跑 node 测试实例)
#   PROXY_SERVER   admin@8.218.49.20     前端代理(pinvou.com nginx)
#   SSH_KEY        ~/.ssh/id_ed25519_pinvou8
#   TEST_DIR       /opt/pinvou-remote-relay-test
#   TEST_PORT      8788
#   BASE_PATH      /pinvou3/remote-test
#   PUBLIC_URL     https://pinvou.com/pinvou3/remote-test
#   SKIP_LOCAL_TESTS=1                   跳过本地 npm test(默认会跑)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RELAY_DIR="$ROOT/remote-control-relay"
RELAY_SERVER="${RELAY_SERVER:-root@47.120.8.237}"
PROXY_SERVER="${PROXY_SERVER:-admin@8.218.49.20}"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/id_ed25519_pinvou8}"
TEST_DIR="${TEST_DIR:-/opt/pinvou-remote-relay-test}"
TEST_PORT="${TEST_PORT:-8788}"
BASE_PATH="${BASE_PATH:-/pinvou3/remote-test}"
PUBLIC_URL="${PUBLIC_URL:-https://pinvou.com/pinvou3/remote-test}"
SERVICE="pinvou-remote-relay-test.service"
TUNNEL_SERVICE="pinvou-remote-relay-test-tunnel.service"
NGINX_SNIPPET="/etc/nginx/snippets/pinvou3-remote-relay-test.conf"
NGINX_SITE="/etc/nginx/sites-enabled/pinvou"
TUNNEL_KEY_COMMENT="relay-test-tunnel"
RELAY_HOST_IP="${RELAY_SERVER#*@}"

relay_ssh() { ssh -i "$SSH_KEY" "$RELAY_SERVER" "$@"; }
proxy_ssh() { ssh -i "$SSH_KEY" "$PROXY_SERVER" "$@"; }

teardown() {
  echo "── 拆除测试端点 ──"
  relay_ssh bash -s -- "$TEST_DIR" "$SERVICE" "$TUNNEL_SERVICE" <<'REMOTE'
set -euo pipefail
test_dir="$1"; service="$2"; tunnel_service="$3"
systemctl disable --now "$tunnel_service" 2>/dev/null || true
systemctl disable --now "$service" 2>/dev/null || true
rm -f "/etc/systemd/system/$tunnel_service" "/etc/systemd/system/$service"
rm -f /root/.ssh/id_ed25519_relay_test_tunnel /root/.ssh/id_ed25519_relay_test_tunnel.pub
rm -rf "$test_dir" /var/lib/pinvou-telemetry-test
systemctl daemon-reload
echo "relay 主机:已移除测试实例与隧道"
REMOTE
  proxy_ssh bash -s -- "$NGINX_SNIPPET" "$NGINX_SITE" "$TUNNEL_KEY_COMMENT" <<'REMOTE'
set -euo pipefail
snippet="$1"; site="$2"; key_comment="$3"
sudo cp -a "$site" "$site.bak-teardown-$(date +%Y%m%d-%H%M%S)"
sudo sed -i "\|include $snippet;|d" "$site"
sudo rm -f "$snippet"
sed -i "\| $key_comment\$|d" "$HOME/.ssh/authorized_keys"
sudo nginx -t > /dev/null 2>&1
sudo systemctl reload nginx
echo "前端代理:已移除 location / 隧道 key 并 reload nginx"
REMOTE
  echo "拆除完成。生产 /pinvou3/remote 未受影响。"
  exit 0
}

if [[ "${1:-}" == "--teardown" ]]; then
  teardown
fi

echo "── 本地检查 ──"
node --check "$RELAY_DIR/server.js"
node --check "$RELAY_DIR/telemetry-service.js"
if [[ "${SKIP_LOCAL_TESTS:-0}" != "1" ]]; then
  (cd "$RELAY_DIR" && npm test)
fi

echo "── 上传 relay 代码到 $RELAY_SERVER:$TEST_DIR ──"
relay_ssh "mkdir -p '$TEST_DIR/web'"
scp -i "$SSH_KEY" \
  "$RELAY_DIR/server.js" "$RELAY_DIR/telemetry-service.js" \
  "$RELAY_DIR/package.json" "$RELAY_DIR/package-lock.json" \
  "$RELAY_SERVER:$TEST_DIR/"
scp -i "$SSH_KEY" "$RELAY_DIR/web/index.html" "$RELAY_DIR/web/stats.html" \
  "$RELAY_SERVER:$TEST_DIR/web/"

echo "── 配置并启动测试 relay 实例(端口 $TEST_PORT) ──"
relay_ssh bash -s -- "$TEST_DIR" "$TEST_PORT" "$BASE_PATH" "$SERVICE" "$PROXY_SERVER" <<'REMOTE'
set -euo pipefail
test_dir="$1"; port="$2"; base_path="$3"; service="$4"; proxy_server="$5"
proxy_ip="${proxy_server#*@}"
cd "$test_dir"
npm ci --omit=dev --no-audit --no-fund > /dev/null
mkdir -p /var/lib/pinvou-telemetry-test
cat > "/etc/systemd/system/$service" <<EOF
[Unit]
Description=PINVOU Remote Relay (TEST endpoint)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=$test_dir
Environment=PORT=$port
Environment=PINVOU_REMOTE_PUBLIC_BASE_PATH=$base_path
Environment=PINVOU_REMOTE_ALLOWED_PROXY_IPS=$proxy_ip
Environment=PINVOU_REMOTE_TRUSTED_PROXY_IPS=$proxy_ip
Environment=PINVOU_TELEMETRY_DATA_DIR=/var/lib/pinvou-telemetry-test
Environment=MAX_ROOMS=100
Environment=ROOM_CREATE_LIMIT=10
Environment=MAX_WS_CONNECTIONS=300
ExecStart=/usr/bin/npm start
Restart=always
RestartSec=3
User=root

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable --now "$service"
sleep 2
systemctl is-active --quiet "$service"
curl -fsS "http://127.0.0.1:$port$base_path/healthz" > /dev/null
echo "测试 relay 实例已启动并通过本地健康检查"
REMOTE

echo "── 确保 SSH 反向隧道(relay → 代理 127.0.0.1:$TEST_PORT) ──"
relay_ssh bash -s <<'REMOTE'
set -euo pipefail
[ -f /root/.ssh/id_ed25519_relay_test_tunnel ] \
  || ssh-keygen -t ed25519 -N "" -q -f /root/.ssh/id_ed25519_relay_test_tunnel -C "relay-test-tunnel"
cat /root/.ssh/id_ed25519_relay_test_tunnel.pub
REMOTE
TUNNEL_PUBKEY="$(relay_ssh 'cat /root/.ssh/id_ed25519_relay_test_tunnel.pub')"

# 注意:options 里绝不能写 permitopen="none"(语法非法会导致整条 key 被 sshd 忽略,
# 认证失败后服务重启风暴会触发 fail2ban 封 IP —— 2026-07-20 实测踩坑)。
proxy_ssh bash -s -- "$TUNNEL_PUBKEY" "$TEST_PORT" "$TUNNEL_KEY_COMMENT" <<'REMOTE'
set -euo pipefail
pubkey="$1"; port="$2"; key_comment="$3"
mkdir -p "$HOME/.ssh" && chmod 700 "$HOME/.ssh"
touch "$HOME/.ssh/authorized_keys" && chmod 600 "$HOME/.ssh/authorized_keys"
if ! grep -q " $key_comment\$" "$HOME/.ssh/authorized_keys"; then
  echo "no-pty,no-agent-forwarding,no-X11-forwarding,no-user-rc,permitlisten=\"127.0.0.1:$port\" $pubkey" \
    >> "$HOME/.ssh/authorized_keys"
fi
REMOTE

# 防 fail2ban 误封:启动隧道前先解封 relay 主机 IP(未封时是无害 no-op)。
proxy_ssh "sudo fail2ban-client set sshd unbanip '$RELAY_HOST_IP' > /dev/null 2>&1 || true"

relay_ssh bash -s -- "$TUNNEL_SERVICE" "$TEST_PORT" "$PROXY_SERVER" <<'REMOTE'
set -euo pipefail
tunnel_service="$1"; port="$2"; proxy_server="$3"
cat > "/etc/systemd/system/$tunnel_service" <<EOF
[Unit]
Description=PINVOU Remote Relay TEST endpoint reverse tunnel to front proxy
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/bin/ssh -N -T -R 127.0.0.1:$port:127.0.0.1:$port -i /root/.ssh/id_ed25519_relay_test_tunnel -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o ServerAliveInterval=30 -o ServerAliveCountMax=3 -o ExitOnForwardFailure=yes $proxy_server
Restart=always
RestartSec=10
User=root

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable --now "$tunnel_service"
sleep 4
systemctl is-active --quiet "$tunnel_service"
REMOTE

proxy_ssh "curl -fsS --max-time 5 'http://127.0.0.1:$TEST_PORT$BASE_PATH/healthz' > /dev/null && echo '隧道回环健康检查通过'"

echo "── 配置前端代理 nginx($BASE_PATH) ──"
proxy_ssh bash -s -- "$NGINX_SNIPPET" "$NGINX_SITE" "$TEST_PORT" "$BASE_PATH" <<'REMOTE'
set -euo pipefail
snippet="$1"; site="$2"; port="$3"; base_path="$4"
sudo tee "$snippet" > /dev/null <<EOF
    # PINVOU3 手机远控【测试端点】— 独立 relay 实例,由 deploy-remote-relay-test.sh 管理。
    # 生产 /pinvou3/remote 路由不受影响;8788 经 SSH 反向隧道回环到 relay 主机。
    location = $base_path/healthz {
        proxy_pass http://127.0.0.1:$port$base_path/healthz;
        proxy_http_version 1.1;
        proxy_set_header Host 127.0.0.1;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        proxy_buffering off;
        proxy_read_timeout 30s;
        proxy_send_timeout 30s;
    }

    location = $base_path/ws {
        proxy_pass http://127.0.0.1:$port$base_path/ws;
        proxy_http_version 1.1;
        proxy_set_header Host 127.0.0.1;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        proxy_set_header Upgrade \$http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_buffering off;
        proxy_read_timeout 86400s;
        proxy_send_timeout 86400s;
    }

    location ^~ $base_path/r/ {
        proxy_pass http://127.0.0.1:$port;
        proxy_http_version 1.1;
        proxy_set_header Host 127.0.0.1;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        proxy_buffering off;
        proxy_read_timeout 30s;
        proxy_send_timeout 30s;
    }
EOF
if ! sudo grep -qF "include $snippet;" "$site"; then
  sudo cp -a "$site" "$site.bak-$(date +%Y%m%d-%H%M%S)"
  sudo sed -i "s|    include /etc/nginx/snippets/pinvou3-remote-relay.conf;|    include /etc/nginx/snippets/pinvou3-remote-relay.conf;\n    include $snippet;|" "$site"
fi
sudo nginx -t > /dev/null 2>&1
sudo systemctl reload nginx
echo "nginx 已加载 $base_path 并 reload"
REMOTE

echo "── 公网验证 ──"
health="$(curl -fsS --max-time 10 "$PUBLIC_URL/healthz")"
echo "$health" | node -e 'const h=JSON.parse(require("fs").readFileSync(0,"utf8")); if(!h.ok||!("room_count" in h)) process.exit(1)'
page="$(curl -fsSL --max-time 10 "$PUBLIC_URL/r/deploy-check")"
[[ "$page" == *'<title>PINVOU Remote</title>'* ]]
prod_health="$(curl -fsS --max-time 10 "https://pinvou.com/pinvou3/remote/healthz")"
echo "$prod_health" | node -e 'const h=JSON.parse(require("fs").readFileSync(0,"utf8")); if(!h.ok) process.exit(1)'

echo "部署完成: $PUBLIC_URL"
echo "桌面端指向测试端点:"
echo "  PINVOU_REMOTE_PUBLIC_URL=\"$PUBLIC_URL\" PINVOU_REMOTE_RELAY_WS_URL=\"wss://pinvou.com$BASE_PATH/ws\" pinvou3-tauri"
