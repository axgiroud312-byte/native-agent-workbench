# Agent 发现与独立安装

`agent-catalog` 负责 [MVP Spec](../specs/native-agent-workbench-mvp.md) 的 R-11、R-12、R-13、R-44、R-59 后端部分，为 AC-03 提供目录、安装和本地校验能力。它不承担设置 UI、业务持久化、认证结果或连接验收。

## 服务接入

公共类型从 crate 根导出，字段在 `src/api.rs`，门面在 `src/lib.rs`。

```rust,ignore
AgentCatalog::new(data_root: PathBuf) -> Result<AgentCatalog, CatalogError>
catalog.refresh(configured: Vec<AgentDefinition>, policy: RefreshPolicy)
    .await -> Result<CatalogSnapshot, CatalogError>
catalog.discover(configured: Vec<AgentDefinition>)
    .await -> Result<LocalDiscovery, CatalogError>
catalog.prepare_install(registry_id: &str, agent_id: AgentId)
    .await -> Result<InstallPlan, CatalogError>
catalog.confirm_install(plan_id: InstallPlanId)
    .await -> Result<InstallOutcome, CatalogError>
catalog.plan_state(plan_id: InstallPlanId) -> Option<InstallState>
catalog.validate_manual(agent_id: AgentId, name: &str, launch: ManualLaunch)
    .await -> Result<ManualValidation, CatalogError>
catalog.adopt_candidate(candidate_key: &str, agent_id: AgentId)
    .await -> Result<AgentDefinition, CatalogError>
CatalogSnapshot::local_only(local: LocalDiscovery, at: Timestamp) -> CatalogSnapshot
```

构造要求绝对的应用数据目录，不安装、不发现。目录句柄可克隆；全部 I/O 由 service 的后台执行器调用。`refresh` 接收已持久化的 Agent 定义，更新其发现证据，保留原 ID、来源、启动参数和分发信息。Registry 行与本地候选没有业务 AgentId；service 分配或复用 ID，再传给安装/配置方法，成功后提交现有 core `PutAgent`。不创建另一套业务定义。

`discover` 不触网；服务可用 `CatalogSnapshot::local_only` 发布结果，此时 Registry 明确为未刷新。`refresh` 返回官方目录与本地证据；网络失败且无缓存时仍返回本机结果。元数据失败信息在 `RegistryEvidence`，不抛弃本机配置。

## 官方来源与缓存

唯一 Registry 来源为 [官方 Registry v1](https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json)。当前解析 root `version` 主版本 1、`agents`、条目的 `id/name/version/distribution`；不执行远端 shell 文本。未知协议格式不会覆盖已有缓存。HTTPS 禁止跳转，连接超时 8 秒、总请求 20 秒，响应限制 4 MiB。

缓存位于 `<data>/agent-catalog/registry-v1.json`，包含来源、格式版本、首次取得/最近校验/过期时间、ETag、Last-Modified 与原始公开元数据。`PreferCache` 使用有效缓存；`Revalidate` 条件请求，304 保留原内容取得时间、更新校验和过期时间。网络错误使用缓存并标记 offline/stale；坏 schema 保留旧缓存并给出问题。缓存用同目录临时文件原子发布。

Windows 分发只按实际字段报告，不从条目存在推断可安装。当前仅 Codex ACP 2.5.0 的精确 npx 包及 Windows x64/arm64 标为支持。其他 Agent、未知分发或未核实版本明确不可安装；现 Runtime 的其他 Agent 映射尚未实现。Codex 条目未提供 Windows binary/checksum，不虚构它们。

## 本机发现与手动配置

只发现 PATH、已有启动定义和应用受控安装目录，不扫描凭据或全盘。Node 使用 native exe；npm 通过已核实的 `npm-cli.js` 由 Node 启动；Codex npm wrapper 解析到该包的 Windows native exe。`.cmd/.ps1/.bat` 不直接启动。手动选择已知 `codex-acp` wrapper 时可解析成 Node 加适配器 JS；带额外参数的 wrapper 必须由用户改成明确 native 配置，避免悄悄丢参数。

`ManualLaunch` 只有 executable/args，不包含环境或秘密字段，并拒绝已知凭据参数。校验路径、Windows exe 头、适配器包身份/入口/版本与最近的配套 native Codex；其他启动形状给出实际不兼容原因。发现结果区分执行 `--version` 与只读取包 manifest 的证据。已安装不等于已连接或已登录。

产品 Runtime 移除继承的 `CODEX_PATH`，让适配器从自己的依赖解析 native Codex，防止多个 Agent 混用全局引擎。显式 `RuntimeOptions` override 仅保留给说明清楚的探针，不写入业务定义。开发资源目录不作为产品默认安装路径。

## 审阅与执行计划

`prepare_install` 需要先加载官方 Registry，验证 native Node/npm，返回进程内不透明 `InstallPlanId`。计划包含服务提供的 AgentId、官方来源、精确适配器与 native 版本、命令 program/args/cwd、目标目录和前置条件。该方法只读本机前置，不创建目标目录或启动安装。

固定组合为 adapter **2.5.0** / native Codex **0.166.0**，后者来自 #7 的真实显式路径验证。适配器原 manifest 的 `^0.165.0` 不保证此组合；安装时同时固定顶层包与适配器依赖 override。目标为 `<data>/agents/codex-acp/2.5.0-codex-0.166.0`，不更新全局工具。

实际命令形状（所有参数独立传递，无 shell）：

```text
<native-node> <npm-cli.js> install --prefix <target> --save-exact
  --no-audit --no-fund --registry=https://registry.npmjs.org/
  --userconfig=<target>/.npmrc --globalconfig=<target>/.npmrc-global
  @agentclientprotocol/codex-acp@2.5.0 @openai/codex@0.166.0
```

用户确认该实际计划后，service 调用 `confirm_install(plan.id)` 并持有任务至结束；关闭设置不取消后台安装。不能用编辑过的 DTO 执行命令。进程内及目标目录锁阻止同目标并发；相同已完成 plan 再确认返回原结果。目标只接受空目录或标记属于本应用的相同组合；解析目录必须留在受控根内，不清理用户其他目录。

安装使用自有 manifest、隔离 npm 配置、公开 npm 来源，单请求 60 秒、总执行 10 分钟、下载自动重试为零。Windows npm 及其后代归本次 Job；stderr 只保留有界临时缓冲，外部失败只带安全错误码，不输出原始诊断或秘密。

Runtime 和 Catalog 共用本地修补的 `process-wrap` 10.0.1：只有 `ACTIVE_PROCESS_ZERO` 能确认整个 Job 为空，父退出与 Job-empty 分别缓存，取消后重新等待仍可靠。超时或等待异常会请求终止并做 5 秒内的 cleanup wait；确认全部退出后可明确重试。`CleanupUnconfirmed` 表示整个进程树尚未确认释放，保留目标锁、拒绝同目标重试，不能把发出终止请求当成完成。服务意外取消安装 future 同样保留锁；进程重启后未确认的旧锁不自动删除。补丁来源、修改范围与许可保留见 `vendor/process-wrap/WORKBENCH_PATCH.md`。

安装成功后重新校验确切包/原生路径，并实际运行适配器和 native Codex `--version`。返回共享 `AgentDefinition`；service 提交持久化，再用现 Runtime 做真实认证发现/握手，只有后者能证明连接成功。

## 验证范围

2026-11-04 的只读 probe 实际取得 Registry v1.0.0、44 条、Codex 条目 2.5.0，并执行版本发现：Node 24.14.0、npm 11.9.0、全局 native Codex 0.166.0。显式传入的开发适配器最近依赖为 Codex 0.165.0，正确显示不兼容；刷新保留其 ID/启动/来源。生成 Prepared 计划，没有执行安装或启动 Agent。

```powershell
cargo run --locked -p agent-catalog --example catalog_probe -- "$dataRoot" "$node" "$adapterJs"
```

集中测试覆盖离线/坏 schema/304 缓存和安装失败/重复确认/清理未确认的门槛。正式独立安装、配套引擎真实握手、设置窗口操作和用户验收须单独记录；本文不把上述只读结果当成 AC-03 完成。
