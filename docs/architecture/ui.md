# 原生工作区与设置（Issue #6 / #8）

原生双栏基础已经接入 `workbench-service::ApplicationService`。正式启动从真实存储恢复确认状态，设置操作通过后台目录、安装和认证接口执行。当前代码已通过定向编译；#8 的真实窗口、安装、登录和偏好重开验收仍需单独完成，不能由编译或 #6 样例窗口代替。

## 文件与状态归属

| 文件 | 职责 |
| --- | --- |
| `main.rs` | 应用初始化、应用级所有者、快捷键、窗口与退出入口 |
| `application.rs` | 后台 handle、单一确认快照订阅、待提交视图编辑、偏好/草稿保存、服务关闭 |
| `host.rs` | UI 使用的 core 确认快照与当前会话；隔离的 fixture 来源 |
| `theme.rs` | 深色语义色、字体、间距、圆角、缩放和尺寸范围 |
| `workbench.rs` | 双栏组合、保留的 Tree/Textarea/各会话滚动实体、选择与 Action 路由 |
| `sidebar.rs` | 项目菜单、原生虚拟 Tree、会话状态与底部设置 |
| `conversation.rs` / `composer.rs` | 会话上下文、独立滚动内容、固定输入容器与草稿 |
| `settings.rs` | 原生设置页签、手动配置表单、外观和工作区控件 |
| `settings_agents.rs` | 目录、实际 Agent 状态、安装审阅、官方认证与按需原因 |
| `fixture.rs` | 显式开发入口与共享 core 样例回放 |

GPUI Global 只持有一个 `ApplicationState` Entity，后者持有可克隆 handle 和应用级 `ServiceSubscription` 任务。设置关闭或切换会话不会释放后台服务。数据和业务 reducer 仍由 core/storage 管线维护；UI 不直连 SQLite、SDK 或 Runtime，不拥有 effect 队列。

`WorkbenchHost::accept_confirmed` 只接收 service 的 workbench、active_session 和 preferences。生产路径没有临时目录代替 Project，也不执行本地 reducer。输入焦点、表单编辑、树选中、滚动和待提交文字属于视图状态，不能作为业务成功证据。

## 窗口、滚动和焦点

- Windows TitleBar 保留拖动与原生窗口按钮，使用连续炭灰表面。
- 默认逻辑尺寸 1240×800，最小 980×640。侧栏默认264、范围240–320，以逻辑像素保存；内部文字和间距按 rem 缩放。
- 侧栏上方项目菜单、中间原生 Tree、底部固定设置。长名称截断，Tree 自管虚拟化、滚动与方向键。
- 工作区是轻量上下文、独立 MessageScroller、固定 Composer。待处理区有高度上限，输入自动增长至四行。
- Tree/Input/Scroller 实体在初始化时建立。订阅不会每个 token 重建它们；树只在结构或展开信息变化时更新。输入只在会话切换或可安全同步的确认变化时设置值。
- 滚动实体按 SessionId 保留，内容数量变化使用 splice/append，必要时重新测量。工具展开不主动抢输入焦点。
- 设置使用原生 Dialog、TabBar、Button、Input/Textarea 和文件选择器。设置只持表单/导航状态；标准 Dialog 处理焦点约束、Escape 与关闭恢复。手动配置与安装审阅在同一 Dialog 内返回 Agent 页，不从异步完成回调突然打开新模态框。

所有颜色集中于 `theme.rs`。本版只有炭灰深色主题，提供100/125/150%应用内缩放；应用缩放不等于 Windows DPI 已验收。

| 快捷键 | 当前行为 |
| --- | --- |
| Ctrl+O | 原生目录选择；成功后调用服务登记真实 Project |
| Ctrl+, | 设置 |
| Ctrl+Shift+E | 聚焦项目/会话树 |
| Ctrl+L | 有会话时聚焦草稿 |
| Ctrl+Q | 请求应用后台关闭后退出 |
| Escape | 关闭最上层标准弹层 |

完整项目/会话创建和删除属于 #9；发送、停止、会话配置、审批和问题提交仍由后续任务接入。本轮没有为这些未接通功能设置可点击假按钮。

## 正式服务接线

`ApplicationService::start(ServiceOptions)` 在后台完成恢复、Restart 和初始读取。默认使用系统应用数据目录；启动失败显示实际原因及重试入口，不回退为假空历史。应用只消费 `ServiceSnapshot`，其 `revision` 用于当前服务实例内的渲染顺序；业务 journal sequence、选择 generation 与持久 preferences_revision 各有独立含义。

服务负责先持久提交，再发布对应快照，随后按 sequence/ordinal 同步 enqueue effects，并在应用级并发 forward。UI 不直接调 Runtime，不等待长 Connect 完成才派发后续操作。接口和顺序详见 [应用后台服务](application-service.md)。

| 操作 | UI 使用的真实接口与约束 |
| --- | --- |
| 初始读取/持续更新 | `snapshot / subscribe / current / changed`，保持一个应用级消费者 |
| 目录选择 | `open_project(directory, name)`；只在确认快照更新后显示真实项目 |
| 会话选择 | `select_session`；只读 `query_session` reply 不覆盖订阅的 active_session |
| 偏好保存 | `save_preferences(expected_revision, preferences)`，使用实际持久 CAS 修订号 |
| 草稿保存 | 对指定 SessionId 查询确认草稿，构造原 core SaveDraft 命令并保留 expected_revision、epoch、CommandId |
| Agent 发现/刷新 | `discover_agents / refresh_agents` |
| 手动配置 | `configure_agent(optional_id, name, ManualLaunch)` |
| 安装 | `prepare_install(registry_id)` → 用户审阅 → `install(plan.id)` |
| 认证 | `discover_authentication / authentication(scope, action)` |
| 退出 | `shutdown`，保持订阅与服务所有者到关闭回复 |

偏好编辑按目标字段合并并串行提交，显示选择值来自确认快照；保存中禁用对应设置控件，失败就近展示原因。Tree 展开与侧栏拖动也保存偏好，取消设置 Dialog 不撤销已确认的即时保存。

连续草稿编辑按会话串行保存，后续输入保留在待提交文字中；未确认失败的文字不会被旧快照清掉。错误按 SessionId 呈现在对应会话，不串到另一会话。这里还没有发送入口：#10 必须先处理待保存草稿，再构造 Submit，不能凭方法调用顺序越过尚未完成的草稿查询/保存。

## 设置中的 Agent 流程

Agent 页首次打开执行本机发现；“刷新”才请求官方 Registry。目录离线或缓存状态如实显示，本机已配置 Agent 不因远端列表不可用而消失。已安装、已连接和已认证分别依据实际证据，不以目录条目存在代表任何一种成功。

目录行只显示名称、版本和可用动作。不可安装项用原生“不可安装”按钮按需展开完整原因，默认没有重复的说明副行。展开仅是按 registry_id 保存的视图状态，不更改后台可安装性。实际错误和安装决策信息仍完整保留。

手动配置使用名称、启动程序和逐行参数；文件选择器选择真实程序，保存调用后台校验。没有环境或凭据表单。既有 AgentId 交服务复用，新配置身份由服务分配。安装/配置完成后服务持久化 PutAgent、发布目录并自动执行真实 initialize 发现，UI 不重复握手。

安装审阅页直接使用后台 InstallPlan，展示：
- 官方来源；
- 适配器和 native Codex 的精确版本；
- 安装目标及命令工作目录；
- 实际程序和逐项参数；
- 所有前置条件及其真实满足状态。

只有用户点击“安装”才交回不透明 plan.id。界面不能修改 plan 后执行、静默下载或升级全局工具。计划不再有效时禁止确认；后台安装继续时可返回列表，返回不是取消安装。运行与失败状态来自后台和实际回复；未知清理/禁止重试由后台门槛执行，UI 不自动重试。

## 非持久认证

设置仅读取对应 Agent 的当前 Agent-only AuthenticationView。方法来自 Runtime 真实报告；不可用项展示原因并禁用，包含当前不支持的密钥输入。没有写死认证方法 ID，也不推断“打开浏览器”即认证成功。

动作携带完整 AuthenticationScope、真实方法 ID 或 opaque interaction ID；提交前核对最新 scope、请求和交互是否仍存在，后台再次验证。状态、方法与交互仅随服务在内存中存在，不进入业务快照数据库、journal 或 WorkspacePreferences。认证 URL、秘密字段与 SDK responder 留在 Runtime，UI 只发 OpenOfficialFlow / Decline / Cancel。session 认证使用自己的真实连接 scope，不与设置 Agent-only epoch 混用。

## 启动和验证边界

```powershell
.\target\debug\agent-workbench.exe
.\target\debug\agent-workbench.exe --data-root <绝对隔离数据目录>
.\target\debug\agent-workbench.exe --dev-fixture
.\target\debug\agent-workbench.exe --dev-fixture=contract
.\target\debug\agent-workbench.exe --dev-fixture=empty
```

开发样例也可通过 `AGENT_WORKBENCH_DEV_FIXTURE` 显式开启，标题显示“开发样例”；此时不启动正式服务，不读取生产数据库或安装/认证 Agent。样例设置只改变样例窗口。正式启动不要设置该变量。

#6 主控窗口记录证明了基础表面、中文多行自动输入、应用缩放和目录选择取消。它不证明 #8 接线后的设置流程，也不证明真实 IME 候选。当前 #8 应用定向编译和格式检查已通过；真实设置、安装、认证、偏好重开恢复仍待主控实测。

980×640、侧栏拖动、改键后树导航、草稿切换、滚动恢复、真实 IME/DPI 的未验证项继续保留。完整流式内容、历史顺序、图片、计划/差异、问答/审批并未由本轮基础呈现完成。关闭最后窗口当前请求退出，托盘与完整退出失败交互归后续桌面任务。

源码行为优先于旧阶段描述：README 的“目录只展示、不保存、没有服务接线”部分需主控整合后更新。本轮不修改 README，不把局部实现或编译结果写成完整 MVP/用户验收。
