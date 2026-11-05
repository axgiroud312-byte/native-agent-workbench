# 统一数据结构与运行契约

版本：Spec v0.1，2026-11-05。依据：用户已认可的 [HTML 原型 v0.2](../design/design-board.html) 与 [交互约定](../design/interaction-notes.md)。本文定义正式 Rust 实现的共同语言；原型中的 JavaScript 对象不是生产数据结构。

## 1. 契约地位与实现顺序

先建立本文件中的 ID、实体、命令、事件和状态转换，再开发前端与服务适配器。双方通过同一 Rust 契约 crate 编译校验，不分别发明字段。

- **用户要求**：多个会话可同时工作，共用项目目录；切换页面继续运行；Codex 优先，经 ACP 接入；模型、强度和权限按 Agent 能力展示；文件、浏览器与终端拥有独立标签状态。
- **工程决策**：业务模型不依赖 GPUI、SQLite、Tokio 或 ACP 的具体类型；副作用由服务执行；界面只消费已标准化的快照和事件。
- 下列命令和事件名称均为本应用的接口设计，不声称 ACP SDK 存在同名方法。具体 SDK 版本、签名和能力在接入任务中查证。
- 跨 crate 的行为状态使用私有字段、构造器和读取方法；公开的序列化记录使用可演进版本和 `#[non_exhaustive]`，避免依赖穷举结构体字面量。

## 2. 责任与数据流

| 层 | 唯一责任 | 可持有的状态 |
| --- | --- | --- |
| 业务 Core（domain + app-service） | 校验命令，确定会话/任务状态，生成服务操作，发布一致快照 | 项目、会话、消息、队列、审批、期望配置及确认配置 |
| Agent Runtime | 进程、ACP 会话、协议能力、请求关联、事件转换 | 连接句柄、外部 Session ID、请求表、认证流程、进程资源 |
| Storage | 事务、迁移、历史和草稿持久化 | SQLite 连接、存储版本、附件文件、提交结果 |
| 文件/浏览器/终端服务 | 对应资源的实际 I/O 和生命周期 | 文档版本、WebView 句柄、PTY/子进程句柄 |
| GPUI 功能视图 | 渲染、焦点、滚动、弹层、动画、输入组件 | `Entity`、`FocusHandle`、组件 State、瞬时选择和布局 |
| App Shell | 创建窗口、组合功能、托盘和退出协调 | 根视图与服务句柄，不包含会话业务分支 |

流程固定为 `UI Action → Core Command → Core 校验/存储 → Service Operation → Event → Core Snapshot → UI`。

其中 domain 保存纯数据类型与确定性转换，app-service 拥有运行中的状态与服务协调；这里只定义逻辑边界，具体能力可先作为模块，不要求每个模块都建立独立 crate。

命令和事件通过有界通道跨执行器传输；Tokio 负责 I/O，GPUI entity 的变更回到 GPUI 上下文。渲染阶段只读状态，不启动进程、解析整段历史或写数据库。

每个业务能力有一个状态拥有者。输入、列表、编辑器、网页和终端的 retained state 在其生命周期内创建一次；关闭设置页、收起面板和切换标签不会重建底层资源。

## 3. ID、版本与外部身份

所有 `*Id` 使用强类型包装，内部为稳定 UUID；本地 ID 与外部 Agent ID 分离。UI 元素使用 `(实体 ID, 控件语义)` 作为稳定标识，不使用行号或标题。

| 标识 | 含义与约束 |
| --- | --- |
| `ProjectId` | 一个已登记的项目；目录更名不自动产生新 ID |
| `SessionId` / `RunId` | 本地会话 / 一次执行；一个会话同一时刻至多一个主 Run |
| `SubmissionId` | 一次用户提交意图；传输重试保持同一 ID，用户再次发送使用新 ID |
| `MessageId` / `BlockId` | 消息 / 可增量更新的内容块；插入历史不改变现有 ID |
| `ToolCallId` / `PermissionId` / `QuestionRequestId` | 本地工具调用 / 审批 / 提问；外部 ID 另存并在连接范围内映射 |
| `DocumentId` / `BrowserPageId` / `TerminalId` / `TabId` | 编辑文档 / 网页实例 / 终端实例 / 一个视图入口 |
| `ConnectionEpoch` | Runtime 每次重新建立连接时递增；旧连接结果不得污染新连接 |
| `RunEpoch` / `ConfigRevision` / `DraftRevision` | Run、配置和草稿的单调版本；异步结果必须比较预期版本 |

外部关联键为 `(SessionId, ConnectionEpoch, external_session_id)`；进程 PID 不作为稳定身份。每个项目记录绝对目录与平台文件身份；Windows 路径比较遵循实际文件系统，禁止只靠字符串 lowercase 判断文件同一性。

## 4. 领域实体

| 实体 | 必须表达的字段 |
| --- | --- |
| `Project` | id、显示名、根目录、规范化文件身份、创建/更新时间、可访问状态 |
| `AgentDefinition` | id、名称、启动命令与参数数组、安装来源/版本、平台条件、能力快照；敏感环境值仅持有凭据引用 |
| `SessionRecord` | id、project_id、agent_id、标题、创建时间、外部会话引用、连接状态、当前 Run、恢复能力、配置修订 |
| `RunRecord` | id、session_id、epoch、起始提交、配置快照、状态、开始/结束时间、停止原因、资源释放状态 |
| `Message` | id、session_id、可选 run_id、角色、有序 blocks、内容版本、产生时间、交付状态、来源信息 |
| `ContentBlock` | id、类别、内容版本、来源；类别含 Markdown/Text/Code/Image/FileReference/Tool/Plan/Permission/Questionnaire |
| `ToolCall` | id、外部调用 ID、session/run、kind、标题、状态、输入/输出、文件位置、可选终端引用 |
| `PermissionRequest` | id、session/run/epoch、外部请求 ID、工具引用、Agent 提供的选项、决策状态、选择值、过期原因 |
| `QuestionRequest` | id、session/run/epoch、外部请求 ID、schema_revision、字段列表、是否阻塞运行、回答状态、过期原因 |
| `QuestionAnswer` | request_id、answer_revision、字段 ID 到合法值的映射、submission_id、本地受理与远端确认状态 |
| `Draft` | session_id、revision、文本、附件引用、文件引用、选择范围、修改时间 |
| `QueuedPrompt` | submission_id、session_id、位置、revision、不可变内容/配置快照、队列状态、暂停原因 |
| `Attachment` | id、名称、MIME、字节数、内容哈希、本地受管文件引用、处理状态、错误；原图与转换图分别追踪 |
| `UsageRecord` | session/run、Agent 报告值、计量单位、来源时间、是否缺失；缓存/总量口径不混算 |

时间戳用于展示与诊断，事件顺序由序号决定。持续时间由运行期单调时钟计算；跨重启的历史持续时间使用保存的完成记录。

消息内容与执行事实分别建模：Agent Markdown 中的“已通过”可以显示为文本，只有工具完成事件、退出码或验证服务结果可以生成应用的“检查通过”状态卡。代码块解析不会执行代码；按钮能力来自固定应用动作，不能由普通文本创造权限批准或任意系统动作。

## 5. 能力与配置

`AgentCapabilities` 至少区分图片输入、恢复已有会话、运行中补充、模型选择、思考强度、权限模式、取消、命令发现、计划/工具/用量报告。能力取 `Unknown / Supported / Unsupported`，并记录来源与连接版本。

`SessionConfigOption` 保存 Agent 的 option_id、category、label、值类型、合法选项、当前值和能力版本。模型名、权限选项值不得硬编码成某个厂商的中文枚举。

`SessionConfiguration` 分三份：

- `desired`：用户下一次希望使用的配置，含 `desired_revision`。
- `effective`：当前 Agent 已确认的配置，含 `confirmed_revision` 与确认来源。
- `pending`：正在应用的请求，含 `operation_id`、目标 revision 和 `Applying / Failed`。

空闲时改变选项，Core 校验后发出配置操作；只有有效确认才更新 effective。返回整个选项集合时作为一致快照替换，避免旧模型的强度选项遗留。

运行时改变模型/强度/权限默认仅修改 desired；新排队消息在提交时冻结该配置。已排队条目不被后续选择器操作静默改写。“立即补充”使用 active Run 的 effective 配置，必要差异通过短提示表示。

新会话默认完整访问：在 Agent 提供的合法权限模式中请求相应值；恢复已有会话保留用户存储的权限选择，不自动提升；若不能映射或被 Agent 拒绝，显示实际生效值及原因，不把 UI 标签当成授权结果。工作台自行打开的终端与文件编辑不冒用 ACP 权限模式。

Codex 适配器可能恢复已有权限配置，或者使用 `agent`（Auto review）默认模式，因此应用必须主动选择并核对生效权限；不能把适配器默认值等同于产品的“完整访问”。[Codex ACP 权限配置](https://raw.githubusercontent.com/agentclientprotocol/codex-acp/main/docs/permission-profiles.md)

ACP 官方当前建议优先使用 Session Config Options；模式、模型与强度仍按实际返回值处理。实现层可以兼容较旧能力，但不得让 UI 读取厂商私有 JSON。[ACP 配置文档](https://agentclientprotocol.com/protocol/v1/session-config-options)

## 6. 命令、事件与操作确认

所有 Core 命令带 `CommandId`、目标 ID、期望 revision；错误为可显示的类型化结果。按钮、快捷键和菜单派发同一命令。

| 命令组 | 应用命令 |
| --- | --- |
| 项目/会话 | AddProject、RenameProject、RemoveProject、CreateSession、RenameSession、DeleteLocalSession、ConnectSession、ResumeSession |
| 输入 | UpdateDraft、AttachImage、RemoveAttachment、SubmitPrompt、EnqueuePrompt、EditQueuedPrompt、RemoveQueuedPrompt、SteerRun |
| 执行 | CancelRun、ResumeQueue、ResolvePermission、SubmitQuestionAnswer、CancelQuestionAnswer、SetDesiredConfiguration |
| 内容 | OpenDocument、EditDocument、SaveDocument、ResolveDocumentConflict、OpenBrowserPage、NavigatePage、StartTerminal、SendTerminalInput、StopTerminal |
| 布局 | OpenContentTab、ActivateContentTab、CloseContentTab、SetPanelLayout、OpenSettings、ReturnToWorkbench |

事件信封：`EventId + SessionId? + ConnectionEpoch? + RunId? + RunEpoch? + Sequence + OperationId? + Payload`。本地 UI 事件也要有对应资源 ID 与 revision；没有 Session 的浏览器/终端不伪造会话归属。

主要事件包括 ConnectionChanged、CapabilitiesReplaced、ConfigConfirmed、SubmissionRecorded、SubmissionRejected、DeliveryChanged、MessageDelta、MessageCompleted、ToolChanged、PlanChanged、PermissionRaised、PermissionResolved、QuestionRaised、QuestionAnswerRecorded、QuestionResolved、QuestionExpired、RunEnded、ResourcesReleased、DocumentChanged、DocumentSaved、TerminalOutput 和 TerminalExited。

应用先校验身份/版本、去重和顺序，再提交状态。文本块只追加到指定 BlockId；工具 patch 合并指定 ToolCallId，省略字段保持原值。Unknown update 记录可诊断信息，不能默认转换为成功。

## 7. 提交、队列与立即补充

交付状态为 `LocalRecorded → Dispatching → InFlight → Completed`，分支为 `Rejected / DeliveryUnknown`。`InFlight` 只表示请求正在等待结果，不等于 Agent 已确认读取全部输入。

1. UI 发送 `(DraftRevision, SubmissionId, 内容快照)`；Core 将消息/队列与附件引用事务性记入本地。
2. Core 的 `SubmissionRecorded` 表示本地已接管该快照；UI 可清除**同一 revision** 的输入，提交仍显示“发送中”或“排队”。用户随后输入的新内容不被清空。
3. 对于本地记录失败，原草稿不变；对于明确未投递/拒绝，保存失败条目并提供恢复到输入框。恢复时如已有新草稿，以独立条目保留，不能覆盖。
4. 适配器只在可证明的协议结果、明确输入回显或支持的确认机制下发布远端接受证据；写入 stdin 成功和 TCP 成功不是该证据。
5. 传输结果未知时停在 DeliveryUnknown，暂停该会话队列，提供核对/手动重发；不能把原输入自动作为新请求重投。

队列状态为 `Queued / Editing / Dispatching / Held / Rejected`。队列入库成功后才显示已排队；编辑使用预期 revision 原子替换，派发已开始则拒绝覆盖并保留编辑稿。

每个会话队列 FIFO；允许不同会话各自推进。正常 Run 完成后才取下一条；失败、断连、用户停止或退出使队列 Held。恢复队列为显式动作，先确认连接、配置和附件可用性。

派发条目时重新校验冻结配置仍被 Agent 支持。配置应用失败留在 Held，不以当前默认值偷偷发送；模型选择变化不影响之前条目。

SteerRun 是应用能力，不假定所有 ACP Agent 都有标准实现。适配器须声明支持范围与接收证据；不支持时禁用入口。不得用“取消当前任务再发送”或“无确认的第二次 prompt”伪装立即补充；未知接收结果不自动转入队列。

当前 Codex ACP 已文档化 `_session/steering` 扩展。Runtime 在初始化响应确认 `_meta.steering.supported`，同时验证适配器版本支持 `idleBehavior: "promptRequired"`，再启用本应用立即补充入口；这个可选行为不是单靠 supported 标志就能证明。[Codex ACP Steering](https://raw.githubusercontent.com/agentclientprotocol/codex-acp/main/docs/steering.md)

本应用的 Codex 请求选择 `promptRequired` 行为，由 app-service 继续持有正常 Run 的启动权。Runtime 将具体协议结果转为以下应用结果：

| 协议结果 | 应用处理 |
| --- | --- |
| `injected` | 记录该 SubmissionId 已加入目标 Run，结束此次补充提交；后续输出、用量和结束仍归原 Run |
| `promptRequired` + `noRunningTurn` | 是明确未消费证据；输入保留为待处理条目，用户可选择排队/作为新任务发送。不能静默改变“立即补充”的语义 |
| `startedNewTurn` | 不把它当普通补充成功。说明本次进入了适配器自行启动分支；先关联新执行与原输入并停止重复派发，显示真实状态，记录兼容性问题并停用未验证的补充路径 |
| `failed` / JSON-RPC 错误 | 保留内容和错误。仅凭失败文本不推断“从未消费”；只有明确的拒绝/未消费证据才允许安全重试 |
| 断线、超时、无法解析 | DeliveryUnknown；保留当前 Run/输入关联并暂停自动后续动作，禁止双发 |

`promptRequired` 可能发生在旧 prompt 仍等待计划审批时。它不代替审批决定，也不等于旧 Run 已完成；新正常 prompt 必须等原执行真正结束与资源条件满足后才可启动。

ACP v1 的 `session/prompt` 响应代表本轮结束，不能要求用户等该响应才解除输入框锁定；输入框以本地可靠接管 receipt 转移内容，同时将远端交付状态单独呈现。

## 8. 会话、Run 与并发

连接状态：`Created → Starting → Authenticating? → Ready → Disconnected / Failed → Starting`；显式结束后 `Closed`。是否存在待审批与是否运行是独立维度，不用一个平面枚举混合所有状态。

Run 状态：`Preparing → Running ↔ AwaitingPermission → Cancelling → Cancelled`；Running 可进入 Completed、Failed、Interrupted。连接消失时未结束的 Run 记为 Interrupted，不能显示已完成。

侧栏状态为推导值：待审批优先显示，其次运行/取消中，再次失败/断连，最后闲置。selected、hover、focus 完全属于 UI 交互维度。

并发保证：至少两个会话可同时执行。工程默认调度上限为 2 个活跃 Run，超出项按资源策略排队；该数字是可调整资源默认值，不是业务会话数量限制。

每个 Session 只有一个负责串行处理命令/事件的 owner；会话之间并行。Runtime 初期优先独立进程/连接隔离，后续复用进程须保持外部会话映射和独立请求生命周期。

每次连接更替增加 ConnectionEpoch；每次执行增加 RunEpoch。过期结果只能用于诊断，不修改现有消息、模型、审批、焦点或状态。关闭面板与切换设置不触发 CancelRun。

会话共用目录不代表共享内存状态，也不自动隔离文件写入。应用发起的文件保存对同一路径串行；Agent 自己写盘仍依赖文件版本检查和冲突提示，不能宣称已防止所有互相覆盖。

## 9. 取消、权限与重连

取消请求以 `(SessionId, RunId, Epoch)` 定位。UI 进入 Cancelling 并暂停队列；收到结束结果才能进入 Cancelled/Completed/Failed。取消与正常完成竞争时保留真实结果，不能把已完成任务改写成失败。

`RunEnded` 与 `ResourcesReleased` 分开。释放所有本 Run 所有的进程/请求/工具资源后才允许复用占用槽位；会话级 Agent 连接和进程可继续存在，不能等待它们退出才结束每轮；超时升级为显式终止操作并显示原因，不假装资源已回收。

审批状态：`AwaitingDecision → SendingDecision → Allowed / Denied / Cancelled / Expired / DecisionFailed`。决策使用 Agent 提供的 option_id，不由中文按钮推导权限范围；选项过期、Run 结束、epoch 变化后不可提交。

拒绝一次工具请求只记录该决定；Agent 可能继续回答或采取替代步骤，因此拒绝不自动终结整个 Run。重复点击同一决定只发送一次。

发送会话取消时，Runtime 同时按协议结束未完成的权限请求；收到取消后仍可能有结尾更新，需按当前 Run 有序收尾。[ACP 取消规则](https://agentclientprotocol.com/protocol/v1/cancellation)

受支持的 Agent 提问单独使用 `QuestionRequest`，不与权限审批共用授权状态。字段保存稳定 field_id、类型（单选/多选/文本）、选项 id/label、required、合法值/数量/长度限制及默认值；未知字段保留可读回退并标识不可提交，不能生成任意执行控件。

回答状态为 `AwaitingAnswer → Recording → Sending → Answered`，分支为 `ValidationFailed / DeliveryUnknown / Cancelled / Expired`。用户编辑以 answer_revision 标识；提交同时校验 schema_revision、Run/connection epoch 和所有字段，可靠保存后才锁定该份回答。重复提交同一 SubmissionId 只派发一次；失败恢复回答而不覆盖新编辑，未知送达不自动重投。

`QuestionAnswerRecorded` 只是本地受理；`QuestionResolved` 必须带远端确认结果。能否取消回答、是否阻塞 Run 由实际协议能力决定；取消一个表单不等于取消整个 Run。Run 结束、连接更换或 Agent 撤回请求后标为 Expired，迟到结果只保留诊断，不修改新提问。

重连先增加 epoch，再重新初始化、认证、查询能力。只有 Agent 明确支持且本地有外部 Session ID，才尝试运行态恢复；否则保留历史并允许创建续接会话。数据库里的 Running 不意味着原进程重启后仍存活。

恢复时历史和实时事件有重放边界：以外部稳定 ID 去重；无稳定 ID 的历史重放暂存为一批，明确完成后接入实时流，不能按文本相同误删真实重复消息。

## 10. 文件、网页、终端和标签

`WorkspaceState` 以 ProjectId 分区，包含 opened_tabs、active_tab、selected_session、active_auxiliary、last_aux_width、zoom_restore、sidebar_width/collapsed。切换项目恢复该项目状态，不把原型全局 files 对象带到生产。

`ContentTarget = Session(SessionId) | Document(DocumentId) | Browser(BrowserPageId) | Terminal(TerminalId)`；TabId 指向一个 target，不复制其内容。浏览器内部标签与顶部标签引用同一个 BrowserPageId。

会话标签关闭只移除入口，保留会话和任务。辅助面板收起只隐藏资源。文件保存、网页关闭和终端停止由各自 owner 执行，不能用通用 `close` 一次释放所有对象。

`DocumentRecord`：project_id、文件身份、路径、encoding、line_ending、saved_revision/content_hash、buffer_revision、磁盘版本、dirty、冲突状态。GPUI EditorState 与光标/滚动不进入纯业务模型。

保存带 expected_disk_version 与 buffer_revision；后台写入完成后仅确认该 revision。保存期间继续输入时保留后续 dirty。默认使用同目录临时文件替换方案，并处理平台权限/锁定失败。

外部改动：clean 文档重新载入并保持可恢复视点；dirty 文档进入 Conflict，保留内存编辑和磁盘内容，提供查看差异、重新载入、明确覆盖或合并后保存。自动保存不能绕过冲突；目录相同的两个项目也共享同一文件写入协调器。

`BrowserPageRecord`：project_id、id、标题、当前 URL、可恢复导航记录、加载状态、错误、可选会话配置引用。真实 DOM、JS 内存和登录状态由 WebView engine/profile 管理，不能通过 SQLite 字段宣称完整还原。

网页状态：`Created → Loading → Ready / Failed`；导航带 NavigationId，旧页面回调不得覆盖新地址。点击停止只取消当前导航，保留最近可见页面。关闭页释放对应 WebView；放大/还原复用实例。

`TerminalRecord`：project_id、id、shell、cwd、launch 参数、运行状态、退出码、可恢复滚动输出引用、终端尺寸。实际进程与 PTY handle 仅在服务层；终端解析器状态不和 Agent 对话文本混存。

终端状态：`Starting → Running → Exited / Failed`；Stop 为 `Stopping → Exited`。按键、粘贴和 resize 发送给对应 TerminalId 的 PTY；输出带递增字节序号，不按任意网络 chunk 当完整 UTF-8/ANSI 字符。

终端标签关闭默认隐藏入口并保留运行，重新打开同一实例可继续查看；明确“停止终端”才结束进程。应用显式退出时协调终端和 Agent 的停止，关闭窗口收进托盘时继续运行。

应用自身终端与 ACP 工具终端分别记录 owner。Agent 请求的终端资源按协议生命周期释放，不能因用户保留了一个终端标签而泄漏已应结束的工具资源。

## 11. 持久化与恢复

SQLite 由单一写入服务协调短事务；通过连接配置启用外键、busy timeout 和合适的日志模式。数据库不保存 GPUI/进程/WebView 句柄；凭据放系统凭据存储，数据库只存引用。

| 表/记录 | 核心关系及索引 |
| --- | --- |
| schema_migrations | version 主键、checksum、applied_at；每项迁移一次 |
| projects / agent_definitions | ProjectId/AgentId 主键，规范路径索引；配置带版本 |
| sessions / runs | session.project_id、run.session_id；按 project+updated_at、session+started_at 索引 |
| messages / message_blocks | message.session_id、block.message_id；session+sequence、message+block_order 唯一 |
| question_requests / question_answers | request_id、schema_revision、answer_revision、submission_id；session/run/epoch 关联；回答提交去重 |
| tool_calls / permission_requests | session/run 外键；外部 ID+epoch 唯一范围；权限状态可查询 |
| drafts / attachments / message_attachments | session draft 唯一；附件哈希/路径；引用关系用于回收 |
| queued_prompts / submissions | SubmissionId 唯一；session+queue_position；内容/配置 revision 和交付状态 |
| documents / document_drafts | 文件身份+project 关系；恢复草稿与磁盘基线分开 |
| workspace_layouts / browser_pages / terminals | ProjectId 分区、对象稳定 ID、版本化恢复数据 |
| settings / event_journal | 设置版本；有界领域事件日志与去重位置，非无限 raw stdout |

创建新表不能替代升级已有 schema。迁移显式检视旧版本并在事务中执行，验证数据保留；迁移失败保持旧数据可诊断，不删除数据库后重建。对未知较新 schema 提示版本不兼容。

本地提交、消息/队列入库及附件引用变更必须原子提交。文本增量可合并后分批保存，Run 结束/退出前做检查点；界面显示的已接管用户输入不能仅存在易失内存。

启动恢复：加载项目/索引/草稿/布局 → 历史分页 → 将未确认运行标为 Interrupted 或待核对 → 队列 Held → 用户需要时连接 Agent。恢复网页 URL 不自动恢复上一页 JS 堆；恢复终端输出不冒充仍在运行的 shell。

历史仅索引本工作台创建的会话。删除项目登记不删除本地项目目录；删除会话须协调活跃任务与本地引用，附件只回收已无引用的受管文件，原始用户文件不在回收范围。

## 12. 关键验收用例

| 用例 | 必须观察的结果 |
| --- | --- |
| 两个会话同时流式输出，再切换项目 | 消息、工具、草稿、审批各归所属会话；非当前任务继续 |
| 连接 A 断开后连接 B 收到 A 的迟到事件 | B 的消息、配置、状态完全不变；日志能关联丢弃原因 |
| Run 完成与取消结果交错 | 只有一个有效终态；队列不意外继续；资源槽位不提前复用 |
| 修改模型后排队，再再次修改模型 | 每条队列使用自己的配置快照；当前任务配置不变 |
| 补充结果未知、连接超时 | 不自动补发、不偷偷转队列；内容保留并显示可核对状态 |
| 队列编辑与取出竞争 | 派发旧快照或确认后的新快照二选一；编辑稿不消失 |
| 审批后切换会话、重连后再次点旧审批 | 只处理原请求一次；旧 epoch 决定被拒绝 |
| 保存文件时继续输入、同时发生外部修改 | 已保存 revision 正确；后续编辑仍 dirty；冲突不静默覆盖 |
| 打开同一网页的两处标签入口 | 切换指向同一个网页实例；关闭同步，其他网页历史保持 |
| 单选/多选/文本回答，提交后请求过期或重连 | 字段校验、同一回答只送一次；旧回答不进入新请求；结果未知不重投 |
| 终端运行时收起面板、进入设置再返回 | 进程和输出连续；显式停止后退出码和释放状态可见 |
| 数据库升级与崩溃恢复 | 项目、历史、草稿和队列仍可读取；不宣称未知任务已完成 |

纯状态机测试覆盖这些可确定竞争条件；真实 ACP、PTY、WebView、IME 和窗口行为由对应集成与人工验收证明。编译通过、HTML 模拟或 fixture 不能代替真实能力验证。

## 13. 协议依据与接入验证点

2026-11-05 查阅的 ACP 官方站点将 v1 标为 Latest、v2 标为 Draft。生产实现锁定具体 SDK/协议组合，适配器封装版本差异；本文不承诺草案能力已被所选 Codex 适配器实现。

- [ACP Session Setup](https://agentclientprotocol.com/protocol/v1/session-setup)：初始化、认证前置条件及外部会话身份。
- [ACP Prompt Turn](https://agentclientprotocol.com/protocol/v1/prompt-turn)：流式更新和一次执行的收尾。
- [ACP Tool Calls](https://agentclientprotocol.com/protocol/v1/tool-calls)：结构化工具调用、状态和增量更新。
- [ACP Session Config Options](https://agentclientprotocol.com/protocol/v1/session-config-options)：配置选项及当前值。
- [ACP Cancellation](https://agentclientprotocol.com/protocol/v1/cancellation)：取消结果与待审批收尾。
- [Codex ACP Steering](https://raw.githubusercontent.com/agentclientprotocol/codex-acp/main/docs/steering.md)：扩展能力、promptRequired 与明确接收结果；不同版本需实际验证。
- [Codex ACP Permission Profiles](https://raw.githubusercontent.com/agentclientprotocol/codex-acp/main/docs/permission-profiles.md)：权限选项与初始/恢复模式。

必须在真实接入阶段验证：Codex ACP 适配器的立即补充接收语义、配置确认/模型强度映射、权限模式、图片输入、load/resume、取消后子资源释放。验证失败时使用明确能力状态与恢复入口，不能靠 UI 演示掩盖。
