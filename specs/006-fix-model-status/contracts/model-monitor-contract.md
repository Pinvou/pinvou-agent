# 契约：大模型状态监控快照

## 消费方

- 前端桥接层：`pinvou3-app/src/tauri-bridge.js`
- 系统监控页面：`pinvou3-app/src/index.html`
- 后端状态命令：`get_monitor_snapshot`、`get_backend_status`

## 提供方

- 后端采样聚合：`monitor::sample_all`
- 模型目标推导：`Pinvou3Bridge` 或其提取出的目标 helper
- 模型状态探测：本地模型探测与远端模型探测

## 快照结构

`get_monitor_snapshot` 返回的大模型字段应保留现有前端可用字段，并新增或明确以下语义。字段名可兼容现有 `vllm` 命名，但语义必须升级为“当前模型目标状态”：

```json
{
  "vllm": {
    "status": "ready",
    "target_kind": "remote",
    "model": "deepseek-v4-pro",
    "configured_model": "deepseek-v4-pro",
    "provider": "deepseek",
    "upstream": "https://api.deepseek.com",
    "metrics_applicable": false,
    "diagnostic": null,
    "metric_diagnostics": [
      {
        "code": "remote_metrics_not_applicable",
        "message": "远端模型不提供本地运行指标",
        "detail": null
      }
    ]
  }
}
```

本地模型且指标可用时：

```json
{
  "vllm": {
    "status": "busy",
    "target_kind": "local",
    "model": "qwen36_35b_256k",
    "configured_model": "qwen36_35b_256k",
    "provider": "vllm",
    "upstream": "http://127.0.0.1:8001/v1",
    "metrics_applicable": true,
    "max_model_len": 262144,
    "num_requests_running": 1,
    "num_requests_waiting": 0,
    "prefix_cache_hit_pct": 82.5,
    "ttft_sum_s": 12.4,
    "ttft_count": 8,
    "tpot_sum_s": 2.1,
    "tpot_count": 256,
    "generation_tokens_total": 1024,
    "prompt_tokens_total": 2048,
    "diagnostic": null,
    "metric_diagnostics": []
  }
}
```

不可用或不可确认时：

```json
{
  "vllm": {
    "status": "offline",
    "target_kind": "remote",
    "model": null,
    "configured_model": "deepseek-v4-pro",
    "provider": "deepseek",
    "upstream": "https://api.deepseek.com",
    "metrics_applicable": false,
    "diagnostic": {
      "code": "unauthorized",
      "message": "远端模型鉴权失败",
      "detail": "HTTP 401"
    },
    "metric_diagnostics": []
  }
}
```

## 状态规则

- `ready`：当前模型目标可访问，且模型匹配或无需强制模型匹配。
- `busy`：当前本地模型目标可访问，且本地运行指标显示存在运行中或等待中请求。
- `mismatch`：目标可访问，但配置模型名与服务返回模型名不一致。
- `offline`：连接失败、超时、鉴权失败、配置无效、响应不是模型服务或其他不可用情况。
- `unknown`：远端目标无法提供足够信息判断模型状态，但不能被等同于本地离线。

## 目标类型规则

- `local`：当前实际配置指向本机模型地址，允许检测本地运行指标。
- `remote`：当前实际配置指向远端模型地址，只检测远端基础状态，不请求或不展示本地运行指标。
- `invalid`：当前配置地址或模型名不足以形成有效监控目标。

## 指标规则

- 本地目标可用时，`max_model_len`、队列、KV、TTFT、吞吐和 token 统计按可用字段逐项展示。
- 本地目标 metrics 不可用时，基础状态仍可为 `ready`，并通过 `metric_diagnostics` 说明指标缺失。
- 远端目标的本地运行指标默认不适用，必须通过 `remote_metrics_not_applicable` 或等价语义表达。
- 前端不得在远端目标下把队列、KV、TTFT、吞吐等空值解释为本地模型异常。

## 兼容性要求

- 前端已有字段不得无计划删除，避免系统监控页直接崩溃。
- 新增诊断字段必须可为空。
- `get_backend_status.vllm_online` 或后续等价字段在远端模型可用时也应为 true。
- 状态检测失败不得导致 `get_monitor_snapshot` 整体返回错误。

## 非目标

- 不自动启动本地模型。
- 不停止占用端口的其他服务。
- 不修复 GPU、系统内存或版本更新栏。
- 不修改 DeepSeek-TUI 底座。
