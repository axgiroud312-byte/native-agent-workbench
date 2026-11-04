# Agent Workbench

Windows 优先的开源原生 Agent 工作台，使用 Rust、GPUI Kit 和 ACP，先把 Codex 的日常工作体验做好。

**当前状态：已建立 Rust + GPUI Kit 最小启动工程。Agent、会话存储和托盘尚未接入，没有正式安装包。**

目标是一个不透明炭灰色、弱外框和弱顶栏的双栏窗口：左侧组织项目与会话，右侧完成输入、流式对话、工具执行、审批和结果查看。多个会话可在同一项目目录并行工作；关闭窗口后收进托盘继续运行。

- [MVP 开发规范与验收要求](docs/specs/native-agent-workbench-mvp.md)
- [贡献与验证约定](CONTRIBUTING.md)
- [Agent 开发约定](AGENTS.md)
- [MIT 许可证](LICENSE)

## 交互方向

整体对话与输入体验参考 OpenChamber、T3 Code、Paseo 和 Codex 的实际实现，包括流式文字、工具与计划、权限和提问、附件、命令选择器、模型和思考强度选择、途中补充和排队。用 GPUI Kit 实现原生控件与真实图标；具体范围、参考源码和不采用的内容见开发规范。

## 技术基础

Rust + GPUI Kit + ACP Rust SDK + Tokio + SQLite。主界面不使用 Electron、React 或 WebView。优先通过 `agentclientprotocol/codex-acp` 接入 Codex；其他 Agent 按实际能力适配。

初始工程使用 Rust **1.98.1**、GPUI Kit **0.9.0**，通过 `rust-toolchain.toml` 和 `Cargo.lock` 固定工具链与依赖。ACP、Tokio、SQLite 随后续业务实现加入，当前不引入空模块和未使用依赖。

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

启动后显示炭灰双栏窗口，可点击“选择文件夹…”或按 `Ctrl+O` 打开系统文件夹选择器。选择只改变当前窗口的工作目录展示，不写入所选目录；当前尚不保存项目，也不能调用 Agent。`Ctrl+Q` 或关闭窗口退出程序，托盘行为属于后续开发。

目录结构：`crates/app/src/main.rs` 负责启动与生命周期，`theme.rs` 集中主题与尺寸，`workbench.rs` 持有窗口状态和目录选择交互。随着实际功能增长再拆分 crate。

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

GitHub Actions 检查文档、Rust 格式和 Windows 构建。开发版 exe 仅作为短期构建验证产物保留，不是正式发布包。

真实窗口与目录选择的本机检查结果将在初始化验证完成后记录。MVP 中的 Agent 调用、中文输入框、托盘和持久化仍待实现及验收。
