# 获取会议详情 (get_meeting_info) - 返回参数

## 返回参数

```json
{
  "errcode": 0,
  "errmsg": "ok",
  "creator_userid": "创建者userid",
  "admin_userid": "会议管理userid (与 creator_userid 有且仅返回一个)",
  "title": "会议标题",
  "meeting_start_datetime": "YYYY-MM-DD HH:mm",
  "meeting_duration": "会议时长秒数",
  "description": "会议描述文本",
  "location": "会议地点文本",
  "main_department": "创建者主部门ID",
  "status": "会议状态枚举值",
  "meeting_type": "会议类型枚举值",
  "attendees": {
    "member": [
      {
        "userid": "内部成员userid",
        "status": "与会状态枚举值",
        "first_join_datetime": "YYYY-MM-DD HH:mm",
        "last_quit_datetime": "YYYY-MM-DD HH:mm",
        "total_join_count": "加入次数",
        "cumulative_time": "累计在会时长秒数"
      }
    ],
    "tmp_external_user": [
      {
        "tmp_external_userid": "外部临时用户ID",
        "status": "与会状态枚举值",
        "first_join_datetime": "YYYY-MM-DD HH:mm",
        "last_quit_datetime": "YYYY-MM-DD HH:mm",
        "total_join_count": "加入次数",
        "cumulative_time": "累计在会时长秒数"
      }
    ]
  },
  "settings": {
    "remind_scope": "提醒范围枚举值",
    "need_password": "是否需要密码布尔值",
    "password": "会议密码",
    "enable_waiting_room": "是否启用等候室布尔值",
    "allow_enter_before_host": "是否允许提前入会布尔值",
    "enable_enter_mute": "入会静音枚举值",
    "allow_unmute_self": "是否允许自我解除静音布尔值",
    "allow_external_user": "是否允许外部用户布尔值",
    "enable_screen_watermark": "是否开启水印布尔值",
    "watermark_type": "水印类型枚举值",
    "auto_record_type": "录制类型枚举字符串",
    "attendee_join_auto_record": "参会者加入自动录制布尔值",
    "enable_host_pause_auto_record": "主持人可暂停录制布尔值",
    "enable_doc_upload_permission": "允许上传文档布尔值",
    "enable_enroll": "是否开启报名布尔值",
    "enable_host_key": "是否启用主持人密钥布尔值",
    "host_key": "主持人密钥字符串",
    "hosts": {"userid": ["主持人userid列表"]},
    "current_hosts": {"userid": ["当前主持人userid列表"]},
    "co_hosts": {"userid": ["联席主持人userid列表"]},
    "ring_users": {"userid": ["响铃用户userid列表"]}
  },
  "meeting_code": "会议号码字符串",
  "meeting_link": "会议链接URL",
  "has_vote": "是否有投票布尔值",
  "has_more_sub_meeting": "是否还有更多子会议枚举值",
  "remain_sub_meetings": "剩余子会议场数",
  "current_sub_meetingid": "当前子会议ID",
  "guests": [
    {
      "area": "国际区号",
      "phone_number": "手机号字符串",
      "guest_name": "嘉宾姓名"
    }
  ],
  "reminders": {
    "is_repeat": "是否周期性枚举值",
    "repeat_type": "重复类型枚举值",
    "repeat_until_type": "结束类型枚举值",
    "repeat_until_count": "限定次数",
    "repeat_until_datetime": "YYYY-MM-DD HH:mm",
    "repeat_interval": "重复间隔数值",
    "is_custom_repeat": "是否自定义重复枚举值",
    "repeat_day_of_week": ["星期几数组"],
    "repeat_day_of_month": ["日期数组"],
    "remind_before": ["提醒秒数数组"]
  },
  "sub_meetings": [
    {
      "sub_meetingid": "子会议ID",
      "status": "子会议状态枚举值",
      "start_datetime": "YYYY-MM-DD HH:mm",
      "end_datetime": "YYYY-MM-DD HH:mm",
      "title": "子会议标题",
      "repeat_id": "周期性会议分段ID"
    }
  ],
  "sub_repeat_list": [
    {
      "repeat_id": "周期性会议分段ID",
      "repeat_type": "重复类型枚举值",
      "repeat_until_type": "结束类型枚举值",
      "repeat_until_count": "限定次数",
      "repeat_until_datetime": "YYYY-MM-DD HH:mm",
      "repeat_interval": "重复间隔数值",
      "is_custom_repeat": "是否自定义重复枚举值",
      "repeat_day_of_week": ["星期几数组"],
      "repeat_day_of_month": ["日期数组"]
    }
  ]
}
```

## 关键返回字段

| 字段                                      | 类型    | 说明                                                                                                          |
| ----------------------------------------- | ------- | ------------------------------------------------------------------------------------------------------------- |
| `creator_userid`                        | string  | 创建者 userid，与 `admin_userid` 有且仅返回一个                                                             |
| `admin_userid`                          | string  | 会议管理 userid，与 `creator_userid` 有且仅返回一个                                                         |
| `title`                                 | string  | 会议标题                                                                                                      |
| `meeting_start_datetime`                | string  | 会议开始时间                                                                                                  |
| `meeting_duration`                      | integer | 会议时长 (秒)                                                                                                 |
| `main_department`                       | integer | 创建者所属主部门                                                                                              |
| `status`                                | integer | 会议状态 (1: 待开始，2: 会议中，3: 已结束，4: 已取消，5: 已过期)                                              |
| `meeting_type`                          | integer | 会议类型 (0: 一次性会议，1: 周期性会议，2: 微信专属会议，3: Rooms 投屏会议，5: 个人会议号会议，6: 网络研讨会) |
| `meeting_code`                          | string  | 会议号码                                                                                                      |
| `meeting_link`                          | string  | 会议链接                                                                                                      |
| `attendees.member`                      | array   | 内部参与者列表                                                                                                |
| `attendees.member[].status`             | integer | 与会状态 (1: 已参与，2: 未参与)                                                                               |
| `attendees.tmp_external_user`           | array   | 外部参与者 (临时 ID)                                                                                          |
| `attendees.tmp_external_user[].status`  | integer | 与会状态 (1: 已参与，2: 未参与)                                                                               |
| `guests`                                | array   | 外部嘉宾列表，每项含 `area`，`phone_number`，`guest_name`                                               |
| `current_sub_meetingid`                 | string  | 当前子会议 ID                                                                                                 |
| `settings.ring_users`                   | object  | 响铃用户列表                                                                                                  |
| `settings.need_password`                | boolean | 是否需要密码 (只读字段)                                                                                       |
| `settings.enable_doc_upload_permission` | boolean | 是否允许成员上传文档                                                                                          |
| `settings.hosts`                        | object  | 主持人列表                                                                                                    |
| `settings.current_hosts`                | object  | 当前主持人列表                                                                                                |
| `settings.co_hosts`                     | object  | 联席主持人列表                                                                                                |
| `reminders`                             | object  | 周期性配置                                                                                                    |
| `has_vote`                              | boolean | 是否有投票 (仅会议创建人和主持人有权限查询)                                                                   |
| `has_more_sub_meeting`                  | integer | 是否还有更多子会议特例 (0: 无更多，1: 有更多)                                                                 |
| `remain_sub_meetings`                   | integer | 剩余子会议场数                                                                                                |
| `sub_meetings`                          | array   | 子会议列表                                                                                                    |
| `sub_meetings[].status`                 | integer | 子会议状态 (0: 默认/存在，1: 已删除)                                                                          |
| `sub_meetings[].repeat_id`              | string  | 周期性会议分段 ID，用于关联子会议所属分段                                                                     |
| `sub_repeat_list`                       | array   | 周期性会议分段信息，修改周期性会议某一场后可能产生不同分段，各分段有不同重复规则                              |
