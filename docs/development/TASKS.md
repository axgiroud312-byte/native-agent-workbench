# 本轮开发任务

总任务：[原生 Agent 工作台 #22](https://github.com/axgiroud312-byte/native-agent-workbench/issues/22)。所有任务正文与进度维护在 GitHub；本页只维护入口和依赖，不复制 Issue 正文。

规范：[Spec 入口](../specs/README.md)。执行：[完整开发指令](IMPLEMENTATION.md)。

| 任务 | 交付成果 | 前置 | 需求/验收覆盖 |
| --- | --- | --- | --- |
| [T01 #23](https://github.com/axgiroud312-byte/native-agent-workbench/issues/23) | 建立 Cargo 工作区与统一数据契约 | — | R01–R13 的共同基础 |
| [T02 #24](https://github.com/axgiroud312-byte/native-agent-workbench/issues/24) | 搭建 GPUI 原生宿主与后台服务底座 | T01 | R01；支撑 R09/R10/R13 |
| [T03 #25](https://github.com/axgiroud312-byte/native-agent-workbench/issues/25) | 实现 SQLite、项目会话与草稿恢复 | T01 | R02/R13；A02/A10/A18 |
| [T04 #26](https://github.com/axgiroud312-byte/native-agent-workbench/issues/26) | 按已确认原型实现工作台布局、内容标签与设置路由 | T02、T03 | R01/R03/R12；A01/A03/A04/A17 |
| [T05 #27](https://github.com/axgiroud312-byte/native-agent-workbench/issues/27) | 接入 Registry 与 Codex ACP 发现、认证和配置 | T01、T02、T03 | R04/R05；A05/A06 |
| [T06 #28](https://github.com/axgiroud312-byte/native-agent-workbench/issues/28) | 实现 Composer、模型选择、命令、文件引用与图片输入 | T04、T05 | R05；A07 |
| [T07 #29](https://github.com/axgiroud312-byte/native-agent-workbench/issues/29) | 实现双会话运行、补充、队列、取消与断连恢复 | T03、T05、T06 | R06/R13；A08/A09/A10 |
| [T08 #30](https://github.com/axgiroud312-byte/native-agent-workbench/issues/30) | 实现结构化流式结果、工具计划、审批与提问控件 | T04、T07 | R07；A11/A12/A19 |
| [T09 #31](https://github.com/axgiroud312-byte/native-agent-workbench/issues/31) | 实现真实文件编辑、外部冲突和 Git 差异 | T03、T04 | R08；A03/A13/A14 |
| [T10 #32](https://github.com/axgiroud312-byte/native-agent-workbench/issues/32) | 实现原生内嵌浏览器与多网页标签 | T02、T04 | R09；A03/A15 |
| [T11 #33](https://github.com/axgiroud312-byte/native-agent-workbench/issues/33) | 实现真实 PTY 终端与独立终端生命周期 | T02、T04 | R10；A03/A16 |
| [T12 #34](https://github.com/axgiroud312-byte/native-agent-workbench/issues/34) | 完成辅助功能、桌面生命周期与一次集中交付 | T03、T04、T05、T06、T07、T08、T09、T10、T11 | R11/R12/R13；最终串联 A01–A19 |

## 执行政策

- 主 Agent 负责全部实现与整合；子 Agent 只做只读探查、API 核实、问题定位与独立审阅。
- 先 T01 统一数据契约；依赖允许时主 Agent 可交错推进相关功能。正常顺序按 T01–T12。
- 前 11 项以实现为主；中途只为实际错误/阻塞做最小检查。T12 集中完成必要检查和真实使用流程，不给每项重复配置测试任务。
- R01–R13 和 A01–A19 全部纳入本轮范围。A19 提问/问卷契约归 T08；最终使用闭环统一由 T12 收尾。
- 规划基线在 `planning/approved-workbench-v02`；开发从含本轮文档的提交起步。当前只有文档与设计，不能恢复已撤销的旧任务替代本轮实现。

