# 智能表格 Webhook 真实场景示例

仅在需要构造 Webhook payload 时按需阅读。示例中的字段 ID 都是占位符，实际请求必须使用用户提供的 schema 中的字段 ID。

## 场景一：记录 Bug（文本、单选、成员、图片）

```json
{
  "add_records": [
    {
      "values": {
        "fABCD1": "登录页在 Safari 浏览器下加载后白屏，其他浏览器正常。",
        "fABCD2": [{"text": "前端"}],
        "fABCD3": [{"text": "严重"}],
        "fABCD4": [{"user_id": "wangwu"}],
        "fABCD5": [{"title": "safari-bug-screenshot.png", "image_base64": "iVBORw0KGgo..."}]
      }
    }
  ]
}
```

图片只传纯 base64，不带 `data:image/...;base64,` 前缀。

## 场景二：记录任务（文本、日期、成员、单选、空图片）

```json
{
  "add_records": [
    {
      "values": {
        "fTITLE": "完成支付模块单元测试，覆盖率达到 80%",
        "fDUEDATE": "1742400000000",
        "fOWNER": [{"user_id": "lisi"}],
        "fSTATUS": [{"text": "未开始"}],
        "fIMAGE": []
      }
    }
  ]
}
```

成员字段：

- 有 userid 时使用 `[{"user_id":"账号名"}]`。
- 只有姓名时可使用 `["张三"]`，但无法匹配时不会写入；可先通过 `wecomcli-contact` 查询 userid。
- 暂不指定时使用 `[]`。

图片字段暂无图片时使用 `[]`。文件附件字段不受 Webhook 支持，应跳过而不是用空数组尝试写入。

## 场景三：批量新增多条记录

```json
{
  "add_records": [
    {
      "values": {
        "fCUST_NAME": "张伟",
        "fCOMPANY": "北京某科技有限公司",
        "fSTAGE": [{"text": "跟进中"}],
        "fSOURCE": [{"text": "展会"}]
      }
    },
    {
      "values": {
        "fCUST_NAME": "陈静",
        "fCOMPANY": "上海某贸易有限公司",
        "fSTAGE": [{"text": "初步接触"}],
        "fSOURCE": [{"text": "冷呼"}]
      }
    },
    {
      "values": {
        "fCUST_NAME": "刘洋",
        "fCOMPANY": "广州某制造有限公司",
        "fSTAGE": [{"text": "已成交"}],
        "fSOURCE": [{"text": "老客户转介绍"}]
      }
    }
  ]
}
```

超过 100 条时先按 `SKILL.md` 取得用户确认；每批不超过 500 条，并遵守频率限制。

## 场景四：更新一条 Webhook 记录

```json
{
  "update_records": [
    {
      "record_id": "REC_20250301",
      "values": {
        "fSTATUS": [{"text": "已完成"}],
        "fPROGRESS": 100
      }
    }
  ]
}
```

只能更新此前通过 Webhook 写入的记录。人工创建或通过普通接口创建的记录无法使用此方式更新。

## 场景五：批量更新 Webhook 记录

```json
{
  "update_records": [
    {"record_id": "REC_001", "values": {"fSTATUS": [{"text": "已通过"}], "fAPPROVER": [{"user_id": "manager_a"}]}},
    {"record_id": "REC_002", "values": {"fSTATUS": [{"text": "已通过"}], "fAPPROVER": [{"user_id": "manager_a"}]}},
    {"record_id": "REC_003", "values": {"fSTATUS": [{"text": "已通过"}], "fAPPROVER": [{"user_id": "manager_a"}]}}
  ]
}
```

## 场景六：销售订单（多种字段类型）

```json
{
  "add_records": [
    {
      "values": {
        "fCUSTOMER": "北京某科技有限公司",
        "fPRODUCT": [{"text": "企业版"}],
        "fAMOUNT": 58000,
        "fSIGN_DATE": "1741622400000",
        "fSALES": [{"user_id": "zhaoliu"}],
        "fCONTRACT": [{"text": "合同文件", "link": "https://doc.example.com/contract/2025-001"}]
      }
    }
  ]
}
```

## 场景七：会议纪要（文本、日期、链接）

```json
{
  "add_records": [
    {
      "values": {
        "fMEETING_TITLE": "支付模块需求评审会",
        "fDATE": "1741622400000",
        "fPARTICIPANTS": "产品、开发、测试",
        "fSUMMARY": "确定优先开发支付模块，目标 3 月底完成联调，4 月初上线。",
        "fDOC_LINK": [{"text": "评审文档", "link": "https://doc.example.com/meeting/20250310"}]
      }
    }
  ]
}
```

## 场景八：同一请求新增并更新

```json
{
  "add_records": [
    {
      "values": {
        "fTITLE": "用户反馈收集与分析",
        "fSTATUS": [{"text": "未开始"}],
        "fPRIORITY": [{"text": "高"}]
      }
    }
  ],
  "update_records": [
    {
      "record_id": "REC_OLD_001",
      "values": {
        "fSTATUS": [{"text": "已完成"}],
        "fPROGRESS": 100
      }
    }
  ]
}
```
