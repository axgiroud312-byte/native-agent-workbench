# 原生工作区组合（Issue #6）

本层建立可承载后续功能的 Windows 原生窗口。业务对象、身份与运行判断直接读取 `workbench_core` 快照；正式启动尚未连接存储和 Runtime，不表示完整 Agent 产品已经可用。

## 文件与状态归属

| 文件 | 职责 |
| --- | --- |
| `main.rs` | 应用初始化、快捷键、原生窗口尺寸与生命周期 |
| `theme.rs` | 深色语义颜色、字体、间距、圆角、缩放与尺寸范围 |
| `workbench.rs` | 双栏组合、保留的输入/树/滚动实体、Action、原生目录选择与设置 Dialog |
| `sidebar.rs` | 项目菜单、虚拟化原生 Tree、会话状态、底部固定设置 |
| `conversation.rs` | 会话上下文、空/错误状态、MessageScroller 与开发内容基础呈现 |
| `composer.rs` | 固定输入容器、草稿文本、只读确认配置与待处理区域 |
| `host.rs` | core 快照与视图偏好、带身份的 UI 意图边界 |
| `fixture.rs` | 显式开发入口、共享 core 样例回放和额外布局状态 |

没有复制业务状态机。连接、执行、队列、权限与提问状态来自 `SessionSnapshot` / `SessionSummary`；不根据文本推断 Agent 能力。模型名优先按确认配置的 ID 查 Agent 返回的能力，找不到标签时保留原 ID。

GPUI 实体只保存交互状态：输入编辑器、树选择、分栏尺寸和每个 SessionId 的滚动位置。它们在 `Workbench::new` 建立，渲染时不新建这些实体，不启动文件、进程、网络或数据库操作。只在切换会话时设置对应草稿，普通重绘不重置输入。

## 表面、滚动与焦点

- 原生 TitleBar 保留 Windows 拖动及窗口控制按钮，背景融合到连续炭灰表面。
- 默认逻辑尺寸 1240×800，最小 980×640。侧栏初始值来自 core 的 `WorkspacePreferences`（264），可拖至 240–320；以逻辑像素保存，防止字体缩放挤占对话。
- 侧栏上方项目菜单、中间 Tree、底部设置。只有 Tree 的列表区域滚动，长名称截断，行使用真实图标和原生键盘导航。
- 工作区上方是轻量会话上下文，中间 MessageScroller 独立滚动，底部 Composer 固定。待处理内容区域有高度上限并独立滚动，输入框自动增长到四行；后续完整表单仍在 #14 实现。
- 对话滚动实体按 SessionId 保留，切换视图不改变运行或请求归属。工具折叠后重新测量高度，不主动抢输入焦点。
- 菜单、设置 Dialog、按钮与文本输入使用 GPUI Kit 标准组件。设置 Dialog 使用库的焦点约束、Escape 关闭及关闭后焦点恢复；真实行为以窗口检查为准。

所有颜色值集中于 `theme.rs`。仅深色主题；文字、内容间距与行高用 rem，窗口和侧栏边界用逻辑 px，分隔线保持细线。设置提供 100%、125%、150% 字体/界面尺度；这是应用内缩放，不代替 Windows DPI 验证。

| 快捷键 | 当前行为 |
| --- | --- |
| Ctrl+O | 原生目录选择器 |
| Ctrl+, | 设置 |
| Ctrl+Shift+E | 聚焦项目/会话树；方向键沿用 Tree 行为 |
| Ctrl+L | 有会话时聚焦草稿 |
| Ctrl+Q | 退出 |
| Escape | 关闭当前标准弹层；具体由对应组件处理 |

项目菜单、按钮和快捷键复用相同处理入口。未接入发送、停止、配置更改、审批或问题提交，因而不显示这些功能的可点击假按钮。

## 服务接入边界

`WorkbenchHost` 当前持有 `WorkbenchSnapshot`、当前 `SessionSnapshot` 和 `WorkspacePreferences`。前端意图为 `UiIntent::Command(CommandEnvelope)` 或 `UiIntent::Preferences(WorkspacePreferences)`。草稿命令包含 SessionId、连接 epoch、CommandId、原草稿 revision；非文本草稿内容被保留。

正式启动只发布空 core 快照。原生目录选择结果保存在当前窗口的临时目录上下文中，不生成 Project 记录、不假装已保存到数据库。视图偏好暂时只在当前窗口有效。生产命令入口在服务未接入时明确拒绝，不执行本地 reducer，不收集无人派发的 RuntimeEffects。

后续服务适配集中在 host 层：
1. 在 UI 线程外调用 #5 的 `WorkbenchService::execute / ingest / ingest_batch / session`。
2. 只有持久化成功的 `CommittedUpdate` 才能替换显示快照；业务错误按返回结果展示。
3. 将同一更新的 `CommittedEffect` 交 Runtime 所有者按 sequence、ordinal 派发，UI 不保管 effect 队列。
4. 读取/保存 `WorkspacePreferences` 走服务接口；选会话取得对应快照后再同步保留的输入/滚动实体。
5. 新增会话和内容时更新 Tree 项、对应 MessageScroller 数量；不要重建整个 Workbench 或在 render 中创建状态实体。

本轮未实现异步服务适配器及动态内容订阅；以上是下游接入位置与提交顺序约束，不表示生产服务已连接。无新的 core 字段、crate 或 Cargo 依赖。

### 当前源码接入位置（实窗后复核）

| 边界 | 当前实际入口 | 后台装配时需要补上的部分 |
| --- | --- | --- |
| UI 意图 | `host.rs::UiIntent`、私有 `dispatch`；`save_draft_text` 构造带 revision 的 core 命令 | 用异步服务回调替换生产分支的 NotReady；不能把 fixture reducer 用作生产服务 |
| 已提交数据 | `WorkbenchHost` 中的 core 快照与 `refresh`；后者当前只读 fixture | 发布 `CommittedUpdate.snapshot` 和同批 `sessions`，并保留未受影响会话；异步旧会话查询返回时按 SessionId/当前选择核对，不能覆盖新选择 |
| 视图同步 | `Workbench::tree_items`、`sync_session`、保留的 Tree/Textarea/MessageScroller | 增量更新树和条目数量；流式快照刷新不能每次调用 `set_value` 重置正在编辑的输入 |
| 视图偏好 | host 的选择、展开、宽度、缩放方法 | 汇入 `save_preferences`；本轮仍是窗口内状态，不是已持久化状态 |
| 目录选择 | `open_project` 的异步选择器结果 | 调用服务 `open_project`，待成功后显示真实 Project；不用临时目录制造业务记录 |

已核对 #5 `CommittedUpdate` 包含 snapshot、受影响 sessions、decisions 和带 sequence/ordinal 的 effects；#7 实际入口为 `AgentRuntime::enqueue` / `RuntimeTicket::forward`，并实现 core `RuntimePort`。应用层应按提交顺序同步 enqueue，再在应用级任务中并发 forward；不能串行等待长寿命 Connect ticket 后才派发后续操作。未入队的 ticket 仍须 forward，保留可归属的失败事实。`RuntimeEventSink::emit` 回到服务 `ingest` 后才能发布新的已提交快照。任务生命期不依赖当前选中的会话视图。

连续草稿编辑需要在接入时串行提交或合并尚未提交的文字，再使用最后确认的 revision 发下一次保存；不能让多次异步保存共享旧 revision。这个异步提交策略尚未由开发样例的同步保存证明。当前 app manifest 只有 GPUI Kit 与 core 依赖，存储/Runtime 装配依赖由主控统一加入。

## 开发入口与覆盖边界

```powershell
.\target\debug\agent-workbench.exe
.\target\debug\agent-workbench.exe --dev-fixture
.\target\debug\agent-workbench.exe --dev-fixture=contract
.\target\debug\agent-workbench.exe --dev-fixture=empty
```

也可显式设置环境变量 `AGENT_WORKBENCH_DEV_FIXTURE=rich|contract|empty`。日常正式启动不要设置该变量。样例窗口标题明确显示“开发样例”。

丰富样例复用 core 交错事件样例，再增加长中文项目名、多项目/会话、认证中、失败、待审批、提问、队列和长正文。样例草稿也经 core 命令更新；任何产生 RuntimeEffects 的操作都被拒绝。样例不启动真实 Agent，不写生产存储，也不证明真实认证或运行成功。

当前内容呈现仅用于 #6 布局：文本/Markdown、附件名称、文件引用、工具摘要与折叠文本。完整流式更新、历史顺序、图片内容、计划、文件差异、完整问答/审批表单分别由后续 Issue 实现；不把本轮基础呈现当成这些功能验收。

## 设计依据与检查

复用 `.local/coordination/reports/argon-preparation.md` 已完成的真实来源研究与 API 核对；采用连续双栏、独立对话滚动、固定输入区和少量必要状态。未照搬 Web/CSS API、装饰性渐变、多层大卡片或重复标题/完整路径说明。GPUI Kit 0.9.0 的实际 Tree、Textarea、MessageScroller、Resizable、TitleBar、Dialog API 已通过编译。

基于 Design Guides 和 design-quality 清单，移除旧启动页的重复项目名、完整磁盘路径及开发口号；空状态保留主操作。无独立原型获用户确认。真实窗口证据和未覆盖项记录于 `.local/coordination/reports/ui-6.md`。

主控真实窗口记录在 `.local/coordination/reports/ui-6-live.md`：已观察正式空状态、开发标识、中文多行输入、设置缩放与 Escape、原生目录选择与取消、正常退出。最小窗口、侧栏拖动、改键后的树导航、草稿切换、滚动恢复及真实 IME 仍待复查，未将应用内150%缩放等同于操作系统 DPI 验收。

关闭最后窗口目前退出应用，托盘由后续任务实现。README 旧启动页验证范围需要主控在整合后更新；本轮未修改 README。
