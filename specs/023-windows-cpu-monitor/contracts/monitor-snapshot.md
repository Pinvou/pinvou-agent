# 契约：系统监控快照

## 调用方

前端系统监控页通过现有 `get_monitor_snapshot` 命令获取完整监控快照。

## 响应字段约定

### `cpu`

Windows 目标平台新增可选字段。

```json
{
  "cpu": {
    "name": "Intel(R) Core(TM) Ultra ...",
    "total_usage_pct": 18.5,
    "process_usage_pct": 3.2,
    "logical_processors": 16
  }
}
```

字段说明：

- `name`：CPU 名称；不可用时允许为空或由前端显示不可用文案。
- `total_usage_pct`：总体 CPU 使用率，百分比。
- `process_usage_pct`：pinvou3 当前进程 CPU 使用率，百分比。
- `logical_processors`：逻辑处理器数量。

### `gpu`

保持现有契约。非 Windows 平台系统监控页继续使用该字段渲染 GPU 卡片。

### `generated_at_ms`

保持现有契约。CPU 卡片使用该字段或前端已有格式化结果展示更新时间。

## 展示规则

- Windows 上：如果 `cpu` 字段存在，资源卡片展示为 CPU 卡片。
- Windows 上：如果 `cpu` 字段为空或部分字段缺失，资源卡片仍展示 CPU 语义，并对缺失项显示占位值。
- 非 Windows 上：继续使用 `gpu` 字段和现有 GPU 文案。
- CPU 字段缺失不得影响 `ram`、`vllm`、`self_perf`、`app` 等字段展示。

## 兼容性

- 新增 `cpu` 字段为可选字段，旧前端忽略该字段不会破坏现有快照。
- 非 Windows 平台可不提供 `cpu` 字段。
