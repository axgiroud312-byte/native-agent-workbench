# 本地存储与项目/会话服务（Issue #5）

`workbench-storage` 实现 [共享 StoragePort](contracts.md)，并提供 app 可直接调用的 WorkbenchService。它只依赖 core、rusqlite、serde/serde_json 和异步回复通道，不依赖 GPUI/ACP，不启动 Agent、不清其他客户端历史。覆盖 R-08、R-09、R-17、R-25、R-49、R-50、R-52、R-57～R-59，为 AC-02、AC-06、AC-14、AC-16、AC-19 提供真实 SQLite/文件基础；不是 UI 或 Agent 验收。

## app 的接入入口

1. `SqliteStorage::open_default().await` 打开 OS 应用数据目录；`open_at(absolute_path)` 是显式应用配置/隔离测试入口。
2. `WorkbenchService::start(storage, fresh_restart_id, at).await` 读取 checkpoint+tail，经 core replay 恢复，再事务提交 Restart。每个新 app 实例必须使用新的启动 CommandId；同一 worker 的同一启动 ID 可重复取得服务，其他启动所有者被拒绝。
3. 应用后台装配使用 `open_project/create_session/select_project/select_session/remove_project/remove_session`，或统一 `execute(CommandEnvelope)`；Runtime 事实走 `ingest/ingest_batch`。正式 UI 通过 [ApplicationService](application-service.md) 调用，不直接拥有 storage/runtime。
4. 返回的 CommittedUpdate 含同一已提交状态的 WorkbenchSnapshot、受影响且仍存在的 SessionSnapshot、decisions、effects 和维护错误。每个 effect 带 sequence/ordinal，app 按该顺序交 Runtime；存储层不执行它们。不要再在 UI 自存 live_state/pending_effects。
5. 查询使用 `snapshot/list_projects/project/list_sessions/session/messages`；`command_result(id)` 读取原持久决策，重复拒绝不会变成成功。`preferences_with_revision/save_preferences_if_revision` 对布局、选择和展开偏好执行真实持久修订号 CAS；低层 `preferences/save_preferences` 仍可用且所有写入同步递增修订号，不另建业务状态机。

服务使用后台线程中的 core reducer 作为唯一业务更新逻辑。可以选用 WorkbenchService，或由 app 直接使用 StoragePort 配合自己持有的 core 状态；不要让两者同时充当业务更新所有者。低层 StoragePort.history 是内部日志分页，可能包含诊断；UI 读取 typed SessionSnapshot/MessagePage，不渲染原始事件 JSON。

## 路径与线程

Windows 默认 `%LOCALAPPDATA%/AgentWorkbench`；macOS 使用用户 Library/Application Support，其他平台使用绝对 XDG_DATA_HOME 或用户 `.local/share`。缺少有效位置时报错，绝不回退到当前项目目录。

目录内固定为 `workbench.sqlite3`（含 SQLite WAL/SHM）和 `blobs/<BlobId>.blob`。Project.directory 仍是原项目目录，不创建工作副本。数据库打开、迁移、事务、读取、文件 canonicalize、附件文件 I/O 全在名为 workbench-storage 的专用线程；调用方只得到异步 oneshot 结果，不在 UI/render 执行同步 SQLite 查询。

已入队操作即使回复 future 被丢弃也可能继续提交，重试须复用稳定命令身份并查询真实结果。`shutdown().await` 等已排队事务结束并关闭 SQLite，再允许释放隔离目录；克隆的句柄也随之关闭。

## schema 与显式转换

业务版本 1、SQLite user_version 3 分开。数据库具有固定 application_id，非本应用数据库或未来版本拒绝打开，不清空、不重建、不生成备份。schema 只有本 crate 维护。

| 表 | 职责与格式 |
| --- | --- |
| workbench_state | 单行 sequence + Versioned<WorkbenchState>，保存唯一业务状态、去重身份及墓碑 |
| journal | 宿主 sequence、会话/项目关联、CommandId/EventId、标准 JournalRecord JSON；已删除对象可保留顺序/身份而去掉 payload |
| project_index | 项目 ID、统一目录 key、展示名、Versioned<Project> 查询投影 |
| session_index | 项目 FK、Versioned<SessionRecord>、Versioned<SessionSnapshot>；包含消息、草稿、队列、运行、请求、附件元数据的 typed 投影 |
| preferences | 独立 Versioned<WorkspacePreferences> |
| preference_revision | 实际偏好修改修订号；CAS、选择修改与删除清理均同步维护 |
| blobs | 应用托管 blob ID、真实长度、pending_gc 状态 |
| blob_refs | blob/会话/附件 ID，外键与计数用于所有权回收 |

查询投影必须在同一事务内从 core 状态生成，禁止 UI 直接改 SQL 行。serde 转换都指定具体版本/类型，未知版本、损坏记录及 checkpoint 水位不匹配均报错，不静默回退成空历史。SQLite 恢复仅查询本数据库登记的会话，不扫描 Codex 历史。

v1 定义原始 checkpoint/journal/preferences/blobs；v1→v2 真正 ALTER blobs 增加 pending_gc、重建 journal 增加关联字段、建立项目/会话/引用投影并回填历史。v2→v3 增加 preference_revision，保留既有偏好、业务记录和附件；已有偏好从修订号 1 开始，空偏好为 0。全部 DDL、回填、application_id/user_version 放在一个事务内；中途 SQL 或转换失败回滚原表、列、版本和数据，没有以 CREATE TABLE IF NOT EXISTS 代替迁移。实现依据 SQLite 的 [事务语义](https://www.sqlite.org/lang_transaction.html) 与 [应用自管 user_version](https://www.sqlite.org/pragma.html#pragma_user_version)。

## 提交与历史分页

StoragePort.commit 以 expected_sequence 开 IMMEDIATE 事务；读取真实状态、按唯一 reducer 重算记录，验证 proposed checkpoint 完全一致，才保存 journal、checkpoint、查询投影和附件引用。陈旧 sequence 返回 SequenceConflict，不覆盖新状态。所有 effects 只在 COMMIT 成功后返回，SQL 失败不返回假成功。

服务 ingest_batch 可把多条文本/语义事件写进同一事务，仍逐条保留日志和决策、给 effect 标注原宿主序号；只在末尾生成一次数据库投影。单条业务拒绝遵守 core 语义记录 Rejected，事务存储故障则整批回滚。WAL + synchronous FULL 用于持久边界；SQL 和 JSON 参数分开绑定，不写凭据到日志。

StoragePort.history 每页 1～500 条，before_sequence 排除自身，从最近范围向前翻页，页内按宿主顺序升序返回，next_before_sequence 不重复边界。WorkbenchService.messages 同样支持排除 MessageId 的 typed 消息页。当前 checkpoint/会话投影仍包含完整历史，消息分页在后台截取；这不是已实现数据库分段历史加载或海量数据性能验收。

## 项目、选择与删除

open_project 在后台对真实目录 canonicalize，Windows 统一分隔符/大小写 key，其他平台保留区分；同目录别名复用原 ProjectId 和原用户展示名。UUID 仍是身份，目录 key 只用于项目复用；没有拿路径当 SessionId。Windows 规则面向普通不区分大小写的项目目录，特殊区分大小写文件系统需在后续显式支持，不静默创建工作副本。

创建会话只走 core CreateSession，使用 caller 保留的 SessionRecord/CommandId；Agent 必须已登记。选择项目和选择会话各自保存视图偏好，项目选择不会隐式隐藏/终止其他会话。读取按稳定 ID，中文路径与名字保留。

remove_session/remove_project 仍受 core 资源释放门槛约束；服务不杀进程，不把 Interrupted 当“已安全释放”。上层协调停止/关闭，收到权威释放事实后才移除。事务删除投影、清理选中已删除对象和项目展开偏好，并去掉该对象的 journal payload；checkpoint 留去重身份/墓碑。项目源码目录、FileReference 目标、ImportedFile.original_path 都不进入删除代码，也不调用 Agent 全局历史删除接口。逻辑删除不等于磁盘级安全擦除。

## 附件所有权

BlobId 只能映射应用 blobs 目录中的固定文件名；不接受任意删除路径。写入先 `.pending` 文件、sync、rename，再登记真实长度；相同 ID/相同字节可重复写，不同字节拒绝覆盖。事务失败留下的未登记文件不会被当作外部用户文件删除，也不会冒充已保存引用。读取检查应用目录/普通文件、catalog 和长度，损坏/丢失明确报错。

每个保留 Attachment 元数据是一个保守的引用根：草稿、队列、提交、消息和工具通过 AttachmentId 借用它，因此它存在时不回收字节；两个会话可借用同一 blob，引用分别计数。当前 core 尚无独立移除闲置附件元数据命令，本版主要随会话/项目删除释放根，不擅自修改 core 子对象。

引用消失时事务标记 pending_gc。回收在事务内确认应用 catalog 所有权且无引用，持久化 pending_gc 后才删该 blob 文件；新的引用不能指向 pending_gc，避免核查后又被借用。删除失败保留 catalog 待重试；服务返回 maintenance_errors（业务提交已成功），下次打开重试 pending 项。未登记文件不盲扫删除。文件操作只针对已验证应用目录下的单一 blob，不递归删除项目。

## 恢复与验证边界

低层 restore 提供 Versioned checkpoint + tail，纯回放不产生副作用；WorkbenchService.start 在开放命令前持久化 Restart，旧运行待核对、旧请求失效、未确认交付保留未知、队列暂停、草稿/附件/布局保留。普通 resume 不代表任务完成；关闭崩溃的资源交接遵守 core 门槛。本层不导入/调用真实 Agent，也不自动重发队列。

少量检查使用隔离临时数据目录和真实 SQLite/文件：旧 v1 升级及 DDL 失败回滚、事务 checkpoint 失败后完整回滚、服务重建后草稿/队列/布局保留、目录身份复用、CAS、防重复历史页、真实 PNG 字节往返/共享引用/删除范围。Agent 事件与释放证据来自共享契约样例，不冒充真实进程已运行/释放；图片检查只验证字节保存，不是渲染验收。
