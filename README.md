# Agent Workbench

Windows 优先的开源原生 Agent 工作台，使用 Rust、GPUI Kit 和 ACP，先把 Codex 的日常工作体验做好。

**当前状态：共享契约、SQLite、Codex ACP 后端、原生设置与项目/会话操作已实现并编译通过。后端已有真实流式调用、停止和双会话证据；新界面的实际安装/连接与导航尚待窗口验证，原生对话发送仍在开发。尚无正式安装包。**

目标是一个不透明炭灰色、弱外框和弱顶栏的双栏窗口：左侧组织项目与会话，右侧完成输入、流式对话、工具执行、审批和结果查看。多个会话可在同一项目目录并行工作；关闭窗口后收进托盘继续运行。

- [MVP 开发规范与验收要求](docs/specs/native-agent-workbench-mvp.md)
- [可执行任务与依赖顺序](docs/task-plan.md)
- [贡献与验证约定](CONTRIBUTING.md)
- [Agent 开发约定](AGENTS.md)
- [MIT 许可证](LICENSE)
- [图标与依赖来源](THIRD_PARTY_NOTICES.md)

## 交互方向

整体对话与输入体验参考 OpenChamber、T3 Code、Paseo 和 Codex 的实际实现，包括流式文字、工具与计划、权限和提问、附件、命令选择器、模型和思考强度选择、途中补充和排队。用 GPUI Kit 实现原生控件与真实图标；具体范围、参考源码和不采用的内容见开发规范。

## 技术基础

Rust + GPUI Kit + ACP Rust SDK + Tokio + SQLite。主界面不使用 Electron、React 或 WebView。优先通过 `agentclientprotocol/codex-acp` 接入 Codex；其他 Agent 按实际能力适配。

工程使用 Rust **1.98.1**、GPUI Kit **0.9.0**、ACP Rust SDK **5.0.0**、Tokio 和 SQLite，通过 `rust-toolchain.toml` 和 `Cargo.lock` 固定工具链与依赖。

## Windows 开发与运行

需要 Rustup、CMake，以及 Visual Studio 2022 Build Tools 的 MSVC x64/x86 编译器和 Windows SDK。Rustup 会按项目配置安装 Rust 1.98.1 与 rustfmt。

如果缺少微软工具，可在终端安装（Windows 可能要求管理员确认）：

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools --exact --source winget --override "--wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --add Microsoft.VisualStudio.Component.Windows11SDK.26100"
```

然后在项目根目录运行：

```powershell
cargo run --locked
```

启动后显示炭灰双栏窗口。`Ctrl+O` 打开系统文件夹选择器并保存项目索引；`Ctrl+N` 从已配置 Agent 创建会话。项目与会话菜单提供关闭/重连、删除记录与明确选择新目录的操作；删除等待后台进程确认退出，不删除项目目录或其他客户端历史。当前对话发送仍待接入。`Ctrl+Q` 或关闭窗口会请求退出，托盘行为属于后续开发。

`Ctrl+L` 聚焦输入，`Ctrl+Shift+E` 聚焦导航，`Ctrl+,` 打开设置。设置支持 100/125/150% 应用缩放。开发者可以通过 `cargo run --locked -- --dev-fixture` 查看明确标记的布局样例；样例不调用 Agent，也不是用户历史。

设置已接入 Agent 发现、Registry 缓存、手动配置、安装计划审阅与官方认证方法。安装仅在明确确认后执行，使用应用专有目录，固定当前验证组合 codex-acp **2.5.0** / Codex **0.166.0**。发现方法或打开官方登录页面不代表认证完成；会话能力与配置仍以 Agent 的实际报告为准。

正式数据默认位于 `%LOCALAPPDATA%/AgentWorkbench`。开发验证可显式使用 `cargo run --locked -- --data-root <绝对数据目录>` 隔离数据；该参数与开发样例不同，仍会运行真实后台服务。

目录结构：`crates/app/src/main.rs` 负责启动与生命周期，`theme.rs` 集中主题与尺寸，`workbench.rs` 组合原生窗口；侧栏、正文、输入和服务入口分别位于同目录模块。

`crates/workbench-core` 提供独立于 UI、协议和数据库的共享业务类型、唯一状态更新器和异步服务接口；见 [业务对象说明](docs/architecture/domain-model.md) 与 [命令/事件契约](docs/architecture/contracts.md)。序列化和关键状态场景已有定向检查，不代表会话或 Agent 功能已可使用。

`crates/workbench-storage` 提供后台 SQLite、迁移、项目/会话与附件保存，见 [存储边界](docs/architecture/storage.md)。`crates/agent-runtime` 提供独立进程、ACP 事件转换、取消与临时认证，见 [Runtime 用法及真实检查](docs/architecture/runtime.md)。窗口状态与接入约定见 [UI 架构](docs/architecture/ui.md)。

`crates/workbench-service` 负责应用级提交顺序、事件回写与已确认快照，见 [应用服务](docs/architecture/application-service.md)。`crates/agent-catalog` 负责 [发现、缓存与安装](docs/architecture/agent-catalog.md)。UI 不持有另一套业务状态机。

## 本地开发技能

`skills-lock.json` 记录 GPUI Kit 技能来源；技能安装副本不提交到仓库。需要时在项目根目录恢复：

```powershell
npx --yes skills experimental_install --frozen-lockfile
```

安装工具和技能是开发辅助，不是应用运行依赖。技能规范与本项目明确要求冲突时，以项目需求为准。

## 检查与验证边界

```powershell
cargo fmt --all --check
cargo build --locked --workspace
python .github/scripts/check_docs.py
git diff --check
```

GitHub Actions 检查文档、Rust 格式和 Windows 构建，不额外上传 exe 或制作安装包。

2026-11-04 已验证：

- Windows x64 本机 `cargo build --locked --workspace --jobs 4` 成功，云端 Windows 构建也通过。
- 真实窗口能启动，炭灰双栏、中文文字和文件夹 SVG 图标正常显示。
- 鼠标打开系统选择器并选择含中文名称的本地目录后，侧栏与主区同步显示目录。
- `Ctrl+O` 重新打开选择器；Escape 取消后保留原目录，并恢复按钮可用状态。
- `Alt+F4` 关闭窗口后进程退出，没有遗留启动工程进程。

随后实现阶段已取得的证据：

- SQLite 真实临时目录检查覆盖事务回滚、迁移、重开、共享附件与删除边界。
- Codex ACP 后端在隔离目录完成真实流式输出、运行中停止和两个会话并行；跟踪的自有进程在结束后全部退出。
- 原生窗口完成两行中文文本输入、应用缩放及选择器取消检查。自动输入中文不等于中文输入法候选组合验收。
- 新应用服务检查覆盖真实 SQLite 偏好修改版本、旧值拒绝、切换归属、重复命令和已提交操作交付；当前设置接线已通过应用与 workspace 构建。
- 项目/会话检查覆盖两项目多会话、草稿归属、目录冲突与重定位、真实受控子进程关闭后删除、超时保留记录。该等待子进程不实现 ACP，这项检查不代表真实 Agent 或窗口流程通过。
- Registry 真实只读检查完成获取、工具发现与安装计划生成，尚未执行正式安装。Windows 自有进程检查验证了父进程先退出、后代仍存活时不会提前报告整组结束，以及取消等待后的正确清理。

这些证据不代表完整 MVP 或视觉方案已获用户验收。新设置的真实窗口/安装、登录交互、完整对话操作、重启恢复和托盘仍待后续实现与验证。
