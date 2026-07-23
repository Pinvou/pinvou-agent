# Web 远控测试端点（remote-test）与 e2e 基建

> 面向要给「手机 web 远程控制」相关功能做真机 e2e 的协作者。
> 一句话：`./scripts/deploy-remote-relay-test.sh` 把本地 `remote-control-relay/` 的当前代码
> 部署成独立于生产的测试实例，桌面 app 用两个环境变量指过去即可真机验证。
>
> **与生产的边界（如实说明）**：生产 relay 实例（8787）、生产路由 `/pinvou3/remote`、生产
> telemetry 数据目录完全不动，测试流量与数据目录独立；但脚本会对**共享前端代理**做有限的
> 运维变更——新增 nginx snippet + 一行 include 并 reload、创建专用低权限隧道账号
> `relay-tunnel`、`/etc/ssh/sshd_config` 末尾该账号的 `Match User` 限权块、对 relay 主机 IP 做 fail2ban
> unban。关键配置采用暂存、原子替换和局部失败回滚，但跨两台机器并非一个全局事务，
> 并非「生产零改动」。

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
`no-pty,no-agent-forwarding,no-X11-forwarding,no-user-rc,permitlisten="127.0.0.1:8788"`，
同时 sshd `Match User relay-tunnel` 设置 `AllowTcpForwarding remote` 与同一 `PermitListen`
——即使 relay 主机 root 或隧道私钥泄露，也只能建立这一个回环反向转发，
无法在代理机执行命令、发起本地转发或监听其他端口。

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

脚本先用 `/pinvou3/remote-test` base path 构建共享 React UI，再上传**本地工作区**的
`remote-control-relay/` 服务代码与完整 `web/dist`。所以验证任何 WebUI 改动的流程是：
改代码 → 跑脚本 → 真机测。重复执行收敛无副作用。默认会先跑本地 `npm test`；只有在已经
单独完成验证时才可用 `SKIP_LOCAL_TESTS=1` 或 `SKIP_WEB_BUILD=1` 跳过对应步骤。

**更新语义**：每次部署都会显式 `systemctl restart` 测试 relay 实例与隧道（即使已 active），
保证新代码/新参数真正生效；重启后做本地健康检查，失败时**自动回滚到上一版**
（`/opt/pinvou-remote-relay-test.bak`，失败版本留在 `.failed` 供排查）并以非零退出。

**安全校验**：在任何远端变更之前，脚本会校验目标 host（`RELAY_SERVER`/`PROXY_SERVER`
必须等于登记的两台主机）、`TEST_DIR`（必须精确等于 `/opt/pinvou-remote-relay-test`）、
`TEST_PORT`（1024–65535 且不得为生产端口 8787）、`BASE_PATH`
（必须以 `/pinvou3/remote-test` 开头），且 `TEST_DIR`/`BASE_PATH` 只允许
字母数字与 `. _ / -`（会被拼接进远端命令，排除一切 shell 元字符）。任一不符直接拒绝执行——包括 `--teardown` 的
`rm -rf` 也依赖这道白名单（远端还会二次校验）。

**失败处理**：高风险文件替换按「暂存 → 原子替换 → 局部失败回滚」执行：

- relay 代码：打成单个 tarball 流式上传（`.tmp` 落盘后原子 mv），远端先解包到 `.extract`
  暂存目录并装好依赖才整体替换——解包/`npm ci` 失败时现役目录未被触碰；替换后健康检查
  失败自动回滚到 `.bak`。
- **公网验证失败也会回滚 relay 侧**到上一版并重启（与生产 `deploy-remote-relay.sh` 同语义）。
- nginx：snippet 先写 `.new`、site 先备份，`nginx -t` 不过则恢复旧 snippet 并撤掉本次
  新增的 include，回到变更前的可用配置后才退出。
- 隧道账号 `authorized_keys`：`relay-tunnel` 为脚本全权管理的专用账号，每次原子重写为
  恰好一行——relay 侧密钥重新生成后旧公钥被整体换掉（密钥轮换幂等），不残留失效 key。
- sshd：主配置末尾一段带 BEGIN/END 标记的专用 `Match User` 块只允许 remote forwarding；
  `sshd -t` 或 reload 失败会恢复旧配置。之所以不用 `sshd_config.d`，是因为该机的 Include
  位于主配置前部，drop-in 中的 `Match` 会污染随后全局指令的解析语境。
- 中途异常退出时，脚本会打印失败阶段；跨 relay/代理两台主机的整体部署不是全局事务，
  重跑脚本可幂等收敛，或用 `--teardown` 清理。

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
- 完整 WebUI 浏览器 smoke：`remote-control-relay/test/web-ui.smoke.cjs`，由
  `npm --prefix pinvou3-app run test:webui` 构建并在桌面/手机视口执行。
- v2 Relay 与部署契约：`relay.test.js`、`deploy.test.js`、`remote-test-deploy.test.js`；退役的
  v1 jsdom 页面测试只由 `npm run test:legacy-v1-ui` 显式运行，不进入 v2 默认门禁。
- desktop 回路 e2e：`pinvou3-app/.../remote_control/manager.rs` 的 `e2e_tests` 模块——
  子进程起真实 node relay + 真实 `relay_client`（WS）+ `RemoteControlManager::new_headless()`，
  tokio-tungstenite 扮演 mobile。缺 node / relay node_modules 时自动跳过，CI 安全。

## 运维注意事项（踩坑记录）

1. **fail2ban**：代理的 sshd 有 fail2ban。隧道服务若认证失败会快速重启刷连接，数秒内被封 IP
   （表现为 `Connection refused`）。部署脚本启动隧道前会先 `unbanip` 兜底；手工排查时用
   `sudo fail2ban-client status sshd` 确认。
2. **隧道 key 权限**：用专用账号 `relay-tunnel`（nologin shell）+ options
   `no-pty,no-agent-forwarding,no-X11-forwarding,no-user-rc,permitlisten="127.0.0.1:8788"`。
   两个坑：options 里**不能写 `permitopen="none"`**——语法非法会让 sshd 忽略整条 key
   （2026-07-20 实测）；也**不能用 `restrict` 替代显式 no-\* 列表**——`restrict` 会置
   `no_port_forwarding` 把转发整体禁用，`permitlisten` 只是转发被允许时的白名单，
   两者组合 = 全部转发被拒（OpenSSH 9.6p1 实测报 `Server has disabled port forwarding`，
   2026-07-21 踩坑）。
   还要注意：`permitlisten` 只约束 `ssh -R`，不会限制 `ssh -L`。脚本因此另外写入
   `/etc/ssh/sshd_config` 末尾的 `Match User relay-tunnel`，使用 `AllowTcpForwarding remote`
   禁止本地转发；不能只靠
   `authorized_keys` options 宣称该密钥只能建立指定反向隧道。
3. **新建系统账号默认是锁定的**：`useradd` 出来的账号 shadow 为 `!`，sshd 按
   "account is locked" 拒绝**包括 pubkey 在内**的一切登录——隧道重启风暴随即触发 fail2ban
   封 IP（2026-07-21 实测踩坑）。必须 `usermod -p '*'`：密码登录不可用但账号不算锁定，
   pubkey 可正常认证。脚本已内置此步骤。
4. **密钥轮换**：relay 侧隧道私钥在 `/root/.ssh/id_ed25519_relay_test_tunnel`，重新生成后
   只需重跑部署脚本——`relay-tunnel` 是脚本全权管理的专用账号，其 authorized_keys 每次
   原子重写为恰好一行，旧公钥/历史错位行自动失效。
   （实现细节：ssh 会把命令参数拼接成一个字符串交给远端 shell 重新分词，含空格的参数
   如公钥必须二次引用——脚本用 `shq` 统一处理，否则 authorized_keys 会写入错位行。）
5. **改代理 nginx 的两个坑**（2026-07-21 实测）：① `sites-enabled/` 下的**任何文件**都会被
   nginx 当活配置加载——备份绝不能放这里，脚本统一放 `/etc/nginx/pinvou-backups/`
   （曾因此导致 teardown 后 `nginx -t` 报残留备份里的悬空 include）；② `sites-enabled/pinvou`
   是指向 `sites-available/pinvou` 的软链，`sed -i` 会把软链替换成脱钩普通文件，脚本一律
   `readlink -f` 解析真实路径再改。改配置只有 `nginx -t` 通过才 reload，snippet 更新走
   `.new` 暂存 + 原子替换，验证失败自动恢复旧配置。
6. 测试端点是**公网可达**的，依赖 pairing token 鉴权（与生产同模型）；不要用它挂长期敏感会话，
   测完敏感数据随手 `--teardown` 或换 QR。

## 服务器变更登记（2026-07-20 首次落地；2026-07-21 评审后收敛）

- `root@47.120.8.237`：`/opt/pinvou-remote-relay-test/`（及 `.bak` / `.failed` 回滚目录）、
  `pinvou-remote-relay-test.service`、`pinvou-remote-relay-test-tunnel.service`、
  `/root/.ssh/id_ed25519_relay_test_tunnel{,.pub}`、`/var/lib/pinvou-telemetry-test/`
- `admin@8.218.49.20`：`/etc/nginx/snippets/pinvou3-remote-relay-test.conf`、
  `sites-enabled/pinvou`（→ `sites-available/pinvou` 软链）内一行 include、
  `/etc/nginx/pinvou-backups/`（site 备份目录）、专用隧道账号 `relay-tunnel`
  （`/home/relay-tunnel/.ssh/authorized_keys` 内 `relay-test-tunnel` 条目）、
  `/etc/ssh/sshd_config` 末尾的 `BEGIN/END PINVOU3 REMOTE TEST TUNNEL` 托管块。
  注：首版脚本曾把隧道 key 放在 `admin` 自己的 authorized_keys，评审后已收敛到
  `relay-tunnel`；新版脚本部署/teardown 时会自动清理该历史条目。
  另：首版脚本在 `sites-enabled/` 直接写 `pinvou.bak-*` 备份且 `sed -i` 断开了
  sites-available 软链，2026-07-21 已手工修复布局（备份移至 `pinvou-backups/`、
  恢复软链），新版脚本会自动收敛同类残留。
