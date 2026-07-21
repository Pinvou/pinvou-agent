# 桌面实例继续拥有执行权和业务数据

完整 WebUI 只是桌面实例的远程界面，Agent、Session、设置、知识库、工具和文件能力仍由桌面实例执行与持有。Relay 只负责鉴权和盲转发，不保存事件窗口或用户业务内容；断线补发所需的有界事件 journal、游标和重置语义全部由桌面实例维护。跨 UI 写 transcript 使用桌面 SessionStore 内的内容 revision 原子 CAS，同一 Session 的 Engine 回合也只在桌面提交边界接受一次。
