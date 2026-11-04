# 命令、事件与业务更新契约（Issue #4）

`workbench-core` 是 UI、Runtime、Storage 的共同契约，基于 [对象与生命周期](domain-model.md)。对应 R-17、R-21、R-25、R-29、R-33～R-37、R-39～R-41、R-43～R-45、R-57、R-62；提供 AC-04、AC-05、AC-09、AC-11、AC-12、AC-19 的类型和纯逻辑基础，不代表真实端到端验收。

## 一个业务更新入口

`reduce(&WorkbenchState, &JournalRecord, ApplyMode)` 生成 proposed Transition。状态字段私有，外部只取 typed 快照；reducer 不生成随机 ID、不读时钟、不执行 I/O。相同初始状态与记录序列产生相同结果。UI 不能另存同义 Session、执行或队列状态机。

app 的固定装配顺序：分配稳定身份和下一个宿主 sequence → reducer → `StoragePort::commit` 原子保存日志及 checkpoint → 事务成功后发布 next state → 交付 RuntimeEffects。StorageCommit.expected_sequence 是 compare-and-swap 条件；存储失败保留原状态/输入，不发 effect，也不假装发送成功。不同会话只共用短暂的排序/发布入口，不能跨网络或数据库等待持有全局锁。

Runtime 通过 RuntimeEventSink 持续回传事实，再走同一管线。Applied 只表示业务意图/事实已应用；CommandResult 不表示远端完成。业务拒绝原子回滚所有业务变化，只保存拒绝结果、日志顺序和去重身份。序号缺口为顶层 SequenceConflict，不自动跳号。崩溃可能发生在保存与交付之间或消费与回执保存之间；本契约不承诺跨崩溃无条件 exactly-once，恢复时暂停核对，不自动重发已保存 effect。

同会话的 RuntimeEffects **按已提交 JournalRecord.sequence，再按该 Transition 的 effect 列表顺序入队并开始协议派发**。例如 Submit 必须先开始，随后才能开始 Stop 的取消，不能因异步任务调度倒序。入队/开始顺序不等于等待完成：长 prompt 仍在等待返回时，必须继续派发后续 Cancel、审批和问题响应，不能把它们排在 prompt 结束之后；不同会话也不能共用长等待锁。本条是 app/RuntimePort 的接入约定，本轮没有新增调度框架或后端验证。

## 身份、版本和来源

| 标识 | 含义 |
| --- | --- |
| CommandId / EventId | 一次宿主命令 / 一项稳定本地事实，本地重放不重分配 |
| SubmissionId | 冻结的逻辑输入，补充回退继续沿用 |
| AttemptId / ResponseAttemptId | 一次 prompt/steering 传输 / 一次 responder 响应交付 |
| RequestId | 传输、配置、取消或待答交互的关联，不冒充远端 RPC ID |
| EventSourceId | 实际事件来源，同来源重传保持稳定 |
| ConnectionEpoch | 会话连接代次，重连和重启递增 |
| sequence | 宿主提交顺序，绝不是 Agent 游标 |

CommandEnvelope/EventEnvelope/JournalRecord 均带版本 1，记录边界使用 Versioned<T>。未知或缺失版本拒绝读取。v1 尚未接入实际用户数据库，本轮扩展 #3 新字段使用 serde default，枚举/接口与样例同步；后续变更由契约负责人统一调整版本、转换和所有消费者，不能各端自行猜测兼容。

Scope 可含 Agent、Session、Run、Request、epoch。会话事件必须带 SessionId/epoch；正文/工具/计划/差异/聊天交互另带 RunId。连接、配置、能力可无 RunId。会话创建前认证错误可使用 Agent 范围，认证交互留在 Runtime，不伪造聊天 RunId。事件可带关联 CommandId，主动通知保持 None；归属从实际运行上下文取得，不能从 UI 当前选中对象取得。

EventSource 区分 Host、Runtime、AgentReplay；本地 ApplyMode::Replay 是另外的执行模式。Runtime 标记 adapter/version；RemoteCursor 仅在远端实际提供可靠 stream/value 时填写。没有游标就保持 None，不把宿主序号、时间或正文 hash 当远端身份。

去重按 CommandId/EventId，或来源+会话+epoch+真实 RemoteCursor，不按内容相似度。同 ID 不重复创建消息或发副作用；新宿主序号上的重复身份也只消费日志顺序。旧 epoch、已删除会话和旧 Run 的普通事件记为 Stale。单独关联旧 prompt 的返回只收尾其 attempt/旧 Run，不能结束新 Run。tombstone 随 checkpoint 保留，重用已删除 SessionId/ProjectId 不能复活；尚未实现墓碑/去重索引压缩。

## 命令、事件和错误

Command 覆盖项目/Agent 登记，会话创建/命名/连接/关闭/删除，草稿/附件，队列编辑/取消/暂停/继续，普通发送/立即补充/回退，停止、配置、审批/问题答复、提交核对和重启协调。创建会话不能注入运行状态/外部历史；绑定 Agent 不可原地更换；有会话时不能偷偷更换项目目录。

Event 覆盖连接/资源释放/会话登记、能力和完整配置报告/请求收尾、运行恢复/确认结束、消息/内容块/增量/完成、工具/计划/差异/附件、权限/提问/交付/失效、传输回执/核对结果、错误及未知诊断。先声明文本块再发 delta，消息完成后不再追加。工具终态不被迟到 Running 覆盖；取消不生成工具成功；修改来源只能是明确 AgentReport，不能把共享目录整个 Git diff 归到一个会话。

BusinessError.kind 区分参数/身份/冲突/不存在、Unknown/Unsupported 能力、连接未就绪/Busy、请求过期、交付未知、协议/认证/传输/存储、序号/资源耗尽、远端执行/明确拒绝/资源不可用。RetryAdvice 为 Never、AfterCorrection、AfterReconnect、ReconcileFirst，描述下一步条件而不是自动重试授权。一般失败/超时不证明未消费，已有接受证据不能降级为 NotSent；执行结果未知可以与已接受同时成立。

## 能力与配置

配置命令带 RequestId、epoch、基于的 capability_revision；Runtime 私有映射真实 configId/optionId。新会话默认完整访问只是初始意图，不写死 Codex 私有 ID，不覆盖用户后续选择或组织策略。

- ConfigurationReported 是完整 effective/capabilities 快照，包含模型联动后的真实 effort/选项；revision 由 Runtime 对实际完整快照统一、单调编号，旧值不能覆盖新值。
- ConfigurationChangeSettled 带 RequestId 和 reported_revision，只结束对应请求。无请求 ID 的主动通知不能靠“值相同”清除 pending；Runtime 先规范化完整报告，再报告请求结束。
- 同会话配置有序；pending 期间保存最新未派发意图，允许合并被取代的未发送意图。旧响应不覆盖新 requested，下一请求派发前重新验证，失效 effort 明确拒绝并暂停队列。
- Run.dispatched_configuration 冻结派发时确认值，之后会话配置不改旧 Run。steering 不带新配置；adapter 自启新轮无可靠证据时快照保持 None。
- 队列保存配置意图，实际派发前必须与能力和确认值一致；否则要求先确认配置，不声称配置+prompt 在协议上原子执行。

Unknown 不等于 Unsupported。顶层 steering、promptRequired 子能力和 extension 来源分开；session_resume 与带回放的 history_load 分开；提问的 Agent 能力与 client_questions 处理器声明分开。图片按实际能力/限制验证，未知限额不等于无限。adapter 未能取得逐模型图像能力时不得凭模型名猜测；实际拒绝仍须保存输入并说明。

## 提交、补充和队列

InputSubmission 内容派发后冻结，草稿修改不改它。TransportAttempt 保存 SubmissionId、目标 Run、Request、Command、epoch 和方法。普通 prompt 的 in-flight 门槛必须等其调用真实返回才解除，补充方法返回不能代替它。

| steering 回执 | 处理 |
| --- | --- |
| injected | 原轮已消费，Submission Accepted；不重发，不宣称正在执行的系统命令已瞬间改向 |
| startedNewTurn | 已消费，关联新本地 Run；不编造远端 ID，不再次 prompt；旧 prompt 返回只结束旧 Run，队列暂停等待可靠完成跟踪 |
| promptRequired | 明确 NotConsumed；等旧 prompt 返回、无待答/取消阻碍且队列 Ready，才允许普通 prompt 回退 |
| failed / timeout / 断连 | 消费未知，保存并暂停，不安全重试 |

本版仅在独立 promptRequired 子能力和已核实 extension 来源可用时发 steering，明确请求该 idle behavior。回退保留 SubmissionId，另分配 AttemptId/RequestId/RunId。无远端 run/message ID 时保持 None。接受与执行完成分开，普通 prompt 的真实接受证据不是单纯写入 socket。

ReconcileSubmission 命令/事件显式携带 SubmissionId、原 AttemptId 与原 attempt_epoch，Scope 则使用当前核对连接的 epoch 和新 RequestId。core 保存两组代次的关联，只接纳对应 Runtime 权威报告；回报 run_id 必须是原尝试的运行，不能指向同会话其他旧运行。普通 resume 成功、旧 epoch 的 Attempt 返回、未知结果都不能解除未知提交。连接替换时旧核对请求失效。

Consumed 同步提交 Accepted（保留原派发代次）、原尝试和所属队列条目 Submitted；只有同时给出该运行已结束的证据，才将尝试置 Returned、解除精确匹配原 AttemptId 的 prompt in-flight 门槛并结束该运行。旧队列保持暂停，不发重试/新 prompt effect；用户可在没有旧等待条目时手动发送，或明确 ResumeQueue 后继续下一条。未附结束事实时，尝试保留 ConsumedAwaitingEnd，输入仍 Accepted，执行仍未知；不能放行新输入或继续队列，且可用新 RequestId 再次核对执行结束，即使又经历 Restart/新连接也不丢这条路径。NotConsumed 还须证明原传输已关闭，不能推翻已知消费；Unknown 不改提交、尝试、队列或旧 prompt 门槛。Runtime 无法查询时保留未知，用户使用新会话边界。

can_dispatch_queue 只在前轮结束、无 prompt in-flight/未知提交/待答、队列 Ready、头部 Waiting、配置/输入重新有效时成立。app 分配身份后通过 Submit 再验，不能依据旧快照直接调 SDK。Editing 阻止派发，Dispatching 不能编辑，不同会话不互相等待。

Stop 立即暂停队列、失效请求，再交 responder 取消和 session/cancel；确认取消另靠原 prompt 返回或权威结束事实。仅 EndOfTurn 可按原 Ready 状态继续；限额、refusal、typed failure 必须映射原因并暂停，不能一律显示成功。停止、失败、断连和重启保留未发内容。**空队列下，前轮确认结束后的用户显式新 Submit 可以发送，不要求 ResumeQueue，也不顺便恢复旧队列。** 有旧等待条目时仍按顺序处理，不能暗中越过。

## 请求交付、关闭与恢复

权限/问题按原 RequestId+epoch 找 Runtime 的活 responder，external_id 可空，只作真实关联。回答验证归属/有效期/选项/required/type/选择数量/未知字段后进入一次性 Submitting。ResponseSubmitted 只表示已交给 SDK，没有 Agent 二次 ACK，更不表示工具成功。ResponseRejected 仅限本地拒绝且 responder 仍活；交付失败未知不能重新激活旧按钮。多个请求互不覆盖，失效/停止后的晚到回应不能复活。

表单支持 Text/Choices 子集，保留 property ID 和 secret 标记；Declined 与 Cancelled 分开。数字/布尔/数组或无法验证的约束映射 Unsupported，core 内保留原 schema。未知 required 不可省略后伪报成功，仍可 decline/cancel。secret 值和认证交互走不持久化的 Runtime 官方认证流程，不提交到聊天日志。SDK 回调注册/转交请求后继续派发，不能 await 用户点击阻塞流式和取消。

### 独立的临时认证端口

`AuthenticationPort::execute_authentication(AuthenticationRequest, AuthenticationNoticeSink)` 是 Runtime 的独立可选接口，保持 RuntimePort/StoragePort 的现有签名和消费者兼容。认证请求/通知均**不实现 Serialize/Deserialize**，不加入 Command、Event、JournalRecord、WorkbenchState 或数据库；app 直接消费 typed 临时通知，不转发给持久 RuntimeEventSink。当前仅声明共享类型和端口，具体适配由 Runtime 负责人完成，未增加 #8 UI 或安装管理。

AuthenticationScope 明确 AgentId+连接代次，可关联已有**本地** SessionId，也可只有 Agent 范围；没有 RunId 或外部会话要求，故 session/new 之前的认证/elicitation 不伪造运行归属。Runtime 验证完整 scope、原认证请求和活跃交互，旧连接、已取消/过期流程不能被旧按钮复活。Agent-only 的代次属于该 Agent 认证连接，不与其他会话连接的同值数字混为一谈。

- Discover 只返回 Agent 实际报告的 AuthenticationMethodId、公开名称/说明、种类与实际可用性，不写死 provider 方法 ID。发现方法既不证明需要认证，也不证明已登录。
- Authenticate 选择实际提供的 method_id，启动官方机制。AgentManaged 映射 SDK authenticate(methodId)；Terminal 表示官方终端机制，不能把它送到 authenticate，未实现处理器须标 Unavailable。终端 args/env、原始 _meta 均由 Runtime 保留，不进入共享方法通知。需要宿主尚未实现的 secret 输入时明确 SecretEntryUnsupported，不搭建凭据收集或存储系统。
- 状态独立为 Unknown/Discovering/Required/InProgress/Authenticated/Cancelled/Failed；Authenticated 必须来自真实官方认证成功或已核实的登录复用，不能由“方法存在”“URL 已打开”“elicitation 已交付”推断。失败使用非机密分类，不能把原始错误 JSON/stderr/临时 URL/token 写入失败文案或 PortFuture 的 BusinessError。
- request-scoped `elicitation/create` 的 URL、message、secret schema/值、SDK responder 留在 Runtime。UI 只收到 opaque AuthenticationInteractionId 和 OfficialUrl/UnsupportedSecret 类型；OpenOfficialFlow 指示 Runtime 打开其保留的官方 URL，然后经原 responder 交付 accept，Decline/Cancel 分别交付对应动作。没有 URL/token/密码字段进入 core 请求，UnsupportedSecret 不假装可回答。
- `elicitation/complete`、撤销、取消或断连产生 InteractionClosed 并使句柄失效。InteractionClosed(Completed) 仅表示该交互结束，认证是否成功仍由独立 Status 说明。句柄本身不是远端 RPC ID 或可跨连接恢复的凭据。

本接口以 t2w 的 SDK5/schema1.15.1 窄核查与 initialize-only 实测对齐：正常 Windows 配置报告 api-key/API Key 和 chat-gpt/ChatGPT；NO_BROWSER 会影响实际方法集合，所以 core 不硬编码这两项，也不以隐藏浏览器作为默认认证条件。标准请求仅 methodId，回应为可带 _meta 的空对象；方法的发现、调用和终端种类遵守已固定 SDK 的实际映射。该上游/Runtime 证据不代表本轮已执行新接口的真实认证或用户验收。

CloseSession 设置独立 closing 门槛，旧 Ready 不可解除；期间拒绝提交/配置/继续队列。匹配 ResourcesReleased 后才能显式新建 epoch 连接。Runtime 仅操作 effect 指定 session+epoch 拥有的资源；旧释放事件不能关闭新连接。删除要求资源释放，留 tombstone；不删项目目录、不清其他客户端历史。

SessionSnapshot.resources_released 只读展示同一 core 资源事实（旧投影 serde 默认 false），不由 UI 推导。用户显式重新定位项目时，PutProject 改目录仅在全部关联会话资源已释放后放行；现有 Session/Run/Message/FileChange/FileReference/external 内容不被修改。存储负责新目录 canonicalize/唯一身份检查；之后存在 external 的连接仍须 resume:true，不能在恢复失败时静默 session/new。

若 CloseSession 保存后、ResourcesReleased 前崩溃，Restart 保留 closing 和“未证实释放”，同时把原资源 epoch 交接到 ClosingResourceHandoff；新实例不再等待旧 epoch 回调。app 使用**当前连接代次+新 RequestId**提交 ReconcileClosingResources，effect 同时指明原 abandoned_epoch。Runtime 只能依据确属该会话/原代次的所有权证据核查或清理，不能按进程名/猜测 PID 杀其他实例的进程。ClosingResourcesReconciled(Released) 必须匹配当前核对请求与原代次，才允许删除或显式新建连接；Unconfirmed 保持关闭/删除门槛并允许重新核查。普通旧 ResourcesReleased 仍被过滤，连当前代次的普通 ResourcesReleased 也不能代替该交接证明。再次崩溃保留最初资源代次，使上一轮核对请求失效；清理证据不等于旧任务成功完成。

本地 replay 只重建状态、清空 effects；恢复完成后 app 必须提交 Restart 再开放实时操作。Restart 递增代次、断连、失效请求、旧运行 Interrupted、未确认交付 Unknown、队列暂停；编辑内容和附件仍保留。旧能力/配置值可读但不能当新连接确认。

Agent load 回放是另一种输入，优先协商无回放 resume。当前 AgentReplay 保存诊断、标 History Partial 并保留本地消息，不直接 append、不按正文 hash 去重、不伪造 RunId；尚未实现跨源完整历史快照合并。恢复会话不恢复运行；只有明确能力和 RunRecovered 事实才重新绑定旧执行。

## 窄端口、存储转换和视图偏好

RuntimePort 接收完整 RuntimeEffect，异步向 RuntimeEventSink 发事实。SDK responder/进程/协议 DTO 留在 adapter。PortFuture 不要求 Send，具体 SDK 可用 LocalSet；app 安排后台执行器，UI/render 不执行阻塞工作。

StoragePort 提供 CAS 事务 commit、checkpoint+tail restore、session/before_sequence/limit 历史分页和 opaque blob ID 附件读写/回收。历史按宿主顺序，before 排除自身、limit 为正，适配器约束批量上限。数据库行可拆分，恢复必须验证版本、归属和顺序；不能把未知行直接喂给 UI。collect_attachment 事务内重查草稿/队列/提交/消息/工具等全部引用，只回收应用所有的无引用 blob，不删导入原路径或项目文件。

WorkspacePreferences 独立保存侧栏逻辑宽度、缩放百分比、选中项目/会话和展开对象，经 StoragePort 读写，不进入会话业务状态机。UI 按窗口裁剪尺寸、过滤已删除选择。AgentDefinition.launch 保存 executable + 非敏感 args；凭据不进参数/业务数据库，安装发现也不等于登录完成。

typed WorkbenchSnapshot/SessionSnapshot 不含任意 JSON、未知 schema 或 SDK handle；未知内容只给类型/诊断关联，原数据留受控记录。工具参数/结果仅作为 readonly 文本，不能当可执行结构。光标/焦点/折叠/滚动属于 UI，选中对象不决定事件归属。

can_steer 表示“当前草稿可立即补充”，不是单纯支持 steering 的能力标记。它与 Submit 调用同一个无副作用校验：当前运行/连接、未决补充或已消费但未结束的核对、配置 pending/确认值、实际输入/附件、独立子能力和 extension 来源都一致验证；UI 无须重建这些业务条件。命令仍重验提交时的冻结内容和身份，防止读快照后状态变化。

## 样例与协议依据

`workbench_core::examples::interleaved_example()` 返回固定 ID/顺序、虚构中文路径的 ContractExample，供各端共用：两会话交错、图片/文件引用、失败工具、两个审批和一个问题、附件队列。`tests/contracts.rs` 复用它检查回放/重复、steering 四态/结束竞态、停止与迟到回答、配置收尾、未知表单、重启编辑保留、closing/旧释放/tombstone 和 AgentReplay 降级。

本契约沿用 Astra 对固定源码的研究：[Codex ACP 2.5.0 steering 类型](https://github.com/agentclientprotocol/codex-acp/blob/519c2671bfe8266b7625b1cebde7657b5f6d1303/src/AcpExtensions.ts)、[配置/恢复/补充/取消实现](https://github.com/agentclientprotocol/codex-acp/blob/519c2671bfe8266b7625b1cebde7657b5f6d1303/src/CodexAcpServer.ts)、[SDK 5.0.0 Responder](https://github.com/agentclientprotocol/rust-sdk/blob/050cd94c081056ace37a7b89abfed7f6220b47eb/src/agent-client-protocol/src/jsonrpc.rs)、[恢复 API](https://github.com/agentclientprotocol/rust-sdk/blob/050cd94c081056ace37a7b89abfed7f6220b47eb/src/agent-client-protocol/src/session.rs)、[schema 1.15.1 elicitation](https://docs.rs/crate/agent-client-protocol-schema/1.15.1/source/src/v1/elicitation.rs)。尚未固定 ACP 依赖，#7 仍须核对最终 API，不能套用旧 Client trait。

本轮未连接真实 Agent、数据库或新 UI；纯样例不证明 Windows 登录/分发、实际图像能力、IME/DPI、并发恢复或用户验收。
