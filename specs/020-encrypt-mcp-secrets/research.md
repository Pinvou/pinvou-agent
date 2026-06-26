# 研究：加密存储 MCP API 密钥

## 决策 1：不再把供应商真实密钥放入内置 manifest

**Decision**：同花顺、企查查、高德天气的内置 manifest 只声明敏感字段需求，不保存真实密钥。真实密钥来自旧配置迁移、用户配置或部署侧预置凭据。

**Rationale**：`pinvou3-app/resources/mcp-servers/*/manifest.json` 会在编译期内嵌，并在启动时写回 `~/.pinvou3/bundle/mcp-servers/`。只要源 manifest 带真实密钥，安装包、源码、用户目录都会暴露同一秘密。

**Alternatives considered**：
- 将密钥加密后继续放在 manifest：拒绝。客户端必须带解密材料，攻击者可从包内恢复，不应作为安全边界。
- 只清理用户目录副本：拒绝。下次启动会被内置资源再次覆盖，源头风险仍在。
- 保持明文但限制文件权限：拒绝。无法防止备份、日志、误读和安装包提取。

## 决策 2：复用系统凭据存储保存 MCP 密钥

**Decision**：扩展现有 `credential_store`，为 MCP 密钥增加独立服务命名和账号命名规则，例如按供应商、工具 id、用途和环境变量名生成稳定凭据引用。

**Rationale**：模型 API Key 已经通过 `keyring` 存储到系统凭据位置。MCP API Key 属于同类敏感凭据，复用同一抽象可以减少实现面，并保持错误脱敏、内存测试替身和跨平台行为一致。

**Alternatives considered**：
- 自行实现文件加密：拒绝。密钥管理复杂，且不如系统凭据存储适合用户级秘密。
- 将密钥写入 Windows 环境变量：拒绝。环境变量仍是明文配置，并会污染其他进程。
- 仅依赖用户手工保存在 shell profile：拒绝。桌面应用用户体验差，且不解决旧配置明文迁移。

## 决策 3：生成 mcp.json 时不持久化真实密钥

**Decision**：`mcp.json` 中不再写入真实 `env` 值或 `Authorization` bearer 值；本地 MCP server 通过凭据引用或运行期注入机制取得密钥，远程 MCP server 需要避免把 bearer header 明文写入持久配置。

**Rationale**：现有 `marketplace.rs` 会把 `manifest.env` 复制到本地 server 的 `env`，也会把远程 server 的 API key 包成 `Authorization` header 写入 `mcp.json`。这会把风险从 manifest 扩散到运行配置。

**Alternatives considered**：
- 只移除 manifest 明文：拒绝。安装后 `mcp.json` 仍会残留明文。
- 每次安装后写入密钥，退出时删除：拒绝。崩溃、备份和读取窗口仍存在。
- 修改 DeepSeek-TUI MCP client 识别加密字段：暂不采用。项目原则要求不重写底座 MCP client；优先在 pinvou wrapper 层解决。

## 决策 4：旧配置迁移必须覆盖 manifest 和 mcp.json

**Decision**：启动或工具市场初始化时扫描用户目录中的目标 manifest 与 `mcp.json`，识别目标供应商明文密钥，成功写入系统凭据后清理文件中的明文。

**Rationale**：已有用户的明文可能同时存在于 `~/.pinvou3/bundle/mcp-servers/<id>/manifest.json` 和 `~/.pinvou3/bundle/mcp.json`。只迁移其中一个文件会留下残余风险。

**Alternatives considered**：
- 要求用户删除 `~/.pinvou3` 重装：拒绝。会破坏用户数据和已安装工具状态。
- 只在重新安装工具时迁移：拒绝。未重新安装的已启用工具仍保留 `mcp.json` 明文。
- 只静态替换固定字符串：拒绝。不能覆盖用户配置或后续供应商字段。

## 决策 5：缺失密钥时明确提示，不静默回退为明文

**Decision**：当凭据缺失或系统凭据不可用时，工具安装/启用返回明确错误，指出供应商和需要重新配置的密钥名称，但不包含密钥内容。

**Rationale**：安全修复不能以重新写入明文作为兜底，也不能让用户只看到 MCP 调用失败。错误必须可恢复并脱敏。

**Alternatives considered**：
- 自动回退到 manifest 明文：拒绝。与核心安全目标冲突。
- 静默跳过工具安装：拒绝。用户难以定位原因。
- 在错误中输出原始配置片段：拒绝。可能泄露密钥。

## 决策 6：零配置统一产品密钥需要后续服务端方案

**Decision**：本 feature 不承诺在没有本地凭据、用户配置或部署预置的情况下为新安装用户提供不可提取的统一供应商密钥。若产品要求新装零配置且不暴露供应商密钥，应另立 feature 建设服务端代理或短期凭据服务。

**Rationale**：只要统一供应商密钥被放进客户端包中，无论明文还是可逆加密，都可以被提取。客户端侧加密只能保护用户本地落盘，不适合保护产品级共享秘密。

**Alternatives considered**：
- 使用硬编码密钥加密 payload：拒绝。安全收益有限且容易造成误判。
- 混淆二进制中的密钥：拒绝。混淆不是密钥管理。
- 继续复用旧明文以保持零配置：拒绝。与本 feature 目标冲突。
