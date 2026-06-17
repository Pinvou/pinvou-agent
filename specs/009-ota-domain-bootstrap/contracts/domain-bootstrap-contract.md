# 契约：H3C 域名引导服务

## 请求

**Endpoint**

```http
POST /v2/bootstrap
Content-Type: application/json
```

请求 host 来自 `~/.pinvou3/windows-ota-bootstrap.json` 的 `bootstrapHost`；未配置或配置格式非法时使用 `https://bootstrap.magic.h3c.com`。

**Body**

```json
{
  "device_id": "219904A17T4257W00018",
  "product_id": "61de63cd22271b82ccd9e1bc258b55e0",
  "timestamp": "1797427200000",
  "sign": "md5-lowercase",
  "sign_type": "0"
}
```

## SN 规则

- 读取设备 BIOS SN 并 trim。
- SN 以 `2198` 或 `2199` 开头时，`device_id` 使用该 BIOS SN。
- SN 为空、读取失败或不以 `2198`/`2199` 开头时，`device_id` 使用固定值 `219904A17T4257W00018`。

## 签名规则

固定值：

- `product_id = 61de63cd22271b82ccd9e1bc258b55e0`
- `sign_type = 0`
- `secret = 664a7836315deb989e5f1451b5860774`

签名输入：

```text
device_id={device_id}&product_id={product_id}&secret={secret}&sign_type={sign_type}&keys={timestamp}
```

签名输出：

- 使用 UTF-8 编码。
- 计算 MD5。
- 输出小写十六进制字符串。
- `timestamp` 与请求体中的 `timestamp` 必须一致。

## 成功响应

域名引导后台返回的数据至少包含 `data` 对象。

```json
{
  "code": 0,
  "data": {
    "smarthubOta": "https://api.intcloud.h3c.com"
  }
}
```

兼容性要求：

- 成功状态兼容正式服务返回的 `code = 0`，也兼容测试 mock 或旧约定中的 `code = 200`。
- `success` 字段可能不存在；不存在时只按成功 `code` 判定，显式为 `false` 时按失败处理。
- 消息字段兼容 `message` 和 `msg`。
- `smarthubOta` key 查找大小写不敏感。
- `data` 可能包含其他服务地址，本 feature 只读取 `smarthubOta`。

## 失败响应

```json
{
  "success": false,
  "code": 500,
  "message": "签名错误"
}
```

失败处理：

- `success = false`、非成功状态、HTTP 异常、JSON 解析失败、缺少 `data`、缺少 `smarthubOta` 或 `smarthubOta` URL 非法时，域名引导失败。
- 域名引导失败时，本次 Windows OTA 检查不得继续访问 OTA 后台。
- 用户可见文案应表达“更新服务暂不可用”或等价友好含义，不展示完整 SN。
