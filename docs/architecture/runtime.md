# Codex ACP Runtime

Issue #7；对应 R-11、R-14～R-21、R-29、R-33～R-36、R-38、R-40、R-43～R-45、R-51、R-57、R-62，支撑 AC-03～AC-05、AC-09、AC-12、AC-19。`agent-runtime` 已提供可独立调用的后端与真实检查入口；app/UI、SQLite、安装器和托盘装配仍属后续任务。

## 版本与启动

已实际配合工作的组合为 Rust 1.98.1、ACP Rust SDK 5.0.0 / schema 1.15.1、Node 24.14.0、codex-acp 2.5.0、Codex CLI 0.166.0。Cargo.lock 由集成负责人维护；没有启用草案 v2 或额外不稳定协议功能。

本地固定安装示例，依赖和 npm lock 留在应用数据或被忽略的开发目录，不能提交安装内容或用户数据：

```powershell
npm install --prefix .local/agents/codex-acp-2.5.0 --save-exact --no-audit --no-fund @agentclientprotocol/codex-acp@2.5.0 @openai/codex@0.166.0
```

`AgentDefinition.launch` 使用 native `node.exe`，参数为适配器 `dist/index.js` 的绝对路径；不直接启动 `.ps1`/`.cmd`，不在运行阶段调用 npx 下载。RuntimeOptions 可指定已解析的 native Codex exe，经子进程 CODEX_PATH 选择；不把 npm wrapper 当 exe。当前实现核对 initialize 返回的适配器名称与 2.5.0 版本，不冒用未经验证的其他版本映射。

## app 如何交付 effects

遵守 [共享契约](contracts.md)：先 reducer，原子保存 journal/checkpoint，发布状态，之后交付 effects。**按已提交 sequence、同一 Transition 的 effect 顺序同步调用 enqueue**，再并发转交每个 ticket 的事实：

```rust,ignore
for effect in committed_effects {
    let ticket = runtime.enqueue(effect);
    // app_tasks 与 sink 由应用服务持有，生命周期不依附当前页面。
    app_tasks.spawn(async move { ticket.forward(&sink).await });
}
```

`RuntimePort::execute` 本身也在方法调用时同步入队，其返回 future 负责转交事实和等待结束；不能把方法调用整体藏在无序 spawn 的 async body 中。`RuntimeTicket::is_enqueued` 仅表示进入本地服务队列，不表示 Agent 接受。

Connect 的 ticket 转交该连接的持续事件，直到释放资源。不能串行 await Connect 结束后才交 Submit，也不能等待长 prompt 完成后才交 Cancel/回答。app 保留后台任务与 Runtime 服务；视图仅读取 typed snapshot。

入口和会话通道都有界；拒绝入队会回传原会话/运行/attempt 的 NotSent、配置拒绝或响应 Unknown。已入队但失去执行者时保留 Unknown，不把没有证据的派发当作未消费。普通文本在会话 actor 收到 Submit 时开始协议请求；附件异步读取保留传输开始顺序，取消可在读取完成前使输入明确 NotSent。这里的准备队列只管 I/O，不是另一套用户排队状态。

RuntimeEventSink 的事件回到同一提交管线，宿主分配 sequence。SDK 派发回调只转交到有界通道；不等用户或数据库。队列满时受控断开并报告未知结果，不静默丢事件后宣称成功。正常结束前按顺序转交正文、结构化内容、消息完成与原 prompt 返回；不会用普通 RunEnded 代替在飞 prompt 的结束。

## 状态与转换

- 每会话独立进程、stdio 和连接；后台 OS 线程运行 Tokio current-thread + LocalSet。模型、思考强度、权限和指令来自真实报告，不设置产品模型白名单。
- 新会话只在 Agent 实际提供 FullAccess 选项时请求它，并保存返回的完整配置；恢复会话不重置用户选择。配置串行派发，完整报告先于特定 RequestId 的结算，旧 Run 配置快照不改写。
- 文本保留原文；工具、计划和差异使用本地稳定 ID、真实外部关联及所属 RunContext。先发布被引用对象，再发布消息中的结构化引用。工具更新不影响其他会话，不把共享目录整个 Git diff 归到当前会话。
- 权限和问题保存原 responder，以 RequestId + epoch + 原运行查找。只发送 Agent 给出的选项；交付 Submitted 不代表 Agent 二次确认或工具成功。未知字段、无法验证的约束明确 Unsupported，decline/cancel 分开；撤回与取消不会复活旧请求。
- 标准取消等待原 prompt 的 cancelled 结果；失败、断连、未知结果保留输入并暂停队列。SDK 请求级取消的本地成功不冒充运行停止。
- steering 仅在顶层能力和已核实的 promptRequired 行为均可用时派发；injected、startedNewTurn、promptRequired、failed 分别转换。超时/失败不盲重发，Runtime 不自行派发用户队列。
- 优先协商无回放 resume；load 的历史通知标记 AgentReplay，保留本地历史并使用既有 Partial/诊断降级。没有远端身份时不造 RunId/游标，也不按正文 hash 去重。旧运行恢复能力当前明确不支持。
- 输入图片由 app 注入 AttachmentResolver，返回现有 Attachment 与受控 blob 字节；文件引用保留实际位置和所选行。未提供 resolver、嵌入普通文件不支持或归属不匹配时，在协议派发前明确拒绝。非文本 Agent 内容的未知结构保留在 Unsupported 内容块，不能假装已接通原生图片输出展示。

## 临时认证

实现 core 的独立 AuthenticationPort / AuthenticationNoticeSink，**不经过 Command、Event、JournalRecord 或 StoragePort**。Agent-only 连接通过 app 注入的 AgentResolver 获得现有 AgentDefinition；也可复用此前 Connect 的启动定义。认证连接使用按 Agent 分配的 epoch；已有会话可使用与真实连接一致的 session scope，绝无伪造 RunId。

Discover 只报告实际方法和可证实的状态。独立发现的状态保持 Unknown；成功建立会话或官方 authenticate 成功才有登录复用/认证成功依据。实际方法随客户端能力协商变化：本轮 URL/form 客户端发现 api-key、chat-gpt、chat-gpt-device-code；需要密钥输入的 api-key 明确为 SecretEntryUnsupported，terminal 类型不发送给 authenticate。

Authenticate 采用 SDK 5 的标准请求，只发送已报告且可用的 methodId。临时 URL、原 RPC 身份和 responder 留在 Runtime；Notice 只含 opaque interaction ID。对 Runtime 收到的 URL 交互，OpenOfficialFlow 才调用系统浏览器并回应 accept；Decline/Cancel、协议撤回、完成与换连接都会使原句柄失效。URL/secret 不写 Runtime 日志，不把未知 secret 表单转换成聊天问题；本轮不提供凭据输入入口。运行中的会话拒绝登录动作，避免在工作途中改变认证连接。

## 资源与关闭

Windows 使用 process-wrap 的 JobObject + KillOnDrop，先挂入自有 Job 再运行子进程；Node、Codex 与其后代均归该 Job。stdout 专供协议，stderr 独立消耗且不原样输出/持久化。关闭协议后给予短暂 EOF 宽限，再终止自有 Job 并检查首次 wait 的内部结果；只有核实释放才发 ResourcesReleased。清理不等待 app 事件消费者。

`shutdown().await` 请求停止并等待服务退出；app 仍须消费各连接的最终事实。超过清理期限会返回需核对错误，不能把停止请求当作成功。旧实例 abandoned_epoch 没有可核实所有权句柄时，ReconcileClosingResources 返回 Unconfirmed；不按进程名或猜测 PID 清理。跨实例持久所有权核对仍需后续恢复/退出整合。

## 可复现检查及边界

四个参数依次为 native Node、适配器 JS、native Codex exe、隔离空目录的绝对路径。CLI 复用官方现有登录，不读取或打印凭据：

```powershell
cargo run --locked -p agent-runtime --example codex_probe --jobs 4 -- "$node" "$adapter" "$codex" "$scratch"
cargo run --locked -p agent-runtime --example runtime_smoke --jobs 4 -- "$node" "$adapter" "$codex" "$scratch"
cargo run --locked -p agent-runtime --example runtime_smoke --jobs 4 -- "$node" "$adapter" "$codex" "$scratch" --auth-only
```

已取得真实 SDK prompt/stream/cancel；共享 RuntimePort + reducer 的两会话实测覆盖交错输出、另一会话并行完成、配置结算、真实工具运行后停止、保留暂停队列和资源释放。该 CLI 的 journal 在内存中，不是 SQLite 验证。另有实际进程树快照与退出核对，以及新增非持久化认证发现实测。后续入队/映射调整通过必要定向检查，按集成负责人要求没有重复同一整套真实场景。

尚未完成 app/UI 装配、实际浏览器/设备码登录闭环、真实审批/问题/steering/恢复与网络故障场景、原生图片输出、IME/DPI、托盘或用户验收。源码映射与纯检查不替代这些证据。
