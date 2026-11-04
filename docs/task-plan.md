# MVP 任务路线

任务正文在 GitHub 原位维护，本文件只保留入口、顺序和验收对应关系，不复制 Issue 正文。

- [总任务 #2](https://github.com/axgiroud312-byte/native-agent-workbench/issues/2)
- [v0.1 MVP 里程碑](https://github.com/axgiroud312-byte/native-agent-workbench/milestone/1)
- [产品 Spec](specs/native-agent-workbench-mvp.md)
- [可开始任务](https://github.com/axgiroud312-byte/native-agent-workbench/issues?q=is%3Aopen+is%3Aissue+label%3Astatus%3Aready)

## 先做什么

**先完成 #3 业务对象，再完成 #4 命令/事件与状态更新；两项都完成后，才开始 UI、存储和 Runtime 功能开发。** 这些前置契约已实现；存储、Runtime、原生基础和设置服务接线已有编译与对应关键检查。完整会话/对话操作及实窗验收仍在推进，具体证据与未验证项见 README 和对应 Issue。见 [业务对象说明](architecture/domain-model.md) 和 [共享契约](architecture/contracts.md)。

创建任务时只有 #3 标记为 ready。后续负责人按实际完成证据更新状态；实时状态以 GitHub 为准。契约可以演进，但同一次变更必须同步类型、转换、样例和使用方。

下图仅为阶段概览，精确前置关系见下表和 GitHub blocked-by 关系。

```mermaid
flowchart LR
    A["#3 业务对象"] --> B["#4 命令与事件"]
    B --> C["#5 存储 · #6 UI 基础 · #7 Runtime"]
    C --> D["#8–#10 第一条真实闭环"]
    D --> E["#11–#16 完整对话交互"]
    E --> F["#17 并发恢复 · #18 托盘"]
    F --> G["#19 整体验收"]
```

## 可执行任务

| Issue | 交付结果 | 直接前置 |
| --- | --- | --- |
| [#3](https://github.com/axgiroud312-byte/native-agent-workbench/issues/3) | 统一业务对象、身份与生命周期 | 无，首个开发任务 |
| [#4](https://github.com/axgiroud312-byte/native-agent-workbench/issues/4) | 统一命令、事件、错误与状态更新契约 | [#3](https://github.com/axgiroud312-byte/native-agent-workbench/issues/3) |
| [#5](https://github.com/axgiroud312-byte/native-agent-workbench/issues/5) | 实现项目、会话、草稿与事件的持久化服务 | [#4](https://github.com/axgiroud312-byte/native-agent-workbench/issues/4) |
| [#6](https://github.com/axgiroud312-byte/native-agent-workbench/issues/6) | 建立克制的原生工作区与 UI 组合基础 | [#4](https://github.com/axgiroud312-byte/native-agent-workbench/issues/4) |
| [#7](https://github.com/axgiroud312-byte/native-agent-workbench/issues/7) | 接通 Codex ACP Runtime 的真实生命周期 | [#4](https://github.com/axgiroud312-byte/native-agent-workbench/issues/4) |
| [#8](https://github.com/axgiroud312-byte/native-agent-workbench/issues/8) | 完成 Agent 发现、安装连接与应用设置 | [#5](https://github.com/axgiroud312-byte/native-agent-workbench/issues/5)、[#6](https://github.com/axgiroud312-byte/native-agent-workbench/issues/6)、[#7](https://github.com/axgiroud312-byte/native-agent-workbench/issues/7) |
| [#9](https://github.com/axgiroud312-byte/native-agent-workbench/issues/9) | 接通项目与会话的创建、切换和删除 | [#5](https://github.com/axgiroud312-byte/native-agent-workbench/issues/5)、[#6](https://github.com/axgiroud312-byte/native-agent-workbench/issues/6)、[#8](https://github.com/axgiroud312-byte/native-agent-workbench/issues/8) |
| [#10](https://github.com/axgiroud312-byte/native-agent-workbench/issues/10) | 跑通原生输入、真实流式对话与停止闭环 | [#9](https://github.com/axgiroud312-byte/native-agent-workbench/issues/9)、[#7](https://github.com/axgiroud312-byte/native-agent-workbench/issues/7) |
| [#11](https://github.com/axgiroud312-byte/native-agent-workbench/issues/11) | 接通模型、思考强度与访问权限控件 | [#10](https://github.com/axgiroud312-byte/native-agent-workbench/issues/10) |
| [#12](https://github.com/axgiroud312-byte/native-agent-workbench/issues/12) | 完成图片、文件引用与斜杠指令输入 | [#10](https://github.com/axgiroud312-byte/native-agent-workbench/issues/10) |
| [#13](https://github.com/axgiroud312-byte/native-agent-workbench/issues/13) | 整理流式内容、工具和计划的原生呈现 | [#10](https://github.com/axgiroud312-byte/native-agent-workbench/issues/10) |
| [#14](https://github.com/axgiroud312-byte/native-agent-workbench/issues/14) | 接通权限审批和 Agent 提问控件 | [#10](https://github.com/axgiroud312-byte/native-agent-workbench/issues/10) |
| [#15](https://github.com/axgiroud312-byte/native-agent-workbench/issues/15) | 展示按会话归属的只读文件差异 | [#13](https://github.com/axgiroud312-byte/native-agent-workbench/issues/13) |
| [#16](https://github.com/axgiroud312-byte/native-agent-workbench/issues/16) | 完成途中补充、排队与停止后的输入管理 | [#11](https://github.com/axgiroud312-byte/native-agent-workbench/issues/11)、[#12](https://github.com/axgiroud312-byte/native-agent-workbench/issues/12)、[#14](https://github.com/axgiroud312-byte/native-agent-workbench/issues/14) |
| [#17](https://github.com/axgiroud312-byte/native-agent-workbench/issues/17) | 打通多会话隔离、断连与重启恢复 | [#5](https://github.com/axgiroud312-byte/native-agent-workbench/issues/5)、[#13](https://github.com/axgiroud312-byte/native-agent-workbench/issues/13)、[#14](https://github.com/axgiroud312-byte/native-agent-workbench/issues/14)、[#15](https://github.com/axgiroud312-byte/native-agent-workbench/issues/15)、[#16](https://github.com/axgiroud312-byte/native-agent-workbench/issues/16) |
| [#18](https://github.com/axgiroud312-byte/native-agent-workbench/issues/18) | 实现托盘继续运行与可控退出 | [#17](https://github.com/axgiroud312-byte/native-agent-workbench/issues/17) |
| [#19](https://github.com/axgiroud312-byte/native-agent-workbench/issues/19) | 完成前端质量、Windows 体验与 MVP 验收 | [#8](https://github.com/axgiroud312-byte/native-agent-workbench/issues/8)、[#9](https://github.com/axgiroud312-byte/native-agent-workbench/issues/9)、[#11](https://github.com/axgiroud312-byte/native-agent-workbench/issues/11)、[#12](https://github.com/axgiroud312-byte/native-agent-workbench/issues/12)、[#13](https://github.com/axgiroud312-byte/native-agent-workbench/issues/13)、[#14](https://github.com/axgiroud312-byte/native-agent-workbench/issues/14)、[#15](https://github.com/axgiroud312-byte/native-agent-workbench/issues/15)、[#16](https://github.com/axgiroud312-byte/native-agent-workbench/issues/16)、[#17](https://github.com/axgiroud312-byte/native-agent-workbench/issues/17)、[#18](https://github.com/axgiroud312-byte/native-agent-workbench/issues/18) |

父子关系表示归属，blocked-by 表示必须先取得的成果；它们不能互相替代。总任务在真实 MVP 和用户验收完成前保持打开，不因创建完任务而关闭。

## 共享修改负责人

| 共享部分 | 负责维护的任务 |
| --- | --- |
| 业务对象、ID、生命周期和序列化含义 | #3；后续修改仍由契约负责人统筹 |
| 命令、事件、错误、状态更新和端口契约 | #4 |
| SQLite 迁移、事件/附件保存与所有权 | #5 |
| 窗口组合、主题 Token、共用控件和焦点规则 | #6 |
| ACP/Agent 原始数据映射和进程生命周期 | #7 |
| 跨功能联调、并发与重启证据 | #17 |
| 关闭/托盘/退出协调 | #18 |
| 集中质量检查与完整验收记录 | #19 |

“负责人”指承接该任务的实现者，尚未预先指派具体人员。每个功能任务要把所需 UI、命令和存储/运行时连接起来，不能只提交一个孤立按钮或接口。

## 前端的共同完成条件

- 参考范围包括整个对话、流式内容、工具/计划/问题/审批/差异，不只包括输入框外形。具体来源见 Spec 第 2 节，实施时核对对应源码。
- 正常页面通过布局、对齐、控件当前值和主次说明功能。没有必要的新信息就不加副标题、次要标签或说明段落。
- 例如模型控件显示实际模型名，不重复“请选择模型”；已有打开项目动作时不再添同义教程。完整路径只在消除歧义或定位目录时出现。
- 错误原因、权限对象/范围和不可逆后果属于必要信息。减少界面说明不能删掉这些内容，更不能删改 Agent 原文。
- 连续正文保持文档式阅读；结构化内容使用合适原生控件，完整内容可展开查看。使用统一真实图标，不以装饰和卡片堆砌代替层级。
- 用实际长度的中文、代码、图片和多会话状态检查，包含最小窗口、缩放、键盘/IME、空/加载/失败/待处理状态。
- 目前没有已确认的独立 UI 原型；已有启动窗口是工程样例。独立原型流程由用户另行调用，不能伪称有已验收设计。
- 普通检查在目标收尾统一做；明确区分实现、真实调用/窗口验证与用户验收。

## 验收覆盖

每项 Issue 已引用具体 R 需求和 AC 验收。以下列出最终整体验收 #19 之外的直接实施任务；#19 统一完成所有验收和未覆盖项记录。

| Spec 验收 | 直接实施任务 |
| --- | --- |
| AC-01 | [#6](https://github.com/axgiroud312-byte/native-agent-workbench/issues/6) |
| AC-02 | [#5](https://github.com/axgiroud312-byte/native-agent-workbench/issues/5)、[#6](https://github.com/axgiroud312-byte/native-agent-workbench/issues/6)、[#9](https://github.com/axgiroud312-byte/native-agent-workbench/issues/9) |
| AC-03 | [#7](https://github.com/axgiroud312-byte/native-agent-workbench/issues/7)、[#8](https://github.com/axgiroud312-byte/native-agent-workbench/issues/8)、[#10](https://github.com/axgiroud312-byte/native-agent-workbench/issues/10) |
| AC-04 | [#3](https://github.com/axgiroud312-byte/native-agent-workbench/issues/3)、[#4](https://github.com/axgiroud312-byte/native-agent-workbench/issues/4)、[#7](https://github.com/axgiroud312-byte/native-agent-workbench/issues/7)、[#9](https://github.com/axgiroud312-byte/native-agent-workbench/issues/9)、[#17](https://github.com/axgiroud312-byte/native-agent-workbench/issues/17) |
| AC-05 | [#4](https://github.com/axgiroud312-byte/native-agent-workbench/issues/4)、[#7](https://github.com/axgiroud312-byte/native-agent-workbench/issues/7)、[#8](https://github.com/axgiroud312-byte/native-agent-workbench/issues/8)、[#11](https://github.com/axgiroud312-byte/native-agent-workbench/issues/11) |
| AC-06 | [#5](https://github.com/axgiroud312-byte/native-agent-workbench/issues/5)、[#10](https://github.com/axgiroud312-byte/native-agent-workbench/issues/10)、[#12](https://github.com/axgiroud312-byte/native-agent-workbench/issues/12)、[#16](https://github.com/axgiroud312-byte/native-agent-workbench/issues/16) |
| AC-07 | [#12](https://github.com/axgiroud312-byte/native-agent-workbench/issues/12) |
| AC-08 | [#10](https://github.com/axgiroud312-byte/native-agent-workbench/issues/10)、[#13](https://github.com/axgiroud312-byte/native-agent-workbench/issues/13)、[#17](https://github.com/axgiroud312-byte/native-agent-workbench/issues/17) |
| AC-09 | [#4](https://github.com/axgiroud312-byte/native-agent-workbench/issues/4)、[#7](https://github.com/axgiroud312-byte/native-agent-workbench/issues/7)、[#13](https://github.com/axgiroud312-byte/native-agent-workbench/issues/13)、[#14](https://github.com/axgiroud312-byte/native-agent-workbench/issues/14) |
| AC-10 | [#15](https://github.com/axgiroud312-byte/native-agent-workbench/issues/15)、[#17](https://github.com/axgiroud312-byte/native-agent-workbench/issues/17) |
| AC-11 | [#4](https://github.com/axgiroud312-byte/native-agent-workbench/issues/4)、[#16](https://github.com/axgiroud312-byte/native-agent-workbench/issues/16) |
| AC-12 | [#4](https://github.com/axgiroud312-byte/native-agent-workbench/issues/4)、[#7](https://github.com/axgiroud312-byte/native-agent-workbench/issues/7)、[#8](https://github.com/axgiroud312-byte/native-agent-workbench/issues/8)、[#10](https://github.com/axgiroud312-byte/native-agent-workbench/issues/10)、[#14](https://github.com/axgiroud312-byte/native-agent-workbench/issues/14)、[#16](https://github.com/axgiroud312-byte/native-agent-workbench/issues/16)、[#17](https://github.com/axgiroud312-byte/native-agent-workbench/issues/17) |
| AC-13 | [#18](https://github.com/axgiroud312-byte/native-agent-workbench/issues/18) |
| AC-14 | [#3](https://github.com/axgiroud312-byte/native-agent-workbench/issues/3)、[#5](https://github.com/axgiroud312-byte/native-agent-workbench/issues/5)、[#9](https://github.com/axgiroud312-byte/native-agent-workbench/issues/9)、[#16](https://github.com/axgiroud312-byte/native-agent-workbench/issues/16)、[#17](https://github.com/axgiroud312-byte/native-agent-workbench/issues/17) |
| AC-15 | [#6](https://github.com/axgiroud312-byte/native-agent-workbench/issues/6)、[#8](https://github.com/axgiroud312-byte/native-agent-workbench/issues/8)、[#11](https://github.com/axgiroud312-byte/native-agent-workbench/issues/11)、[#12](https://github.com/axgiroud312-byte/native-agent-workbench/issues/12)、[#13](https://github.com/axgiroud312-byte/native-agent-workbench/issues/13)、[#14](https://github.com/axgiroud312-byte/native-agent-workbench/issues/14)、[#18](https://github.com/axgiroud312-byte/native-agent-workbench/issues/18) |
| AC-16 | [#5](https://github.com/axgiroud312-byte/native-agent-workbench/issues/5)、[#17](https://github.com/axgiroud312-byte/native-agent-workbench/issues/17) |
| AC-17 | [#10](https://github.com/axgiroud312-byte/native-agent-workbench/issues/10) |
| AC-18 | [#6](https://github.com/axgiroud312-byte/native-agent-workbench/issues/6)、[#8](https://github.com/axgiroud312-byte/native-agent-workbench/issues/8)、[#9](https://github.com/axgiroud312-byte/native-agent-workbench/issues/9)、[#10](https://github.com/axgiroud312-byte/native-agent-workbench/issues/10)、[#11](https://github.com/axgiroud312-byte/native-agent-workbench/issues/11)、[#12](https://github.com/axgiroud312-byte/native-agent-workbench/issues/12)、[#13](https://github.com/axgiroud312-byte/native-agent-workbench/issues/13)、[#14](https://github.com/axgiroud312-byte/native-agent-workbench/issues/14)、[#15](https://github.com/axgiroud312-byte/native-agent-workbench/issues/15)、[#16](https://github.com/axgiroud312-byte/native-agent-workbench/issues/16)、[#17](https://github.com/axgiroud312-byte/native-agent-workbench/issues/17)、[#18](https://github.com/axgiroud312-byte/native-agent-workbench/issues/18) |
| AC-19 | [#3](https://github.com/axgiroud312-byte/native-agent-workbench/issues/3)、[#4](https://github.com/axgiroud312-byte/native-agent-workbench/issues/4)、[#5](https://github.com/axgiroud312-byte/native-agent-workbench/issues/5)、[#6](https://github.com/axgiroud312-byte/native-agent-workbench/issues/6)、[#7](https://github.com/axgiroud312-byte/native-agent-workbench/issues/7)、[#17](https://github.com/axgiroud312-byte/native-agent-workbench/issues/17) |

## 当前假设与讨论边界

Windows 优先、Codex 优先、同目录并发、默认请求完整访问、能力驱动控件、两种发送方式、托盘继续、只读差异和不导入外部历史均已有用户选择，不重新询问。

具体快捷键映射、进程复用、附件限额和 Windows 打包形式是 Spec 中注明的工程选择，由相关任务核实。只有新事实会改变已选产品体验时才提出具体场景继续讨论，不以普通技术细节阻塞全部任务。
