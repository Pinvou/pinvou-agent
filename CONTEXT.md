# pinvou3 远程界面上下文

本上下文描述 pinvou3 桌面实例通过浏览器提供完整远程界面时使用的统一语言。

## Language

**桌面实例（Desktop Instance）**：
一份正在运行或可重新启动的 pinvou3 本地安装，是 Agent 执行、业务数据和本地能力的唯一归属方。
_Avoid_: 设备房间、远控 Session

**完整 WebUI（Full WebUI）**：
桌面界面在浏览器中的同源形态，提供相同的业务界面，并允许必要的平台和响应式差异。
_Avoid_: 手机远控页、Web 管理后台

**Web 客户端（Web Client）**：
通过电脑、平板或手机浏览器访问完整 WebUI 的单个交互端。
_Avoid_: 手机端、Mobile

**Web 访问端点（Web Access Endpoint）**：
绑定一个桌面实例的持久远程入口；它跨桌面实例重启保持有效，直到用户主动停止或刷新访问凭证。
_Avoid_: Session 房间、临时二维码房间

**Relay**：
在 Web 客户端与桌面实例之间传输请求、事件和允许下载的内容，但不拥有 Agent 执行权或用户业务数据。
_Avoid_: 云端 Agent、数据源

**Session**：
桌面实例拥有的一段 Agent 会话；Web 客户端可以独立选择 Session，但选择结果不改变桌面界面当前打开的 Session。
_Avoid_: Web 访问端点、Room

**产物（Artifact）**：
由 Session 产生并登记的可预览、编辑或下载成果；完整 WebUI 的下载能力仅面向产物。
_Avoid_: 任意桌面文件
