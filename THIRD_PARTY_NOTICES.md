# 第三方来源

项目自身的 MIT 许可证不覆盖第三方代码或图标。

- UI 使用 [GPUI Kit 0.9.0](https://crates.io/crates/gpui-kit/0.9.0)，其包声明为 Apache-2.0；GPUI、基础组件和平台实现由它选择配套依赖。
- 图标来自 `gpui-kit-assets 0.9.0` 随附的 Lucide 图标资源。使用实际 SVG 资源，由 GPUI 原生渲染；完整图标许可保留在 [licenses/LICENSE-LUCIDE](licenses/LICENSE-LUCIDE)。
- 其余依赖版本由 `Cargo.lock` 记录，各自遵循原有许可证。正式分发安装包前，应按实际启用的依赖与打包内容生成完整第三方许可清单；当前没有正式分发包。

上游项目的行为和结构可作为参考；直接复制实现时另行核对对应源文件的许可。
