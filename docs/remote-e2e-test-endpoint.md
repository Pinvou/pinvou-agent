# Web 远控测试端点（remote-test）与 e2e 基建

> 面向要给「手机 web 远程控制」相关功能做真机 e2e 的协作者。
> 一句话：`./scripts/deploy-remote-relay-test.sh` 把本地 `remote-control-relay/` 的当前代码
> 部署成独立于生产的测试实例，桌面 app 用两个环境变量指过去即可真机验证。
>
> **与生产的边界（如实说明）**：生产 relay 实例（8787）、生产路由 `/pinvou3/remote`、生产
> telemetry 数据目录完全不动，测试流量与数据目录独立；但脚本会对**共享前端代理**做有限的
> 运维变更——新增 nginx snippet + 一行 include 并 reload、创建专用低权限隧道账号
> `relay-tunnel`、对 relay 主机 IP 做 fail2ban unban。这些变更全部由脚本自动备份、
> 原子替换、失败回滚，并非「生产零改动」。

## 架构

```
手机浏览器 / 桌面 app
   │  https://pinvou.com/pinvou3/remote-test/{r/,ws,healthz}
   ▼
前端代理  admin@8.218.49.20  (nginx, pinvou.com)
   │  proxy_pass → 127.0.0.1:8788
   ▼
SSH 反向隧道  pinvou-remote-relay-test-tunnel.service
   │  47.120.8.237 --ssh -R 127.0.0.1:8788--> relay-tunnel@8.218.49.20
   ▼
relay 主机  root@47.120.8.237
   pinvou-remote-relay-test.service  (node, 端口 8788, /opt/pinvou-remote-relay-test)
```

**为什么有隧道**：云安全组从前端代理到 relay 主机只放行 8787（生产 relay 专用），
测试实例的 8788 无法被代理直连；relay 主机出方向 22 可达代理，因此用
`ssh -R` 把测试实例回环到代理本机。两个 systemd 单元均 `Restart=always` + enabled，重启自愈。

**隧道权限收敛**：隧道不再使用 `admin` 账号。代理侧有专用低权限账号 `relay-tunnel`
（nologin shell，无 sudo），其 `authorized_keys` 仅一行，options 为
`restrict,permitlisten="127.0.0.1:8788"`——即使 relay 主机 root 或隧道私钥泄露，
也只能建立这一个回环反向转发，无法在代理机执行任何命令或转发其他端口。

与生产的关系：

| | 生产 | 测试 |
|---|---|---|
| 公网路径 | `/pinvou3/remote` | `/pinvou3/remote-test` |
| relay 实例 | `pinvou-remote-relay.service`（8787） | `pinvou-remote-relay-test.service`（8788） |
| 部署脚本 | `scripts/deploy-remote-relay.sh` | `scripts/deploy-remote-relay-test.sh` |
| nginx | `snippets/pinvou3-remote-relay.conf` | `snippets/pinvou3-remote-relay-test.conf` |
| 隧道登录账号 | 无隧道（安全组直连 8787） | 代理侧专用账号 `relay-tunnel` |

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

**更新语义**：每次部署都会显式 `systemctl restart` 测试 relay 实例与隧道（即使已 active），
保证新代码/新参数真正生效；重启后做本地健康检查，失败时**自动回滚到上一版**
（`/opt/pinvou-remote-relay-test.bak`，失败版本留在 `.failed` 供排查）并以非零退出。

**安全校验**：在任何远端变更之前，脚本会校验目标 host（`RELAY_SERVER`/`PROXY_SERVER`
必须等于登记的两台主机）、`TEST_DIR`（必须是 `/opt/pinvou-remote-relay-test` 本身或其
后缀/子目录）、`TEST_PORT`（1024–65535 且不得为生产端口 8787）、`BASE_PATH`
（必须以 `/pinvou3/remote-test` 开头）。任一不符直接拒绝执行——包括 `--teardown` 的
`rm -rf` 也依赖这道白名单（远端还会二次校验）。

**失败处理**：远端每处变更都按「暂存文件 → 原子替换 → 失败回滚」执行：

- relay 代码：先整目录备份再覆盖；健康检查失败自动回滚。
- nginx：snippet 先写 `.new`、site 先备份，`nginx -t` 不过则恢复旧 snippet 并撤掉本次
  新增的 include，回到变更前的可用配置后才退出。
- 隧道账号 `authorized_keys`：按 comment 整行原子替换——relay 侧密钥重新生成后旧公钥
  会被整体换掉（密钥轮换幂等），不会残留失效 key。
- 中途异常退出时，脚本会打印失败阶段；重跑脚本即可幂等收敛。

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

移除测试实例、隧道、nginx location、代理上的 `relay-tunnel` 账号（含 authorized_keys）
以及旧版脚本遗留在 `admin` authorized_keys 里的历史隧道 key。生产 relay 实例与
`/pinvou3/remote` 路由不改动；共享代理的 nginx 会 reload。

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
2. **隧道 key 权限**：用专用账号 `relay-tunnel`（nologin shell）+ options
   `restrict,permitlisten="127.0.0.1:8788"`。注意 options 里**不能写 `permitopen="none"`**——
   语法非法会让 sshd 忽略整条 key（`Server accepts key` 都没有），随后触发上面第 1 条的
   fail2ban 连锁。`restrict` 与 `permitlisten` 组合是合法且收敛的写法（2026-07-20 实测）。
3. **密钥轮换**：relay 侧隧道私钥在 `/root/.ssh/id_ed25519_relay_test_tunnel`，重新生成后
   只需重跑部署脚本——代理侧按 comment 整行原子替换 authorized_keys，旧公钥自动失效。
4. 改代理 nginx 只有 `nginx -t` 通过才 reload；脚本改 `sites-enabled/pinvou` 前会自动备份
   （`pinvou.bak-*`），snippet 更新走 `.new` 暂存 + 原子替换，验证失败自动恢复旧配置。
5. 测试端点是**公网可达**的，依赖 pairing token 鉴权（与生产同模型）；不要用它挂长期敏感会话，
   测完敏感数据随手 `--teardown` 或换 QR。

## 服务器变更登记（2026-07-20 首次落地；2026-07-21 评审后收敛）

- `root@47.120.8.237`：`/opt/pinvou-remote-relay-test/`（及 `.bak` / `.failed` 回滚目录）、
  `pinvou-remote-relay-test.service`、`pinvou-remote-relay-test-tunnel.service`、
  `/root/.ssh/id_ed25519_relay_test_tunnel{,.pub}`、`/var/lib/pinvou-telemetry-test/`
- `admin@8.218.49.20`：`/etc/nginx/snippets/pinvou3-remote-relay-test.conf`、
  `sites-enabled/pinvou` 内一行 include、专用隧道账号 `relay-tunnel`
  （`/home/relay-tunnel/.ssh/authorized_keys` 内 `relay-test-tunnel` 条目）。
  注：首版脚本曾把隧道 key 放在 `admin` 自己的 authorized_keys，评审后已收敛到
  `relay-tunnel`；新版脚本部署/teardown 时会自动清理该历史条目。
