# 应用后台服务

`workbench-service::ApplicationService` 是原生 app 的唯一业务 I/O 入口。app 持有可克隆 handle 和一个应用级订阅；设置页、会话视图不持有数据库或 Runtime。业务状态与 reducer 仍由 `workbench-core` 定义，在 `workbench-storage::WorkbenchService` 的事务管线中维护。服务内只有已确认的 `ServiceSnapshot` 读投影和临时任务地址，没有第二份可变 WorkbenchState。

## 启动与订阅

`ApplicationService::start(ServiceOptions::new())` 使用 storage 的正式系统数据目录；`with_data_root(PathBuf)` 指定隔离位置。后台线程使用 Tokio current-thread executor + LocalSet，容纳非 Send 的 PortFuture。对 UI 返回的 ServiceFuture 是 Send，仅等待跨线程回复；文件、SQLite、进程与网络不在 UI 线程执行。

启动完成前，storage 恢复历史并持久化一个新 Restart，再读取快照、偏好和已选会话。历史恢复不声称远程运行已恢复，不自动连接、重发或恢复队列。

`snapshot()` 返回 `Arc<ServiceSnapshot>`；`subscribe()` 返回应用级 ServiceSubscription，`current()` 读取最新值，`changed().await` 等待后续值。订阅采用 watch：中间渲染通知可以合并，业务事件不会因此丢失。`revision` 是本服务实例内的视图修订号，`workbench.sequence` 是持久 journal 顺序，不能互换。

## 有序命令与事实

handle 的方法在**调用当下**同步 try_send，future 第一次 poll 不负责入队。先创建 SaveDraft future 再创建 Submit future，即使先 poll 后者，入队顺序仍保持。UI 队列上限 128；满时明确返回 Busy，意图未接受，UI 保留输入，不能假装已提交。

后台串行处理命令和 Runtime 事实：

1. storage 提交事务并返回 CommittedUpdate。
2. 发布同次提交的 typed WorkbenchSnapshot 和受影响 SessionSnapshot。
3. 按 sequence、ordinal **同步**调用 runtime.enqueue。
4. 在应用级并发 forward 每张 RuntimeTicket，不等待长 Connect/prompt 结束才处理 Stop、回答或其他会话。

Runtime 事实通道上限 512，背压等待，不按 token 丢弃。每批至多 32 个已排队事实，保留原顺序、身份和对象先于引用。storage.ingest_batch 提交后才发布与交付 effects。不做文本 hash 去重；远程 cursor、EventId、旧 run/epoch 过滤仍由 core 处理。

事件事务失败时停止正常接收，保留失败批次并关闭 Runtime，向调用方报告未完成持久化和需核对的状态；不自动重发远程输入。偏好读取是附带查询，其失败不能吞掉已经提交的业务 effects。

`execute(CommandEnvelope)` 贯穿原 CommandId、Scope、会话、run、request、epoch。返回 CommandReceipt（确认的 journal sequence、视图 revision、真实持久 CommandStatus）。重复命令也读取原结果，因此被拒命令不会因再次提交而变成 Applied。

新鲜命令直接使用同次事务 CommittedDecision 的 Applied/Rejected；只有 Duplicate 才读取原持久结果。后续可选数据库查询不能把已确定的业务提交误报成未提交。准入检查失败而命令身份实际已提交时，仍返回原结果，不重新交付 effect。

## 选择与偏好

`open_project(directory,name)` 复用 storage 的规范路径身份并选中项目。`select_session(Option<SessionId>)` 保存选择、读取该会话，再发布单次确认视图。查询在同一后台串行流内完成，晚到查询不能更改后来的选择。`query_session(id)` 只返回 SessionReply，带 session_id、selection_generation、committed_sequence；它不改变 active_session。UI 不得用旧 reply 覆盖订阅的较新正文或草稿。

`save_preferences(expected_revision,preferences)` 对 SQLite 中的实际修改版本执行 CAS，成功后发布真实读回值。preferences_revision 跨重启保留，独立于 schema/model version；旧请求返回 Conflict。storage schema v3 从 v1/v2 保留既有偏好，并增加独立计数。删除对象导致的选择清理也递增该值。侧栏宽度遵循 240–320；选择身份必须存在。持久视图偏好不会进入会话业务 reducer。

### Issue #9 项目与会话操作

`select_project(Option<ProjectId>)` 保持项目与当前会话归属一致：换到另一个项目时清空不属于它的会话选择，选择 None 同时清会话；相同项目保留当前会话。不会停止其他项目任务。需要在另一项目新建时显式传 project_id，不把 selected_project 静默解释为独立默认目录。storage 的选项目/选会话也保持这一关系；重建会修复旧不一致偏好，写入仍有实际修订号。

`create_session(project_id,agent_id,title)` 验证目录与已配置 Agent，分配一次本地身份，经统一 core 管线创建、持久化 Connect，再保存选择；返回 SessionId 表示本地记录已创建，不代表 Ready。创建已确认后，连接或偏好失败会保留真实状态、last_error 并仍返回该 ID，防止 UI 重试制造第二条记录。新 external 只由 Runtime 的真实 SessionRegistered 提供；已有 external 继续连接必须 resume:true。

`remove_session(id)` / `remove_project(id)` 由应用后台协调。未连接/已释放记录直接走 core 删除；其他目标先 CloseSession，继续处理其他会话，直到相同 session+epoch 的权威资源释放事实已持久化才删。删除项目需全部目标会话释放后一个 DeleteProject 原子移除索引。等待最多 15 秒，超时保留记录并返回 ReconcileFirst，不在迟到释放后偷偷完成已超时删除。同一对象的重复请求共享等待；重叠删除、目标重连、被删项目的新建会话会明确 Busy。关闭后的历史保留，直到删除命令真正成功。

SessionSnapshot.resources_released 是 core 原始资源事实的只读投影，serde 对旧投影保守默认 false。UI 不以 Disconnected/取消请求/工单完成代替该事实。closing_handoff 未核实时保留记录并要求权威核对，服务不伪造释放或猜测进程所有权。pending_removals 展示实际正在等待的目标和会话；错误通过该操作 future 与 last_error 返回。

`relocate_project(id,directory)` 是用户明确选择新目录的窄操作。所有旧连接必须已确认释放，无未核实 closing_handoff；否则 Busy/ReconcileFirst，不偷偷关闭连接。后台规范化目录并核对已登记身份；目录冲突返回 Conflict，不合并项目。只有 Project.directory 更新，历史正文、文件引用、差异、运行和 external 原样保留，不按新根目录重写旧路径；后续 SDK 恢复拒绝则保留真实错误/历史，不悄悄创建新远端上下文。

project_directories 为每个项目提供最近一次后台目录检查的路径、checked_at 与可选错误。启动、打开/选项目、新建/连接和显式重新定位时检查；目录失效不丢历史，禁止隐式回退 cwd，用户可关闭旧连接后重新定位。相关文件系统查询全部留在 storage worker。选择切换先清旧 active_session，再读取新归属，读失败也不会把旧会话正文挂到新选择下。

## Agent 目录与安装

服务直接重导出 agent-catalog 的 DTO。`discover_agents()` 仅做本机发现，`refresh_agents()` 明确刷新官方 Registry（失败/缓存状态由 catalog 提供）。长发现、校验、安装任务并发运行，不占用命令处理循环。完成结果通过同一后台消息流应用。

已配置定义以已提交 AgentDefinition 为准。发现按 registry_id 或已校验 launch 复用 AgentId；过期探测只能更新仍与其起始定义一致的记录。刷新不会覆盖用户刚修改的 executable/args。

`prepare_install(registry_id)` 先持久化/复用稳定 AgentId，再产生包含来源、精确版本、目标目录、先决条件和命令的 InstallPlan。`install(plan_id)` 只接受本实例准备过且尚未消费的计划；确认后移入 running_installations，不能隐式升级或重复执行。编辑该 Agent 会使旧计划失效。已开始的安装由 catalog 管理目标互斥和进程释放；服务不另造安装器。

`configure_agent(optional_id,name,ManualLaunch)` 调用 catalog 的真实 launch 校验，规范化后复用本地身份，再用 PutAgent 提交。配置仅包含 executable + args，无环境、凭据或全局 CODEX_PATH。安装/手动配置成功后先发布已提交目录，再自动发起 Agent-only authentication Discover（真实 initialize）。完成配置/安装不表示登录成功。

## 临时认证与资源解析

`discover_authentication(agent_id)` 分配新的 Agent-only AuthenticationScope epoch。`authentication(scope,action)` 支持 Discover、Authenticate、Interact、Cancel，使用 core 原类型。新 scope 使旧 Agent-only notice 失效；session scope 独立按本地 session、Agent 和连接 epoch 检查。notice 按当前 scope 与主请求身份过滤，不把旧请求的结果覆盖新认证。

认证有专门 FIFO 入队器，调用 Runtime AuthenticationPort 后并发等待完成，因此用户交互或取消不排在登录完成之后。方法、状态、opaque interaction handle 仅在内存，永不进入 journal/SQLite；URL、凭据、SDK responder 和 secret 字段仍只在 Runtime。Methods 不表示 Authenticated。

AgentResolver 只返回已提交的目录定义。AttachmentResolver 校验 session/AttachmentId 归属，只读取 Managed blob，由 storage 在受控附件目录解析。Send resolver future 通过消息/oneshot 桥接 LocalSet，不携带非 Send storage future 跨线程。

## 退出

`shutdown()` 拒绝新普通操作、持久暂停现有会话队列，然后请求 Runtime 清理并继续接收最终事实。待 Runtime 停止、ticket/auth 转发结束、已接受目录任务完成和事件提交排空后关闭 SQLite，才回复。失败明确返回，不能把终止请求当释放确认。app 保留 handle 时，关设置页或切换视图不释放资源；最后一个公共 handle 丢弃也触发同一路径。正式窗口/托盘退出由 app 另行接线。

本轮不实现 #9/#10 专属交互，不额外发 prompt 验证 Agent 回答；服务检查与真实窗口验收分别记录。
