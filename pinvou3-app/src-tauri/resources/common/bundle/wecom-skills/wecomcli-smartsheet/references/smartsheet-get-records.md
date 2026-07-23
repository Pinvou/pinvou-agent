# smartsheet_get_records API

查询智能表格中指定子表的记录信息，支持分页读取。支持通过 docid 或文档 URL 定位文档，二者传入其一即可。

## 技能定义

```json
{
  "name": "smartsheet_get_records",
  "description": "查询智能表格中指定子表的记录信息。支持分页查询（cursor + limit），不填 cursor 从第一行开始。支持通过 docid 或文档 URL 定位文档，二者传入其一即可。",
  "inputSchema": {
    "properties": {
      "cursor": {
        "anyOf": [
          {
            "type": "string"
          },
          {
            "type": "null"
          }
        ],
        "default": null,
        "description": "查询游标，不填代表从第一行记录开始查询",
        "title": "Cursor"
      },
      "docid": {
        "anyOf": [
          {
            "type": "string"
          },
          {
            "type": "null"
          }
        ],
        "default": null,
        "description": "文档的 docid，与 url 二选一传入",
        "title": "Docid"
      },
      "limit": {
        "anyOf": [
          {
            "type": "integer"
          },
          {
            "type": "null"
          }
        ],
        "default": null,
        "description": "分页大小，每页返回多少条数据；当不填写该参数或将该参数设置为 0 时，如果总数大于 1000，一次性返回 1000 行记录，当总数小于 1000 时，返回全部记录；limit 最大值为 1000",
        "title": "Limit"
      },
      "sheet_id": {
        "description": "子表的 sheet_id，用于指定要查询的智能表格中的哪个子表",
        "title": "Sheet Id",
        "type": "string"
      },
      "url": {
        "anyOf": [
          {
            "type": "string"
          },
          {
            "type": "null"
          }
        ],
        "default": null,
        "description": "文档的访问链接，与 docid 二选一传入",
        "title": "Url"
      }
    },
    "required": [
      "sheet_id"
    ],
    "title": "smartsheet_get_recordsArguments",
    "type": "object"
  }
}
```

## 参数说明

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| cursor | string | 否 | 查询游标，不填代表从第一行记录开始查询 |
| docid | string | 与 url 二选一 | 文档的 docid |
| limit | integer | 否 | 分页大小，每页返回多少条数据；当不填写该参数或将该参数设置为 0 时，如果总数大于 1000，一次性返回 1000 行记录，当总数小于 1000 时，返回全部记录；limit 最大值为 1000 |
| sheet_id | string | 是 | 子表的 sheet_id，用于指定要查询的智能表格中的哪个子表 |
| url | string | 与 docid 二选一 | 文档的访问链接 |

## 请求示例

以传入docid为例：

```json
{
    "docid": "DOCID",
    "sheet_id": "123Abc"
}
```

以传url为例：

```json
{
    "url": "https://doc.weixin.qq.com/smartsheet/xxx",
    "sheet_id": "123Abc"
}
```

## 响应示例（含分页）

```json
{
    "errcode": 0,
    "errmsg": "ok",
    "total": 100,
    "has_more": true,
    "next_cursor": "mock_cursor_token",
    "records": [
        {
            "record_id": "rec_001",
            "create_time": "1700000000000",
            "update_time": "1700000000000",
            "values": {
                "成员": [
                    {"user_id": "real.user001", "id_type": 1}
                ],
                "序号": [
                    {"text": "1", "type": "text"}
                ],
                "附件": [
                    {
                        "doc_type": 2,
                        "file_ext": "xlsx",
                        "file_type": "Wedrive",
                        "file_url": "https://drive.weixin.qq.com/s?k=MOCK_TOKEN",
                        "name": "report.xlsx",
                        "size": 12345
                    }
                ]
            },
            "creator_name": "张三",
            "updater_name": "张三"
        },
        {
            "record_id": "rec_002",
            "create_time": "1700000000000",
            "update_time": "1700000000000",
            "values": {
                "截图": [
                    {
                        "height": 1080,
                        "id": "img_mock_id",
                        "image_url": "https://wdcdn.qpic.cn/mocked_path?w=1920&h=1080",
                        "title": "screenshot_001",
                        "width": 1920
                    }
                ],
                "序号": [
                    {"text": "2", "type": "text"}
                ]
            },
            "creator_name": "张三",
            "updater_name": "李四"
        }
    ]
}
```

## 响应数据结构

- `total`: 总记录数（integer）
- `has_more`: 是否还有更多数据（boolean）
- `next_cursor`: 下一页游标，用于分页（string，仅当 has_more 为 true 时存在）
- `records`: 记录数组


## 分页查询

```bash
# 第一页
wecom-cli doc smartsheet_get_records '{"docid": "DOCID", "sheet_id": "SHEETID"}'

# 下一页（使用上一条的 next_cursor）
wecom-cli doc smartsheet_get_records '{"docid": "DOCID", "sheet_id": "SHEETID", "cursor": "上一条的 next_cursor 值"}'
```

