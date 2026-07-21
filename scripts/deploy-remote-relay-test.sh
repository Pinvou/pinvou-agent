#!/usr/bin/env bash
# 部署/拆除 PINVOU 手机远控【测试端点】(默认 https://pinvou.com/pinvou3/remote-test)。
#
# 用途:把本地 remote-control-relay/ 的当前代码部署为独立于生产的测试实例,
#       供 web 远控相关功能的真机 e2e 使用。
#
# 与生产的边界(如实说明):
#   - 不改动:生产 relay 实例(pinvou-remote-relay.service/8787)、生产路由 /pinvou3/remote、
#     生产 telemetry 数据目录;测试流量与数据目录完全独立。
#   - 会更改(共享前端代理上的有限运维面,均自动备份/原子替换/失败回滚):
#     1) nginx:snippets/pinvou3-remote-relay-test.conf + sites-enabled/pinvou 一行 include + reload
#     2) 专用低权限隧道账号 relay-tunnel(nologin shell)及其 authorized_keys
#     3) fail2ban:启动隧道前对 relay 主机 IP unbanip(未封时为无害 no-op)
#
# 架构(详见 docs/remote-e2e-test-endpoint.md):
#   手机/桌面端 → 前端代理(admin@8.218.49.20, nginx pinvou.com)
#     → 127.0.0.1:8788(SSH 反向隧道,relay-tunnel 账号)→ relay 主机(root@47.120.8.237)
#     → pinvou-remote-relay-test.service(node, 端口 8788)
#   云安全组只放行代理→relay 主机的 8787(生产),所以测试实例走反向隧道。
#
# 用法:
#   ./scripts/deploy-remote-relay-test.sh            # 部署/更新(幂等;更新必重启实例+健康检查+失败回滚)
#   ./scripts/deploy-remote-relay-test.sh --teardown # 拆除
#
# 安全校验(在任何远端变更之前执行,防止 env 覆盖误伤):
#   - RELAY_SERVER / PROXY_SERVER 必须等于登记主机,否则直接拒绝执行;
#   - TEST_DIR 必须是 /opt/pinvou-remote-relay-test 本身或其后缀/子目录(--teardown 的 rm -rf 依赖此);
#   - TEST_PORT 必须是 1024-65535 且不得为生产端口 8787;BASE_PATH 必须以 /pinvou3/remote-test 开头。
#
# 可用环境变量覆盖(默认值即当前线上布局):
#   SSH_KEY        ~/.ssh/id_ed25519_pinvou8
#   TEST_DIR       /opt/pinvou-remote-relay-test
#   TEST_PORT      8788
#   BASE_PATH      /pinvou3/remote-test
#   PUBLIC_URL     https://pinvou.com/pinvou3/remote-test
#   SKIP_LOCAL_TESTS=1                   跳过本地 npm test(默认会跑)
# 注:必须带 -E,否则函数+heredoc 调用失败时 ERR trap 不触发(错误路径无阶段报告)。
set -eEuo pipefail

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
TUNNEL_USER="relay-tunnel"

# 登记主机:脚本的一切远端操作只允许落在这两台机器上。
EXPECTED_RELAY_SERVER="root@47.120.8.237"
EXPECTED_PROXY_SERVER="admin@8.218.49.20"
PROD_PORT=8787
RELAY_HOST_IP="${RELAY_SERVER#*@}"

STAGE="初始化"
on_err() {
  local rc=$?
  echo "✗ 失败于阶段: $STAGE (exit=$rc)" >&2
  echo "  远端变更均按「暂存 → 原子替换 → 失败回滚」执行;如怀疑残留,可重跑脚本(幂等收敛)或 --teardown。" >&2
  exit "$rc"
}
trap on_err ERR

# 在任何远端变更之前校验目标与路径,防止 env 覆盖把部署/teardown 指到错误目标。
validate_config() {
  local err=0
  if [ "$RELAY_SERVER" != "$EXPECTED_RELAY_SERVER" ]; then
    echo "拒绝执行: RELAY_SERVER=$RELAY_SERVER 不是登记的 relay 主机($EXPECTED_RELAY_SERVER)" >&2; err=1
  fi
  if [ "$PROXY_SERVER" != "$EXPECTED_PROXY_SERVER" ]; then
    echo "拒绝执行: PROXY_SERVER=$PROXY_SERVER 不是登记的前端代理($EXPECTED_PROXY_SERVER)" >&2; err=1
  fi
  if ! [[ "$TEST_PORT" =~ ^[0-9]+$ ]] || [ "$TEST_PORT" -lt 1024 ] || [ "$TEST_PORT" -gt 65535 ]; then
    echo "拒绝执行: TEST_PORT=$TEST_PORT 必须是 1024-65535 的数字" >&2; err=1
  elif [ "$TEST_PORT" = "$PROD_PORT" ]; then
    echo "拒绝执行: TEST_PORT=$PROD_PORT 是生产 relay 端口" >&2; err=1
  fi
  case "$BASE_PATH" in
    /pinvou3/remote-test|/pinvou3/remote-test/*) ;;
    *) echo "拒绝执行: BASE_PATH=$BASE_PATH 必须以 /pinvou3/remote-test 开头" >&2; err=1 ;;
  esac
  case "$TEST_DIR" in
    /opt/pinvou-remote-relay-test|/opt/pinvou-remote-relay-test-*|/opt/pinvou-remote-relay-test/*) ;;
    *) echo "拒绝执行: TEST_DIR=$TEST_DIR 必须是 /opt/pinvou-remote-relay-test 本身或其后缀/子目录" >&2; err=1 ;;
  esac
  case "$PUBLIC_URL" in
    *"$BASE_PATH"*) ;;
    *) echo "拒绝执行: PUBLIC_URL=$PUBLIC_URL 与 BASE_PATH=$BASE_PATH 不一致" >&2; err=1 ;;
  esac
  [ "$err" = 0 ] || exit 1
}

relay_ssh() { ssh -i "$SSH_KEY" "$RELAY_SERVER" "$@"; }
proxy_ssh() { ssh -i "$SSH_KEY" "$PROXY_SERVER" "$@"; }

# ssh 会把所有命令参数用空格拼接成一个字符串,交给远端登录 shell 重新分词——
# 含空格的参数(如公钥)若不按远端 shell 语法二次引用,会被拆散(2026-07-21 实测踩坑:
# 公钥被拆成 3 个参数,authorized_keys 写入错位行)。shq 做 POSIX 安全单引号引用。
shq() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }
shq_all() { local out="" a; for a in "$@"; do out="$out $(shq "$a")"; done; printf '%s' "${out# }"; }

teardown() {
  echo "── 拆除测试端点 ──"
  STAGE="拆除 relay 主机侧资源"
  relay_ssh "bash -s -- $(shq_all "$TEST_DIR" "$SERVICE" "$TUNNEL_SERVICE")" <<'REMOTE'
set -euo pipefail
test_dir="$1"; service="$2"; tunnel_service="$3"
# 远端二次校验:任何删除动作之前,确认 test_dir 是预期的测试目录(防 env 覆盖导致 rm -rf 误伤)。
case "$test_dir" in
  /opt/pinvou-remote-relay-test|/opt/pinvou-remote-relay-test-*|/opt/pinvou-remote-relay-test/*) ;;
  *) echo "拒绝删除: test_dir=$test_dir 不在测试目录白名单内" >&2; exit 1 ;;
esac
systemctl disable --now "$tunnel_service" 2>/dev/null || true
systemctl disable --now "$service" 2>/dev/null || true
rm -f "/etc/systemd/system/$tunnel_service" "/etc/systemd/system/$service"
rm -f /root/.ssh/id_ed25519_relay_test_tunnel /root/.ssh/id_ed25519_relay_test_tunnel.pub
rm -rf "$test_dir" "$test_dir.bak" "$test_dir.failed" /var/lib/pinvou-telemetry-test
systemctl daemon-reload
echo "relay 主机:已移除测试实例与隧道"
REMOTE
  STAGE="拆除代理侧资源"
  proxy_ssh "bash -s -- $(shq_all "$NGINX_SNIPPET" "$NGINX_SITE" "$TUNNEL_KEY_COMMENT" "$TUNNEL_USER")" <<'REMOTE'
set -euo pipefail
snippet="$1"; site="$2"; key_comment="$3"; tunnel_user="$4"
sudo cp -a "$site" "$site.bak-teardown-$(date +%Y%m%d-%H%M%S)"
sudo sed -i "\|include $snippet;|d" "$site"
sudo rm -f "$snippet" "$snippet.new" "$snippet.rollback"
# 专用隧道账号连同 home(含 authorized_keys)一起移除。
if id "$tunnel_user" >/dev/null 2>&1; then
  sudo userdel -r "$tunnel_user" 2>/dev/null || sudo userdel "$tunnel_user"
fi
# 兼容旧版脚本:清理 admin 自己 authorized_keys 里的历史隧道 key 条目(原子替换)。
if [ -f "$HOME/.ssh/authorized_keys" ] && grep -q " $key_comment\$" "$HOME/.ssh/authorized_keys"; then
  tmp="$(mktemp)"
  grep -v " $key_comment\$" "$HOME/.ssh/authorized_keys" > "$tmp" || true
  chmod 600 "$tmp"
  mv "$tmp" "$HOME/.ssh/authorized_keys"
fi
sudo nginx -t
sudo systemctl reload nginx
echo "前端代理:已移除 location、隧道账号与历史 key 并 reload nginx"
REMOTE
  echo "拆除完成。生产 relay 实例与 /pinvou3/remote 路由未改动;共享代理的 nginx 已 reload(变更面见脚本头注释)。"
  exit 0
}

validate_config

if [[ "${1:-}" == "--teardown" ]]; then
  teardown
fi

STAGE="本地检查"
echo "── 本地检查 ──"
node --check "$RELAY_DIR/server.js"
node --check "$RELAY_DIR/telemetry-service.js"
if [[ "${SKIP_LOCAL_TESTS:-0}" != "1" ]]; then
  (cd "$RELAY_DIR" && npm test)
fi

STAGE="上传 relay 代码(旧版备份为 $TEST_DIR.bak)"
echo "── 上传 relay 代码到 $RELAY_SERVER:$TEST_DIR ──"
# 先整目录备份现役版本(含 node_modules),供健康检查失败时回滚;再覆盖上传。
relay_ssh "if [ -d '$TEST_DIR' ]; then rm -rf '$TEST_DIR.bak' && cp -a '$TEST_DIR' '$TEST_DIR.bak'; fi; mkdir -p '$TEST_DIR/web'"
scp -i "$SSH_KEY" \
  "$RELAY_DIR/server.js" "$RELAY_DIR/telemetry-service.js" \
  "$RELAY_DIR/package.json" "$RELAY_DIR/package-lock.json" \
  "$RELAY_SERVER:$TEST_DIR/"
scp -i "$SSH_KEY" "$RELAY_DIR/web/index.html" "$RELAY_DIR/web/stats.html" \
  "$RELAY_SERVER:$TEST_DIR/web/"

STAGE="配置并重启测试 relay 实例(端口 $TEST_PORT,含失败回滚)"
echo "── 配置并重启测试 relay 实例(端口 $TEST_PORT) ──"
relay_ssh "bash -s -- $(shq_all "$TEST_DIR" "$TEST_PORT" "$BASE_PATH" "$SERVICE" "$PROXY_SERVER")" <<'REMOTE'
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
systemctl enable "$service"
# 关键:即使服务已 active 也必须显式 restart,否则旧进程仍跑旧代码,"更新幂等"不成立。
systemctl restart "$service"
sleep 2
if ! { systemctl is-active --quiet "$service" && curl -fsS "http://127.0.0.1:$port$base_path/healthz" > /dev/null; }; then
  echo "✗ 新版本健康检查失败" >&2
  if [ -d "$test_dir.bak" ]; then
    echo "── 自动回滚到上一版($test_dir.bak)──" >&2
    rm -rf "$test_dir.failed"
    mv "$test_dir" "$test_dir.failed"
    mv "$test_dir.bak" "$test_dir"
    systemctl restart "$service" || true
    sleep 2
    if systemctl is-active --quiet "$service" && curl -fsS "http://127.0.0.1:$port$base_path/healthz" > /dev/null; then
      echo "回滚完成,已恢复上一版(失败版本保留在 $test_dir.failed)" >&2
    else
      echo "⚠ 回滚后健康检查仍未通过,请人工登录检查: journalctl -u $service" >&2
    fi
  else
    echo "⚠ 无历史版本可回滚,请人工检查: journalctl -u $service" >&2
  fi
  exit 1
fi
echo "测试 relay 实例已重启并通过本地健康检查"
REMOTE

STAGE="确保隧道私钥(relay 侧)"
echo "── 确保 SSH 反向隧道(relay → 代理 127.0.0.1:$TEST_PORT,账号 $TUNNEL_USER) ──"
relay_ssh bash -s <<'REMOTE'
set -euo pipefail
[ -f /root/.ssh/id_ed25519_relay_test_tunnel ] \
  || ssh-keygen -t ed25519 -N "" -q -f /root/.ssh/id_ed25519_relay_test_tunnel -C "relay-test-tunnel"
cat /root/.ssh/id_ed25519_relay_test_tunnel.pub
REMOTE
TUNNEL_PUBKEY="$(relay_ssh 'cat /root/.ssh/id_ed25519_relay_test_tunnel.pub')"

STAGE="代理侧隧道账号与 authorized_keys(原子替换,支持密钥轮换)"
# 权限收敛:专用低权限账号 relay-tunnel(nologin shell)+ options 白名单,
# 私钥即使泄露也只能建立指定端口的回环反向转发,无法在代理机执行命令。
# 注意 1:options 里绝不能写 permitopen="none"(语法非法会导致整条 key 被 sshd 忽略,
#   认证失败后服务重启风暴会触发 fail2ban 封 IP —— 2026-07-20 实测踩坑)。
# 注意 2:不能用 restrict 替代显式 no-* 列表 —— restrict 会置 no_port_forwarding,
#   把转发整体禁用,permitlisten 只是转发被允许时的白名单,两者组合 = 全部转发被拒
#   (OpenSSH 9.6p1 实测:Server has disabled port forwarding,2026-07-21 踩坑)。
proxy_ssh "bash -s -- $(shq_all "$TUNNEL_USER" "$TUNNEL_PUBKEY" "$TEST_PORT" "$TUNNEL_KEY_COMMENT")" <<'REMOTE'
set -euo pipefail
tunnel_user="$1"; pubkey="$2"; port="$3"; key_comment="$4"
if ! id "$tunnel_user" >/dev/null 2>&1; then
  nologin_path="$(command -v nologin || echo /usr/sbin/nologin)"
  sudo useradd --create-home --shell "$nologin_path" --comment "pinvou3 remote-test tunnel" "$tunnel_user"
fi
# useradd 新建账号的 shadow 是 '!',sshd 按 "account is locked" 拒绝包括 pubkey 在内的一切登录,
# 隧道重启风暴随即触发 fail2ban 封 IP(2026-07-21 实测踩坑)。
# 置为 '*':密码登录不可用,但账号不算锁定,pubkey 可正常认证。幂等,每次部署都执行。
sudo usermod -p '*' "$tunnel_user"
tunnel_group="$(id -gn "$tunnel_user")"
home_dir="$(getent passwd "$tunnel_user" | cut -d: -f6)"
ak="$home_dir/.ssh/authorized_keys"
sudo install -d -m 700 -o "$tunnel_user" -g "$tunnel_group" "$home_dir/.ssh"
new_line="no-pty,no-agent-forwarding,no-X11-forwarding,no-user-rc,permitlisten=\"127.0.0.1:$port\" $pubkey"
# relay-tunnel 是本脚本全权管理的专用账号,authorized_keys 收敛为恰好一行:
# 暂存(同目录,保证 mv 同文件系统原子)→ 权限 → mv。relay 侧密钥轮换后旧公钥被整体
# 替换,历史错位行/失效 key 一并清除,天然幂等。
sudo bash -c '
  set -euo pipefail
  ak="$1"; new_line="$2"; owner="$3"
  tmp="$(mktemp "$ak.XXXXXX")"
  printf "%s\n" "$new_line" > "$tmp"
  chmod 600 "$tmp"
  chown "$owner" "$tmp"
  mv "$tmp" "$ak"
' _ "$ak" "$new_line" "$tunnel_user:$tunnel_group"
# 兼容旧版脚本:该 key 若曾放在 admin 自己的 authorized_keys,移除(权限收敛)。
if [ -f "$HOME/.ssh/authorized_keys" ] && grep -q " $key_comment\$" "$HOME/.ssh/authorized_keys"; then
  tmp="$(mktemp)"
  grep -v " $key_comment\$" "$HOME/.ssh/authorized_keys" > "$tmp" || true
  chmod 600 "$tmp"
  mv "$tmp" "$HOME/.ssh/authorized_keys"
  echo "已从 admin 的 authorized_keys 移除历史隧道 key(收敛到 $tunnel_user)"
fi
REMOTE

# 防 fail2ban 误封:启动隧道前先解封 relay 主机 IP(未封时是无害 no-op)。
STAGE="fail2ban 解封 relay 主机 IP"
proxy_ssh "sudo fail2ban-client set sshd unbanip '$RELAY_HOST_IP' > /dev/null 2>&1 || true"

STAGE="配置并重启 SSH 反向隧道"
relay_ssh "bash -s -- $(shq_all "$TUNNEL_SERVICE" "$TEST_PORT" "$PROXY_SERVER" "$TUNNEL_USER")" <<'REMOTE'
set -euo pipefail
tunnel_service="$1"; port="$2"; proxy_server="$3"; tunnel_user="$4"
proxy_ip="${proxy_server#*@}"
cat > "/etc/systemd/system/$tunnel_service" <<EOF
[Unit]
Description=PINVOU Remote Relay TEST endpoint reverse tunnel to front proxy
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/bin/ssh -N -T -R 127.0.0.1:$port:127.0.0.1:$port -i /root/.ssh/id_ed25519_relay_test_tunnel -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o ServerAliveInterval=30 -o ServerAliveCountMax=3 -o ExitOnForwardFailure=yes $tunnel_user@$proxy_ip
Restart=always
RestartSec=10
User=root

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable "$tunnel_service"
# 已 active 也强制 restart:端口/密钥/目标账号变更必须生效。
systemctl restart "$tunnel_service"
sleep 4
systemctl is-active --quiet "$tunnel_service"
REMOTE

STAGE="隧道回环健康检查"
proxy_ssh "curl -fsS --max-time 5 'http://127.0.0.1:$TEST_PORT$BASE_PATH/healthz' > /dev/null && echo '隧道回环健康检查通过'"

STAGE="配置前端代理 nginx($BASE_PATH,暂存 + 失败回滚)"
echo "── 配置前端代理 nginx($BASE_PATH) ──"
proxy_ssh "bash -s -- $(shq_all "$NGINX_SNIPPET" "$NGINX_SITE" "$TEST_PORT" "$BASE_PATH")" <<'REMOTE'
set -euo pipefail
snippet="$1"; site="$2"; port="$3"; base_path="$4"
# 1) 暂存新 snippet(不影响现役配置)
sudo tee "$snippet.new" > /dev/null <<EOF
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
# 2) 备份现役 snippet 与 site(回滚用)
if sudo test -f "$snippet"; then
  sudo cp -a "$snippet" "$snippet.rollback"
else
  sudo rm -f "$snippet.rollback"
fi
site_changed=0
if ! sudo grep -qF "include $snippet;" "$site"; then
  sudo cp -a "$site" "$site.bak-$(date +%Y%m%d-%H%M%S)"
  sudo sed -i "s|    include /etc/nginx/snippets/pinvou3-remote-relay.conf;|    include /etc/nginx/snippets/pinvou3-remote-relay.conf;\n    include $snippet;|" "$site"
  site_changed=1
fi
# 3) 原子替换 snippet 并整体验证;失败则恢复到变更前状态
sudo mv "$snippet.new" "$snippet"
if ! sudo nginx -t > /dev/null 2>&1; then
  echo "✗ nginx -t 失败,自动回滚 nginx 变更" >&2
  if sudo test -f "$snippet.rollback"; then
    sudo mv "$snippet.rollback" "$snippet"
  else
    sudo rm -f "$snippet"
  fi
  if [ "$site_changed" = "1" ]; then
    sudo sed -i "\|include $snippet;|d" "$site"
  fi
  sudo nginx -t
  exit 1
fi
sudo rm -f "$snippet.rollback"
sudo systemctl reload nginx
echo "nginx 已加载 $base_path 并 reload"
REMOTE

STAGE="公网验证"
echo "── 公网验证 ──"
health="$(curl -fsS --max-time 10 "$PUBLIC_URL/healthz")"
echo "$health" | node -e 'const h=JSON.parse(require("fs").readFileSync(0,"utf8")); if(!h.ok||!("room_count" in h)) process.exit(1)'
page="$(curl -fsSL --max-time 10 "$PUBLIC_URL/r/deploy-check")"
[[ "$page" == *'<title>PINVOU Remote</title>'* ]]
prod_health="$(curl -fsS --max-time 10 "https://pinvou.com/pinvou3/remote/healthz")"
echo "$prod_health" | node -e 'const h=JSON.parse(require("fs").readFileSync(0,"utf8")); if(!h.ok) process.exit(1)'

STAGE="完成"
echo "部署完成: $PUBLIC_URL"
echo "桌面端指向测试端点:"
echo "  PINVOU_REMOTE_PUBLIC_URL=\"$PUBLIC_URL\" PINVOU_REMOTE_RELAY_WS_URL=\"wss://pinvou.com$BASE_PATH/ws\" pinvou3-tauri"
