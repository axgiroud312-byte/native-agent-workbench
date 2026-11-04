# Agent Workbench

Windows 优先的开源原生 Agent 工作台，使用 Rust、GPUI Kit 和 ACP，先把 Codex 的日常工作体验做好。

**当前状态：规格阶段。尚未实现应用、运行真实 Agent 或发布安装包。**

目标是一个不透明炭灰色、弱外框和弱顶栏的双栏窗口：左侧组织项目与会话，右侧完成输入、流式对话、工具执行、审批和结果查看。多个会话可在同一项目目录并行工作；关闭窗口后收进托盘继续运行。

- [MVP 开发规范与验收要求](docs/specs/native-agent-workbench-mvp.md)
- [贡献与验证约定](CONTRIBUTING.md)
- [Agent 开发约定](AGENTS.md)
- [MIT 许可证](LICENSE)

## 交互方向

整体对话与输入体验参考 OpenChamber、T3 Code、Paseo 和 Codex 的实际实现，包括流式文字、工具与计划、权限和提问、附件、命令选择器、模型和思考强度选择、途中补充和排队。用 GPUI Kit 实现原生控件与真实图标；具体范围、参考源码和不采用的内容见开发规范。

## 计划中的技术基础

Rust + GPUI Kit + ACP Rust SDK + Tokio + SQLite。主界面不使用 Electron、React 或 WebView。优先通过 `agentclientprotocol/codex-acp` 接入 Codex；其他 Agent 按实际能力适配。

依赖版本在首次实现时核查并锁定。目前没有 `Cargo.toml`，因此不提供尚不能运行的构建命令。

## 本地开发技能

`skills-lock.json` 记录 GPUI Kit 技能来源；技能安装副本不提交到仓库。需要时在项目根目录恢复：

```powershell
npx --yes skills experimental_install --frozen-lockfile
```

安装工具和技能是开发辅助，不是应用运行依赖。技能规范与本项目明确要求冲突时，以项目需求为准。

## 当前仓库检查

```powershell
python .github/scripts/check_docs.py
git diff --check
```

这里只检查文档结构、引用与需求编号。它不能证明应用编译、窗口交互或 Agent 接入已经可用。
