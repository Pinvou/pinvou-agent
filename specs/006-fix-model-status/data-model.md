# 数据模型：大模型状态监控

## 模型监控目标

**用途**：表示系统监控页应检测的当前实际模型目标。

**字段**：

- `base_url`：当前实际模型地址。
- `configured_model`：当前实际配置的模型名。
- `provider`：当前实际 provider 标识。
- `target_kind`：目标类型，例如本地模型、远端模型、配置异常。
- `source`：配置来源摘要，例如环境变量、用户设置、预设默认值。

**验证规则**：

- 监控目标必须与聊天实际推理目标一致。
- 远端目标不得被本机默认地址状态覆盖。
- 地址为空、格式错误或缺少模型名时，目标类型应为配置异常。

## 模型状态快照

**用途**：表示一次大模型状态检测的用户可见结果。

**字段**：

- `status`：在线、离线、不可确认、鉴权失败、模型不匹配或配置异常。
- `target_kind`：本地、远端或配置异常。
- `model`：服务返回的实际模型名。
- `configured_model`：当前配置模型名。
- `upstream`：实际检测地址。
- `diagnostic`：整体状态诊断。
- `metrics_applicable`：本地运行指标是否适用于当前目标。
- `metrics_diagnostics`：本地指标缺失或不适用原因。

**验证规则**：

- 远端模型可访问且配置有效时，不得显示为本地离线。
- 本地模型服务响应非模型内容时，不得显示为在线。
- 模型名不匹配时必须优先显示模型不匹配状态。
- 状态检测失败不得影响 GPU、系统内存和应用信息展示。

## 本地运行指标

**用途**：表示仅适用于本地模型服务的运行信息。

**字段**：

- `max_model_len`：上下文长度。
- `num_requests_running`：运行中请求数。
- `num_requests_waiting`：等待中请求数。
- `prefix_cache_hit_pct`：KV 或 prefix cache 命中率。
- `ttft`：首字延迟统计。
- `tps`：生成吞吐统计。
- `generation_tokens_total`：累计生成 token。
- `prompt_tokens_total`：累计输入 token。

**验证规则**：

- 当前目标为远端模型时，本地运行指标默认不适用。
- 当前目标为本地模型且指标可用时，应逐项展示可用值。
- 单个指标缺失不得清空其他可用指标。
- metrics 缺失不得单独导致基础模型状态离线。

## 状态诊断信息

**用途**：解释模型状态不可用、不可确认或指标缺失的原因。

**字段**：

- `code`：稳定的机器可读原因。
- `message`：中文可读说明。
- `detail`：可选补充信息。

**常见 code**：

- `invalid_config`
- `connection_failed`
- `request_timeout`
- `unauthorized`
- `unexpected_response`
- `model_mismatch`
- `remote_metrics_not_applicable`
- `metrics_unavailable`
- `metric_missing`

**验证规则**：

- 不可用、不可确认、模型不匹配和配置异常状态必须有整体诊断。
- 远端模型的本地指标不适用必须通过指标诊断表达，不得让用户误以为本地模型异常。
