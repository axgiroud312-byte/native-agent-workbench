# 第三方来源

项目自身的 MIT 许可证不覆盖第三方代码或图标。

- UI 使用 [GPUI Kit 0.9.0](https://crates.io/crates/gpui-kit/0.9.0)，其包声明为 Apache-2.0；GPUI、基础组件和平台实现由它选择配套依赖。
- 图标来自 `gpui-kit-assets 0.9.0` 随附的 Lucide 图标资源。使用实际 SVG 资源，由 GPUI 原生渲染；完整图标许可保留在 [licenses/LICENSE-LUCIDE](licenses/LICENSE-LUCIDE)。
- 持久化使用 [rusqlite](https://github.com/rusqlite/rusqlite) 0.41.1 与 `libsqlite3-sys` 0.39.1，两者包声明为 MIT。启用 bundled SQLite；SQLite 库本身遵循其作者的[公共领域声明](https://www.sqlite.org/copyright.html)，不由本仓库 MIT 重新授权。
- ACP 使用 [Rust SDK](https://github.com/agentclientprotocol/rust-sdk) 5.0.0 与 schema 1.15.1，包声明为 Apache-2.0；进程管理使用 [process-wrap](https://github.com/watchexec/process-wrap) 10.0.1，包声明为 Apache-2.0 OR MIT。
- `vendor/process-wrap` 保留该发布版本的源码、版权与双许可证，包含针对 Windows Job 退出判定的局部修补。修改说明见该目录的 `WORKBENCH_PATCH.md`；第三方源码不采用本项目的 MIT 声明替代原许可证。
- [codex-acp](https://github.com/agentclientprotocol/codex-acp) 作为独立进程安装与运行，采用其自身 Apache-2.0 许可；应用仓库不包含其安装目录、Codex 凭据或私人会话。
- Registry 的 HTTPS 获取使用 [reqwest](https://github.com/seanmonstar/reqwest) 0.12.28，包声明为 MIT OR Apache-2.0。安装元数据来自官方 ACP Registry，Agent 安装包仍遵循各自许可证。
- 其余依赖版本由 `Cargo.lock` 记录，各自遵循原有许可证。正式分发安装包前，应按实际启用的依赖与打包内容生成完整第三方许可清单；当前没有正式分发包。

上游项目的行为和结构可作为参考；直接复制实现时另行核对对应源文件的许可。
