<!-- 仅本地 vLLM 弱模型(千问 3.5 系)追加的脚手架段;强模型不注入。维护注意:与 instructions.md 主干同源维护,改动需同步评估消融。 -->

## 本地模型补充
- 算术 / 跑脚本用 `exec_shell python3 -c '...'`;git 操作用 `exec_shell git …`(如 `git log`),不要编 `git_log` 之类不存在的工具名。
- 工具调用前的前言一两句就够了,别长篇铺垫策略。
