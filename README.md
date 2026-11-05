# Agent Workbench

Rust + GPUI / gpui-component 原生 Agent 工作台。

项目已按新的需求重新规划。用户已确认可交互 HTML 原型 v0.2，当前交付设计、开发规格和可执行任务；正式原生实现尚未开始。

- [已确认设计原型](docs/design/design-board.html)：下载后可直接用浏览器打开；Agent、网页与终端在原型中为模拟。
- [开发规范](docs/specs/README.md)：产品行为、统一数据契约、架构与设计 Token。
- [任务索引](docs/development/TASKS.md)：总任务 #22、开发任务 #23–#34 及依赖关系。
- [完整开发指令](docs/development/IMPLEMENTATION.md)：主 Agent 集中实现，子 Agent 只做探查与辅助，必要验证统一收尾。

主要功能包括项目与会话管理、Codex ACP、图片与文件输入、并发与队列、结构化回复、文件编辑、内嵌浏览器、真实终端和独立设置页。

已确认原型作为视觉基准。正式应用使用原生 GPUI，开发从统一数据结构开始；原型源码和固定数据不作为正式前端实现。

## 当前状态

本轮文档位于 `planning/approved-workbench-v02` 分支。旧实现与旧任务已撤销；请使用本轮 [总任务 #22](https://github.com/axgiroud312-byte/native-agent-workbench/issues/22) 作为执行入口。

许可证：[MIT](LICENSE)。原型嵌入图标的来源及许可证见原型“关于原型”。
