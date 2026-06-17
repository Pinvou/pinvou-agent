# 研究：Windows OTA 域名引导

## 决策 1：域名引导实现放在 Windows 专用模块

**Decision**：新增 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs`，承载域名引导配置、请求签名、BIOS SN 选择、响应解析和 `smarthubOta` 查找逻辑；`windows_update.rs` 调用该模块解析 OTA host。

**Rationale**：规格限定 Windows OTA，用户也明确希望尽量只改 Windows 系统下的文件代码。现有 `windows_update.rs` 已经集中管理 Windows OTA 查询、下载、MSI 安装和反馈，新增 Windows 专用模块可以避免继续膨胀单文件，同时不引入跨平台接口变更。

**Alternatives considered**：
- 直接把所有逻辑继续写进 `windows_update.rs`：文件已包含 OTA、zip、MSI、反馈和大量测试，继续追加会降低可维护性。
- 放到 `updater.rs`：会把 Windows 专用协议带入跨平台命令层，不符合当前 OS 分支边界。
- 新增跨平台 `domain_bootstrap` 抽象：当前只有 Windows 使用，抽象过早。

## 决策 2：配置文件使用用户目录 JSON，缺失时不创建或覆盖

**Decision**：配置文件路径为 `~/.pinvou3/windows-ota-bootstrap.json`，内容使用 JSON：

```json
{
  "bootstrapHost": "https://bootstrap.magic.h3c.com"
}
```

文件不存在、为空、缺少 `bootstrapHost` 或 `bootstrapHost` 格式非法时使用默认 `https://bootstrap.magic.h3c.com`；只有实际使用的引导服务不可访问或返回无效结果时，才返回友好失败并停止 OTA 查询。

**Rationale**：`~/.pinvou3` 已是项目用户数据根，受 `PINVOU3_HOME` 支持，便于测试隔离，也不会被 MSI 安装目录覆盖。JSON 可读性足够，后续如果需要增加环境、备注或诊断字段也有扩展空间。

**Alternatives considered**：
- 写入安装目录配置：可能需要管理员权限，升级/重装时也更容易被覆盖。
- 复用 `settings.json`：该配置是基础设施引导地址，不是常规 UI 偏好；加入 settings 会扩大前端和用户偏好迁移范围。
- 环境变量作为主配置：不利于现场部署人员长期维护，也不满足“提供配置文件”的要求。

## 决策 3：SN 选择以 BIOS SN 为主，固定 SN 兜底

**Decision**：域名引导前读取 Windows BIOS SN，trim 后如果以 `2198` 或 `2199` 开头，则作为请求 `device_id`；否则使用固定 SN `219904A17T4257W00018`。SN 前缀判断抽为纯函数以便单测，实际 BIOS 读取失败、为空或命令异常均走固定 SN。

**Rationale**：用户明确指定 BIOS SN 和固定兜底规则。将选择规则与读取动作拆开，可以稳定覆盖 `2198`、`2199`、其他前缀、空值和读取失败场景。

**Alternatives considered**：
- 继续使用 `COMPUTERNAME` 或 `PINVOU3_OTA_SN`：不满足 BIOS SN 的业务要求。
- 因 BIOS 读取失败而中断更新检查：与固定 SN 兜底规则相冲突，会降低非目标设备和测试环境可用性。
- 在前端读取 SN：会把设备身份细节暴露到 UI 层，不符合隐私和模块边界。

## 决策 4：迁移 C# 域名引导业务契约

**Decision**：按 C# `H3C.DomainRedirect` 参考组件迁移请求契约：

- `POST /v2/bootstrap`
- `device_id`：有效 SN
- `product_id`：`61de63cd22271b82ccd9e1bc258b55e0`
- `timestamp`：当前 Unix 毫秒
- `sign_type`：`0`
- `sign`：`device_id={SN}&product_id={ProductId}&secret={SecretKey}&sign_type={SignType}&keys={Timestamp}` 的 UTF-8 MD5 小写值
- secret：`664a7836315deb989e5f1451b5860774`

响应解析期望 `data` 为服务 key 到 URL 的对象，查找 `smarthubOta` 时大小写不敏感。

**Rationale**：该契约来自已实现组件，是域名引导后台的事实来源。当前 Rust 项目已有 `reqwest`、`serde_json`、`md5` 和时间依赖，迁移无需增加重量级依赖。

**Alternatives considered**：
- 只请求固定 URL 并跳过签名：后台可能拒绝请求，不符合参考实现。
- 只兼容完全小写/完全一致 key：C# 组件明确使用大小写不敏感匹配，Rust 迁移应保持一致。

## 决策 5：OTA host 需要写入待反馈记录

**Decision**：`UpdateFeedbackRecord` 增加 `ota_host` 字段。查询更新成功时 `UpdateInfo`/Windows 私有结构携带解析出的 OTA host；启动 MSI 前写入反馈记录；升级后 `report_pending_update_result` 优先使用记录中的 `ota_host` 调用 `/ota/pkg/package/updateLog`。旧记录缺少该字段时允许重新域名引导一次作为兼容。

**Rationale**：规格要求查询、下载和反馈使用同一 OTA 来源。升级反馈发生在新进程首次运行后，如果只重新域名引导，后台调度结果可能变化。将 OTA host 写入本地待反馈记录可以保留“本次升级”的来源上下文。

**Alternatives considered**：
- 反馈时重新域名引导：实现简单，但不能保证与安装前 OTA 来源一致。
- 反馈继续使用旧固定 `DEFAULT_OTA_HOST`：违背域名引导目标，也会在专网/预发布环境中失效。
- 把 OTA host 存到前端状态：安装后进程退出，前端状态不可保留。

## 决策 6：失败处理不使用旧 OTA 地址兜底

**Decision**：实际请求的域名引导服务失败、响应为空、缺少 `data`、缺少 `smarthubOta` 或 `smarthubOta` URL 非法时，本次 Windows OTA 检查失败并返回友好文案，不再访问旧固定 OTA host。

**Rationale**：规格假设明确排除了离线缓存或旧 OTA 地址兜底。失败即停止能避免命中错误环境，也让测试人员能直接发现引导配置或后台异常。

**Alternatives considered**：
- 使用 `https://api.intcloud.h3c.com` 兜底：表面上提升成功率，但破坏后台调度和专网环境行为。
- 缓存最近成功的 OTA host：会引入过期、撤回和环境切换问题，MVP 不需要。
