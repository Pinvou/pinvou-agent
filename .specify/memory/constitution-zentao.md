# 禅道问题解决方案

**适用范围**: 本规范适用于项目中BUG解决的方案。

## 一、禅道BUG获取

- 本项目的BUG记录在禅道上，禅道地址为<https://itplan.h3c.com/>，本项目BUG记录在 测试 -【MegaBook】MegaBook PTL - Windows - 品悟 路径下，你可以【优先】通过<https://itplan.h3c.com/bug-browse-1193--byModule-23311.html>获取本项目的BUG列表。
- 使用zendao.exe获取禅道BUG信息，并使用zentao.exe所配置的用户名和密码进行禅道的登录操作。

## 二、版本信息的获取

- 当前项目的修复版本名称为【Win_Pinvou3_0.7.0】
- 你可以在<https://itplan.h3c.com/execution-build-5723.html>页面找到当前项目的所有版本信息，并找到包含上述版本名称的数据栏，其中的ID列，即为版本ID，记为buildID

## 三、修复后设置禅道BUG状态

- 当确认BUG已经修复完成后，应当先将修复的代码提交到github 和 gitlab 上，然后再设置禅道上相应BUG的状态为【已解决】，设置禅道的方法如下：

1. 先调用：POST https://itplan.h3c.com/api.php/v1/tokens
这个接口返回 REST Token，同时响应头会带登录态 Cookie，比如 zentaosid。这里的重点是拿到网页会话 Cookie。
在 PowerShell 中调用 Invoke-WebRequest 时 SHOULD 使用 `-UseBasicParsing`，避免交互式安全确认中断流程。

2. 用这个 Cookie 打开解决页面：
GET https://itplan.h3c.com/bug-resolve-<BugID>.html
从页面里解析到表单字段，包括：kuid、buildExecution、assignedTo、resolvedBuild、resolvedDate。
其中 `kuid` 不是表单 input，而是页面脚本变量（例如 `var kuid = 'xxxx';`），提交时通过 `uid=<kuid>` 传递。

3. 再用同一个 Cookie 提交页面表单：
POST https://itplan.h3c.com/bug-resolve-<BugID>.html
表单里提交这些关键字段：
resolution=fixed
duplicateBug=0
resolvedBuild=<buildID>
buildExecution=<buildExecution>
assignedTo=<assignedTo>
status=resolved
comment=<完整解决备注>
uid=<kuid>
resolvedDate=<resolvedDate>

其中：
- `assignedTo` SHOULD 使用页面当前已选值；若页面未返回已选值，可回退为当前登录账号。
- `resolvedBuild` MUST 使用当前项目版本对应 buildID（例如 Win_Pinvou3_0.7.0 对应 3476）。

同时带上页面请求头：
Referer
Origin
User-Agent
X-Requested-With=XMLHttpRequest
Accept=application/json, text/javascript, */*; q=0.01
成功时返回内容里有：
parent.location='/bug-view-<BugID>.html'

4. 最后回查确认：
REST GET /api.php/v1/bugs/<BugID> 显示 status=resolved
resolution=fixed
resolvedBuild=3476 / Win_Pinvou3_0.7.0（或当前目标版本）
actions 里出现 resolved 动作，并且 comment 是完整备注
active 列表不再出现这个 Bug

5. 【完整解决备注】 的内容为：
原因分析：简要描述BUG产生的原因
解决方案：简要描述解决BUG的思路，注意不必写出代码实现细节
提交链接：git的提交链接

## 四、已验证流程（2026-06-05）

以下流程已在 Bug 53099 上验证成功：

1. `POST /api.php/v1/tokens` 获取 Cookie 会话。
2. `GET /bug-resolve-53099.html`，从页面中提取：
	- `kuid`（脚本变量）
	- `buildExecution`
	- `assignedTo`
	- `resolvedBuild`（使用目标版本 buildID，示例为 3432）
	- `resolvedDate`
3. `POST /bug-resolve-53099.html`，提交 `resolution=fixed` 等字段。
4. 响应命中 `parent.location='/bug-view-53099.html'`。
5. `GET /api.php/v1/bugs/53099` 回查得到：
	- `status=resolved`
	- `resolution=fixed`
	- `resolvedBuild.id=3432`