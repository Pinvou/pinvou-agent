# 研究结论：Windows 软件更新

## Decision: 沿用现有 `updater.rs` 命令入口，按平台分支切换包类型

**Rationale**：现有前端已经围绕 `check_for_update`、`download_update`、`install_update`、`cancel_download`、`update:progress` 构建了状态和进度 UI。继续使用这组命令可以复用用户体验，减少前端大改；Windows 只需把后端返回的包类型、下载校验、安装启动语义从 `.deb/pkexec` 替换为 `zip/MSI`。

**Alternatives considered**：新增一套 `windows_check_update` 命令会让前端和测试出现两套更新状态；接入 Tauri 官方 updater 会绕开 H3C OTA 查询/反馈协议，不能满足迁移 C# 项目逻辑的要求。

## Decision: Windows 更新信息查询迁移 H3C OTA 协议

**Rationale**：C# `UpdateHttpService.CheckUpdateVersionAsync` 使用 `POST /ota/pkg/package/upgrade/check`，请求字段为 `sn`、`softwareId`、`version`、可选 `hardwareInfo`。响应成功条件是 `success == true` 且 `code == 200`，`data` 中包含 `updateInfo`、`updateType`、`updateVersion`、`pkgMd5`、`incrPkgMd5`。这是本 feature 查询阶段的兼容目标。

**Alternatives considered**：继续使用 Linux `latest.json` 静态源最小，但无法反馈 OTA 结果，也不兼容 H3C 服务端；实现独立服务端适配层会增加部署面，暂不需要。

## Decision: 下载优先复用现有 HTTP 流式下载和进度事件

**Rationale**：现有 `download_update` 已具备 HTTP 下载、停滞超时、取消、写盘、进度事件和校验逻辑。Windows 可以复用这条下载循环，把目标扩展为 zip 包并使用 H3C 返回的 MD5 或包清单 hash 校验。这样前端 `update:progress` 和取消语义保持一致。

**Alternatives considered**：迁移 C# 的断点续传下载器可获得 resume 能力，但会引入新复杂度；本 feature 成功标准没有要求断点续传，先保留普通 HTTP 下载。

## Decision: 下载 zip 直接按完整包解析

**Rationale**：实际 H3C 下载地址返回的 zip 已经是完整包内容，根目录包含 `OtaInfo.json` 与 `Files/Pinvou3/pinvou3_0.4.4_x64_en-US.msi`。因此解析策略为：下载完成后直接安全解压 zip，在解压目录中读取 `OtaInfo.json` 并定位 MSI。

**Alternatives considered**：兼容嵌套完整包会增加不必要分支，且与当前服务端实际包结构不一致；直接扫描所有 `.msi` 会绕过清单，容易安装错误文件。

## Decision: 根据 `OtaInfo.json` 中的 Pinvou3 子软件信息定位 MSI

**Rationale**：样例 `OtaInfo.json` 顶层 `softwareInfos` 中存在 `softwareId = Pinvou3_Win`、`softwareType = Pinvou3`、`sourceDir = Pinvou3`，`fileMetaInfos[0].fileName = pinvou3_0.4.4_x64_en-US.msi`，文件实际位于 `Files/Pinvou3/`。实现应根据 `softwareId` 或 `softwareType` 选择目标子项，再用 `sourceDir + fileMetaInfos.filePath/fileName` 定位安装文件，并校验扩展名和 hash。

**Alternatives considered**：从 `attachData.exeName` 取文件名也可辅助定位，但 `attachData` 是字符串化 JSON 且可能为空，应作为 fallback，不作为唯一来源。

## Decision: 安装器启动成功后持久化待反馈状态，再退出当前进程

**Rationale**：启动 `MSI` 后当前 pinvou 进程需要退出以避免文件占用；退出前必须写入“已请求安装/待反馈”记录，包含软件标识、SN、旧版本、目标版本、包信息和阶段状态。升级完成后再次运行时，系统才能知道需要调用反馈接口。

**Alternatives considered**：安装前立即反馈成功不真实；等待 `MSI` 完成再退出可能与安装器接管和文件占用冲突；只靠内存状态会在退出后丢失。

## Decision: 反馈使用 H3C OTA 日志接口并支持重试

**Rationale**：C# `UpdateHttpService.UpdateOtaLogAsync` 使用 `POST /ota/pkg/package/updateLog`，字段为 `softwareIdentification`、`sn`、`currentVersion`、`updateVersion`、`updateErrorInfo`、`updateResult`。本 feature 需要在升级后的后续运行提交成功、失败、取消或未知结果；若网络失败，应保留记录以便下次重试。

**Alternatives considered**：只记录本地日志不满足服务端闭环；每次启动无条件反馈会造成重复上报，必须有已反馈状态或幂等保护。

## Decision: Windows 安装启动使用系统安装器，路径必须限定在更新目录

**Rationale**：现有 Linux `validate_deb_path` 已有“安装包必须在 `~/.pinvou3/updates/` 内”的安全边界。Windows 应沿用同等约束：安装文件 canonicalize 后必须位于更新目录或其本次解压子目录内，扩展名必须是 `.msi`，并通过 `msiexec` 或 Windows shell 启动。启动成功只代表安装器已接管，不代表安装完成。

**Alternatives considered**：直接执行任意路径的安装文件风险过高；把 `MSI` 复制到临时目录会增加清理和路径穿越风险。
