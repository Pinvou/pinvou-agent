# 契约：前端更新状态与 Tauri 命令

## 状态字段

现有 `tauri-bridge.js` 已有以下状态字段，应继续复用并按 Windows 语义扩展：

```js
{
  updateInfo: null,
  updateChecking: false,
  updateCheckError: null,
  updateDownloading: false,
  updateProgress: 0,
  updateReady: false,
  updateError: null,
  updateCancelling: false
}
```

Windows 更新需要新增或复用的状态语义：

- `updateInfo.available = true`：存在可下载完整包。
- `updateProgress`：下载阶段按字节计算；解析/准备安装阶段可保持 100 或展示安装中。
- `updateReady`：Linux 表示安装完成待重启；Windows 表示安装器已启动或升级反馈完成时的用户可见状态需重新命名或分支展示，避免误导。
- `updateError`：下载、解析、安装器启动或反馈阶段错误。

## `check_for_update`

**调用**

```js
await invoke("check_for_update")
```

**返回**

```json
{
  "available": true,
  "current_version": "0.4.3",
  "latest_version": "0.4.4.0",
  "notes": "更新说明",
  "pub_date": "",
  "url": "https://example/pinvou3.zip",
  "sha256": "",
  "size": 0,
  "package_md5": "BFBE3F51BEF0B52D3882B2E6A7B41B38",
  "software_id": "Pinvou3_Win",
  "sn": "设备序列号",
  "update_type": "Normal"
}
```

**错误**

- 查询接口不可达。
- 响应成功标志不是 `success = true && code = 200`。
- 更新信息缺少目标版本或完整包下载信息。

## `download_update`

**调用**

```js
await invoke("download_update", { info: state.updateInfo })
```

**Windows 返回**

```json
{
  "package_path": "C:\\Users\\...\\.pinvou3\\updates\\pinvou3_0.4.4.0.zip",
  "installer_path": "C:\\Users\\...\\.pinvou3\\updates\\0.4.4.0\\full\\Files\\Pinvou3\\pinvou3_0.4.4_x64_en-US.msi",
  "latest_version": "0.4.4.0"
}
```

**兼容约定**

- 现有 Linux 返回字符串 deb 路径。实现可通过新增结构或包装命令处理平台差异；前端必须能兼容 Windows 返回结构。
- 下载进度继续通过 `update:progress` 事件上报。
- 解析包失败应在此阶段返回错误，不进入安装。

## `install_update`

**Linux 调用**

```js
await invoke("install_update", { debPath: debPath })
```

**Windows 调用**

```js
await invoke("install_update", { installerPath: installerPath })
```

**Windows 行为**

- 校验 `installerPath` 在更新目录内。
- 确认扩展名为 `.msi`。
- 启动 Windows 安装器。
- 安装器成功启动后写入待反馈记录。
- 退出当前 pinvou 进程。

## `cancel_download`

**调用**

```js
await invoke("cancel_download")
```

**行为**

- 仅影响下载阶段。
- Windows 解析包或安装器已启动后，不应再尝试取消系统安装。

## 启动后反馈

应用启动或更新面板初始化时应触发一次待反馈检查：

```js
await invoke("report_pending_update_result")
```

**返回**

```json
{
  "had_pending": true,
  "reported": true,
  "result": "UPGRADE_SUCCEED",
  "message": "更新结果反馈成功"
}
```

**行为约定**

- 没有待反馈记录时返回 `had_pending = false` 或静默成功。
- 反馈失败时不得清理本地记录。
- 前端不应阻塞主界面启动；失败只在更新面板或日志中可见。
