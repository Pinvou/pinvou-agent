# 契约：系统监控内存快照

## 消费方

- 前端桥接层：`pinvou3-app/src/tauri-bridge.js`
- 系统监控页面：`pinvou3-app/src/index.html`

## 提供方

- Tauri command：`get_monitor_snapshot`
- 后端采样聚合：`monitor::sample_all`
- 平台能力：`os::ram_snapshot`

## 快照结构

`get_monitor_snapshot` 返回的监控快照中，`ram` 字段必须遵守以下结构：

```json
{
  "ram": {
    "total_kib": 33554432,
    "used_kib": 12582912,
    "swap_total_kib": 0,
    "swap_used_kib": 0
  }
}
```

当系统内存采样失败时：

```json
{
  "ram": null
}
```

## 字段规则

- `total_kib`：物理内存总量，单位 KiB，成功采样时必须大于 0。
- `used_kib`：物理内存已用量，单位 KiB，成功采样时不得大于 `total_kib`。
- `swap_total_kib`：交换空间或页面文件总量，单位 KiB；不可获取时为 0。
- `swap_used_kib`：交换空间或页面文件已用量，单位 KiB；不可获取时为 0。

## 兼容性要求

- Linux 下字段含义必须与现有 `/proc/meminfo` 解析结果一致。
- Windows 下必须至少保证物理内存字段有效。
- 不支持的平台可以返回 `ram: null`。
- 采样失败不得使 `get_monitor_snapshot` 整体返回错误。

## 非目标

- 不改变 GPU 快照字段。
- 不改变 vLLM 快照字段。
- 不改变前端页面结构和文案。
