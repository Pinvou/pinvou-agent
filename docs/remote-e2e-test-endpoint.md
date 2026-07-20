# Web 远控测试端点（remote-test）与 e2e 基建

> 面向要给「手机 web 远程控制」相关功能做真机 e2e 的协作者。
> 一句话：`./scripts/deploy-remote-relay-test.sh` 把本地 `remote-control-relay/` 的当前代码
> 部署成独立于生产的测试实例，桌面 app 用两个环境变量指过去即可真机验证，**生产零影响**。

## 架构

```
手机浏览器 / 桌面 app
   │  https://pinvou.com/pinvou3/remote-test/{r/,ws,healthz}
   ▼
前端代理  admin@8.218.49.20  (nginx, pinvou.com)
   │  proxy_pass → 127.0.0.1:8788
   ▼
SSH 反向隧道  pinvou-remote-relay-test-tunnel.service
   │  47.120.8.237 --ssh -R 127.0.0.1:8788--> 8.218.49.20
   ▼
relay 主机  root@47.120.8.237
   pinvou-remote-relay-test.service  (node, 端口 8788, /opt/pinvou-remote-relay-test)
```

**为什么有隧道**：云安全组从前端代理到 relay 主机只放行 8787（生产 relay 专用），
测试实例的 8788 无法被代理直连；relay 主机出方向 22 可达代理，因此用
`ssh -R` 把测试实例回环到代理本机。两个 systemd 单元均 `Restart=always` + enabled，重启自愈。

与生产的关系：

| | 生产 | 测试 |
|---|---|---|
| 公网路径 | `/pinvou3/remote` | `/pinvou3/remote-test` |
| relay 实例 | `pinvou-remote-relay.service`（8787） | `pinvou-remote-relay-test.service`（8788） |
| 部署脚本 | `scripts/deploy-remote-relay.sh` | `scripts/deploy-remote-relay-test.sh` |
| nginx | `snippets/pinvou3-remote-relay.conf` | `snippets/pinvou3-remote-relay-test.conf` |

测试实例故意调小了容量（`MAX_ROOMS=100` / `MAX_WS_CONNECTIONS=300` / `ROOM_CREATE_LIMIT=10`），
telemetry 数据目录独立（`/var/lib/pinvou-telemetry-test`），与生产互不沾。

## 使用

### 部署 / 更新测试端点（幂等）

在本地仓库根目录（需要持有 `~/.ssh/id_ed25519_pinvou8`）：

```bash
./scripts/deploy-remote-relay-test.sh
```

脚本会上传**本地工作区**的 `remote-control-relay/` 代码（server.js / web/index.html 等），
所以验证任何 web 远控改动的流程是：改代码 → 跑脚本 → 真机测。重复执行收敛无副作用。
默认会先跑本地 `npm test`，可用 `SKIP_LOCAL_TESTS=1` 跳过。

### 桌面 app 指向测试端点

环境变量驱动（`pinvou3-app/src-tauri/src/remote_control/manager.rs` 读取）：

```bash
PINVOU_REMOTE_PUBLIC_URL="https://pinvou.com/pinvou3/remote-test" \
PINVOU_REMOTE_RELAY_WS_URL="wss://pinvou.com/pinvou3/remote-test/ws" \
pinvou3-tauri        # 或 ./pinvou3-app/run-dev.sh 调试构建
```

之后正常「开远程控制 → 手机扫码」，二维码自动指向测试端点。

### 拆除

```bash
./scripts/deploy-remote-relay-test.sh --teardown
```

移除测试实例、隧道、nginx location 与代理上的隧道 key，生产不动。

## 配套自动化测试（无真机时）

- relay 单测：`cd remote-control-relay && npm test`（房间/鉴权/限流/payload 上限等）。
- 页面 e2e（jsdom 驱动真实 `web/index.html`）：`remote-control-relay/test/mobile-download.test.js`
  是模板——用 `JSDOM(runScripts:'dangerously')` 加载页面、桩掉 WebSocket/createObjectURL，
  直接调用页面全局函数（`handleDesktopEvent` / `showPreview` / 按钮 click）断言 DOM 与出站消息。
  **新 web 功能照此模式加 `test/*.test.js` 即自动进 `npm test`。**
- desktop 回路 e2e：`pinvou3-app/.../remote_control/manager.rs` 的 `e2e_tests` 模块——
  子进程起真实 node relay + 真实 `relay_client`（WS）+ `RemoteControlManager::new_headless()`，
  tokio-tungstenite 扮演 mobile。缺 node / relay node_modules 时自动跳过，CI 安全。

## 运维注意事项（踩坑记录）

1. **fail2ban**：代理的 sshd 有 fail2ban。隧道服务若认证失败会快速重启刷连接，数秒内被封 IP
   （表现为 `Connection refused`）。部署脚本启动隧道前会先 `unbanip` 兜底；手工排查时用
   `sudo fail2ban-client status sshd` 确认。
2. **authorized_keys options**：隧道 key 的 options 里**不能写 `permitopen="none"`**——语法非法
   会让 sshd 忽略整条 key（`Server accepts key` 都没有）。正确写法见脚本：
   `no-pty,no-agent-forwarding,no-X11-forwarding,no-user-rc,permitlisten="127.0.0.1:8788"`。
3. 改代理 nginx 只有 `nginx -t` 通过才 reload；脚本改 `sites-enabled/pinvou` 前会自动备份
   （`pinvou.bak-*`）。
4. 测试端点是**公网可达**的，依赖 pairing token 鉴权（与生产同模型）；不要用它挂长期敏感会话，
   测完敏感数据随手 `--teardown` 或换 QR。

## 服务器变更登记（2026-07-20 首次落地）

- `root@47.120.8.237`：`/opt/pinvou-remote-relay-test/`、`pinvou-remote-relay-test.service`、
  `pinvou-remote-relay-test-tunnel.service`、`/root/.ssh/id_ed25519_relay_test_tunnel{,.pub}`、
  `/var/lib/pinvou-telemetry-test/`
- `admin@8.218.49.20`：`/etc/nginx/snippets/pinvou3-remote-relay-test.conf`、
  `sites-enabled/pinvou` 内一行 include、`~admin/.ssh/authorized_keys` 内 `relay-test-tunnel` 条目
