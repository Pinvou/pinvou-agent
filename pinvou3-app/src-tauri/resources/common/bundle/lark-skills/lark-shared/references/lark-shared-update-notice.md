# 更新与 `_notice`

lark-cli 命令执行后，如果检测到新版本，JSON 输出中会包含 `_notice.update` 字段（含 `message`、`command` 等）。

除非用户正在询问更新、版本或 notice，否则不要把 `_notice` 原样复制为当前任务的主要答案，也不要为了 notice 中断当前任务去反复查 help。

需要稳定 JSON 给脚本或机器读取时，可以在命令前设置：

```bash
LARKSUITE_CLI_NO_UPDATE_NOTIFIER=1 LARKSUITE_CLI_NO_SKILLS_NOTIFIER=1 <lark-cli command>
```

当你在输出中看到 `_notice.update` 时，先完成用户当前请求；如仍相关，再简短告知用户：品悟内 lark-cli 由应用按锁文件钉扎分发与升级，**不要执行 `lark-cli update` 自行更新**（自行更新会脱离品悟校验，导致下次使用时被重装回钉扎版本）；新版本会随品悟应用更新自动就位。

另外两类 notice：
- `_notice.skills`：本地 Skills 与当前 CLI 不同步。品悟内 skills 随应用分发，同样**不要**自行运行更新命令，等品悟应用更新即可。
- `_notice.deprecated_command`：本次使用了兼容保留的旧命令；后续调用改用 `replacement`。
