# macOS Keychain 反复弹窗说明

## 现象

社区版（GitHub Releases 的 `*-community.dmg`）在 macOS 上首次访问 Keychain 时弹出
"xxx 想要使用钥匙串" 的授权窗口；**即使点了「始终允许」,后续仍会反复弹出同一窗口**。
相关 Issue:#175。

## 根因

这是 macOS 安全机制在 **ad-hoc 签名** 下的固有行为,不是应用代码 bug。

### 1. 社区版是 ad-hoc 签名

`pinvou3-app/src-tauri/config/platforms/macos/tauri.conf.json` 中
`macOS.signingIdentity = "-"`(Apple 的 ad-hoc 签名标识),未启用 Hardened Runtime,
也没有 Apple Developer ID 公证(公证需要 Apple 开发者账号 + `$99/年` 证书)。

ad-hoc 签名**没有证书身份,只有一个 `cdhash`**(二进制内容的密码学指纹)。

### 2. "始终允许"绑定应用的代码签名身份

macOS Keychain 的「始终允许」不是全局开关,而是把"哪个应用可以免密访问"这条规则
写进**那一条 keychain item 的 ACL(Access Control List)**。这里的"哪个应用"是用
**designated requirement(DR)** 来识别的:

- 有证书签名的应用:DR 形如 `identifier "com.pinvou.pinvou3" and anchor apple generic
  and certificate leaf[subject.CN] = ...`。这是**稳定**身份,ACL 一次写入、长期命中。
- ad-hoc 签名的应用:DR 形如 `cdhash H"…"`。这是**不稳定**身份:
  - 应用每次重新编译/发布,`cdhash` 都会变;
  - Universal binary(Apple Silicon + Intel)两个架构切片各有独立 `cdhash`;
  - 某些 macOS 版本下,cdhash-only DR 在后续访问的校验中无法稳定匹配。

结果:点「始终允许」时,系统尝试把当前进程的 DR 写入 item ACL;但因为 ad-hoc 的 DR
无法在后续访问时可靠命中,系统下次访问又把当前进程判定为"未授权应用" → 重新弹窗。

### 3. keyring 后端不设自定义 ACL

应用使用 `codewhale-secrets` → `keyring` crate(v3.x,`apple-native` feature)
→ `security-framework` 的经典 `SecKeychainAddGenericPassword` API 创建 Keychain item。
该 API **不传任何 `SecAccess` / 自定义 ACL**,完全依赖 macOS 默认访问控制 —— 而
默认访问控制在 ad-hoc 签名下正是无法稳定放行上述 DR 的那一层。

### 4. 多 item × 多次读取放大问题

应用按用途分多个 Keychain service(均以 `pinvou3-` 前缀命名,与第三方应用隔离):

- `pinvou3-model-api-key` — 模型 API Key(每个模型一条)
- `pinvou3-search-api-key` — 搜索 API Key
- `pinvou3-mcp-secret` — 工具市场 / MCP 连接器密钥
- `pinvou3-ima-secret` — IMA 密钥
- `pinvou3-acp-provider-key` — ACP 供应商密钥
- `pinvou3-remote-knowledge-token` — 远程知识库设备令牌
- `pinvou3-shared-knowledge-backup` — 共享知识库备份密钥

每个 item 首次访问各自弹一次;而设置页每次打开(`get_settings`)都会回读所有已配置
item 的状态。**单 item 反复弹 + 多 item 各自弹 = "无限弹窗" 体感。**
连接远程 / 共享知识库时读取设备令牌与备份密钥,同样会触发对应 item 的首次弹窗。

> 注:全部 Keychain 访问都在单一 Tauri 主进程内完成,不涉及子进程或 Node sidecar
> 另开 Keychain(已排查 `codex-bridge-runtime`、`remote-control-relay`,均不读 Keychain)。

## 缓解措施(本仓库已做)

`pinvou3-app/src-tauri/src/platform/credential_store.rs` 增加了**进程级凭据值缓存**:

- 同一 `(service, account)` 在一次进程生命周期内只访问 Keychain **一次**(首次 `get`
  触发授权弹窗,用户点「允许」后读取成功并缓存);
- 之后命中缓存即不再触碰 Keychain,**应用运行期间不再为该凭据反复弹窗**;
- `set` / `delete` 同步更新缓存;环境变量路径不经过此缓存;Keychain 仍是单一真相源;
- 仅缓存 `Ok` 结果(含 `Ok(None)` 表示"已知不存在"),`Err` 不缓存以便自愈。

**剩余边界**:应用重启后,每个已配置凭据的首次访问仍会弹一次。这是 ad-hoc 签名的
根本限制,缓存无法跨越进程边界。

## 用户手动缓解(可选)

如不想忍受任何弹窗,可二选一:

### 方案 A:用环境变量绕过 Keychain(最干净)

把 API Key 放进环境变量,应用优先读环境变量、完全不碰 Keychain:

```bash
# 模型 key(以 DeepSeek 为例)
echo 'export DEEPSEEK_API_KEY="sk-你的key"' >> ~/.zshrc
# 搜索 key(按你用的 provider)
# Metaso:    METASO_API_KEY
# Baidu:     BAIDU_SEARCH_API_KEY
source ~/.zshrc
```

完整环境变量名:模型 provider 见 `CodeWhale/crates/secrets/src/lib.rs` 的 `env_for`
函数;搜索 provider(`METASO_API_KEY`、`BAIDU_SEARCH_API_KEY`)见
`pinvou3-app/src-tauri/src/platform/prefs/search.rs` 的 `env_key_names`;Bocha/Tavily
不提供环境变量通道。

### 方案 B:手动给 Keychain item 加宽松 ACL

打开「钥匙串访问」App → 找到 `pinvou3-*` 开头的条目 → 双击 → 「访问」标签 →
把应用列表设为「允许所有应用程序访问此项」(安全性下降,但可一劳永逸止弹)。

## 永久方案(规划中)

接入 Apple Developer ID 签名 + 公证后,应用获得**稳定证书身份**,Keychain ACL 可
持久命中,「始终允许」即真正生效、不再反复弹。落地点与前置条件见
`pinvou3-app/src-tauri/packaging/macos/entitlements.plist` 的注释路线图:

1. `tauri.conf.json`:`macOS.hardenedRuntime = true`(公证要求 Hardened Runtime)。
2. 配置 `signingIdentity` 为 Developer ID Application 证书 + `notarytool` 公证。
3. 评估是否补充 `com.apple.security.cs.allow-jit` / `disable-library-validation`
   等 entitlement(按实际运行决定,不全开)。
4. `com.apple.security.device.audio-input` 必须保留(ASR 麦克风采集必需)。

官方 Developer ID 构建走私有发布流水线(`scripts/release-macos.sh` 头注释提及的
"official" 后缀产物);社区版受限于无 Apple 证书,ad-hoc 是当前可发布形态。
