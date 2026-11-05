# Agent Workbench · 设计 Token

状态：2026-11-05 用户已确认 [HTML 原型 v0.2](design-board.html)。本文件从该原型实际 CSS 提取视觉基准，供原生 gpui-component 实现使用。

原型 SHA-256：`E51DA03DDE8EE82CB8DF939DCF648D7D4E6EFDE38D75680F2DB1435AF5A06111`。

Token 命名是正式实现的语义映射，数值来自原型。个别原型规则有多次覆盖，以下使用最终有效规则；早期需求中的推荐色值不覆盖已确认画面。

## 1. 颜色

| Token | 值 | 对应用途 |
| --- | --- | --- |
| `bg.window` | `#191B1D` | 顶栏、窗口基础 |
| `bg.sidebar` | `#1C1E20` | 项目/会话与设置导航 |
| `bg.workspace` | `#232527` | 对话与设置内容 |
| `bg.auxiliary` | `#202224` | 辅助面板 |
| `bg.surface` | `#2A2C2F` | 通用表面 |
| `bg.composer` | `#2B2D30` | 输入区 |
| `bg.user-message` | `#2C2E31` | 用户消息 |
| `bg.result` | `#2C3032` | 结果摘要 |
| `bg.file-result` | `#2C2F32` | 文件变更卡片 |
| `bg.tool` | `#292C2E` | 工具摘要 |
| `bg.code` | `#1D2022` | 代码块正文 |
| `bg.code-header` | `#2B2F32` | 代码块标题行 |
| `bg.terminal` | `#191D1F` | 终端内容 |
| `bg.hover` | `#303236` | 通用悬停 |
| `bg.selected` | `#35383C` | 通用选中/按下 |
| `bg.nav-selected` | `#303236` | 项目与会话选中 |
| `bg.tab-selected` | `#33363A` | 内容标签选中 |
| `bg.tab-hover` | `#292C2F` | 非选中标签悬停 |
| `bg.tab-selected-hover` | `#383C40` | 已选中标签悬停 |
| `bg.web-tab-selected` | `#383E43` | 浏览器当前网页标签 |
| `bg.popover` | `#2B2E32` | 选择器、菜单 |
| `border.subtle` | `#36393D` | 区域边界 |
| `border.composer` | `#45484D` | 输入区边界 |
| `border.composer-focus` | `#69717C` | 输入聚焦 |
| `border.code` | `#3C4247` | 代码块边界 |
| `border.file-hover` | `#535B63` | 文件卡片悬停 |
| `text.primary` | `#E6E7E9` | 主文字 |
| `text.secondary` | `#ADB1B7` | 辅助文字 |
| `text.muted` | `#858B92` | 弱提示 |
| `text.body` | `#D2D5DA` | Agent 正文 |
| `text.result-title` | `#EDF0F2` | 重要结论 |
| `text.code` | `#CBD4DD` | 代码块文字 |
| `focus.ring` | `#A9B8CA` | 键盘焦点环 |
| `status.success` | `#A7BF9C` | 已完成、增量 |
| `status.warning` | `#D2B37D` | 待审批、未保存 |
| `status.error` | `#D99B98` | 错误、删除量 |
| `action.primary-bg` | `#D7DADF` | 默认提交按钮 |
| `action.primary-text` | `#202225` | 默认提交按钮文字 |

原型内嵌的浅色 Atlas 示例网页属于网页内容，不进入工作台主题。终端 ANSI 颜色与文件差异颜色按数据含义处理，不能作为全局装饰色。

## 2. 文字与图标

| Token | 尺寸 / 字重 / 行高 | 用途 |
| --- | --- | --- |
| `type.ui` | 13 / 400 / 1.5 | 应用默认文字 |
| `type.body` | 14 / 400 / 1.85 | Agent 正文 |
| `type.body-large` | 16 / 400 / 1.85 | 设置中的较大对话字号 |
| `type.result-title` | 17 / 600 / 自然行高 | 回复重要结论 |
| `type.section` | 13 / 500–600 / 1.5 | 局部标题、结果卡标题 |
| `type.tab` | 12 / 400 / 1.5 | 顶部内容标签 |
| `type.control-compact` | 11 / 400 / 1.5 | Composer 选择器、代码标题、网页标签 |
| `type.code` | 12 / 400 / 1.85 | 代码块 |
| `type.terminal` | 12 / 400 / 1.9 | 终端内容 |
| `type.metadata` | 10–11 / 400 / 1.5 | 时间、增删统计、辅助信息 |
| `type.settings-title` | 24 / 500 / 1.5 | 设置页标题 |
| `icon.default` | 16 | 常用控件 |
| `icon.compact` | 13–14 | 标签与输入选择器 |
| `icon.result` | 17–19 | 文件、结果卡片 |

macOS 使用系统 UI 字体；Windows 使用系统 UI 字体与 Microsoft YaHei 等 CJK 回退。代码/终端优先平台等宽字体。中文、英文与符号同排时按真实字体度量验证基线。

通用图标使用 Lucide，与原型相同。正式打包保留其许可证及来源；Agent 品牌图标如另行引入，使用可追踪的官方资产。图标按钮带可访问名称；含义不直观或有快捷键时提供简短 Tooltip。

## 3. 布局与尺寸

| Token | 基准值 | 行为 |
| --- | --- | --- |
| `window.default` | 1240 × 800 | 首次打开尺寸 |
| `window.minimum` | 980 × 640 | 本期最小桌面验收尺寸 |
| `sidebar.default/min/max` | 264 / 240 / 320 | 可折叠与拖动 |
| `aux.default/min` | 400 / 300 | 与对话空间共同约束 |
| `chat.minimum-with-aux` | 380 | 正常双区展示的可用宽度 |
| `chrome.height` | 48 | 集成窗口顶栏 |
| `context.height` | 44 | 会话和辅助面板上下文行 |
| `content-tab.height` | 32 | 顶部标签 |
| `content-tab.width` | 105–194 | 长标题截断，溢出横向滚动 |
| `web-tab.height` | 29 | 浏览器内网页标签 |
| `navigation.row` | 36 | 项目和会话 |
| `conversation.max-width` | 780 | 回复阅读宽度 |
| `composer.max-width` | 820 | 底部输入区 |
| `composer.text-min/max` | 63 / 180 | 多行增长后内部滚动 |
| `composer.bottom-inset` | 20 | 底部留白 |
| `popover.width` | 286 | 模型等下拉框；内容更长时按可用范围调整 |
| `popover.max-height` | min(440, 视口80%) | 内部列表滚动 |
| `file-card.min-width` | 190 | 宽处按列、窄处单列 |
| `file-card.gap` | 8 | 同级文件卡片间距 |
| `resize.hit-width` | 5–6 | 容易命中的拖动区域 |

会话内边距 22–58，Composer 内边距 18–38，随窗口宽度变化；同一内容组共享对齐线。文件卡片在窄工作区改为单列，布局不因文件名或代码长度被撑开。

原型小于 800 的临时网页适配不是本期桌面窗口规格。原生实现以 980×640 及以上为验收范围，同时正确处理系统缩放与更长文案。

## 4. 间距、圆角与边界

- 间距基础：4；常用 8 / 12 / 16 / 24；信息组之间允许更大的阅读留白。
- 控件、标签圆角：6；小型次操作：4–5；文件/终端卡片：8；用户消息、结果块和代码块：9；Composer：10；短模态：12。
- 区域边界：1 个逻辑像素的细线，由边界一侧负责。结果组织优先使用实心表面与留白。
- 选中导航使用浅背景、文字提亮及细指示条；当前原型指示条宽 2，高 14。用户已明确选用此表达，作为项目对通用设计指南的有意定制。
- 基础区域无投影；原型 Popover 使用 `0 8px 24px #0004` 的局部层叠阴影，正文卡片保持平面。
- 键盘焦点为 2px 外描边、2px 间隔；布局为焦点环留空间，裁切不能抹掉焦点。

## 5. 状态组合与动效

| 场景 | 时间 / 曲线 | 变化属性 |
| --- | --- | --- |
| Hover / Selected / Pressed | 120ms / 普通缓动 | 背景、文字、边界 |
| 面板打开/收起/放大/还原 | 180ms / cubic-bezier(.2,.8,.2,1) | 区域宽度、内容透明度 |
| 弹层进入 | 120ms / 同上 | 透明度、最多 3px 位移 |
| 弹层退出 | 100ms / ease | 透明度 |
| 设置内容切换 | 180ms / 同上 | 透明度、最多 3px 位移 |
| 分界线拖动、光标、选择 | 直接响应 | 实际位置和文本选择 |
| 流式文字 | 按内容到达更新 | 文本和所需布局，无整段重播动画 |
| Reduced motion | 0ms | 保留状态结果，以即时切换完成 |

状态可组合：选中同时悬停或聚焦，选中语义保持；Focus 环叠加而非代替选中；Disabled 阻止激活及误导性悬停；Loading 保留控件几何。完整状态机见[交互约定](interaction-notes.md)。

## 6. 映射到 GPUI

1. 调色板与尺寸集中在主题/设计系统模块。业务组件只引用语义 Token。
2. 100% 尺寸用于视觉校准；按当前根字体换算为 GPUI 的相对单位，使用组件的语义大小和当前版本的布局接口。不要假设框架预设的某个 spacing 名称恰好等于原型中的 px。
3. 基础主题值通过框架主题更新入口同步；产品扩展间距/尺寸由自有设计系统状态持有。
4. 窗口控制、安全区域、栅格图片和真实拖动边界是平台/物理尺寸，按测量处理。
5. 选择器、菜单、焦点、列表、编辑器优先依赖 gpui-component 的行为；业务结果卡片在此基础上组合。
6. 当前“对话字号”只调整回复阅读字号。若后来加入全应用缩放，需要连同间距、图标、控件和测量缓存一并处理，不与面板放大混用。

## 7. 原型已确认与实施检查

已确认：信息层级、布局比例、顶部内容标签、右侧工具、实体结果块、颜色方向、独立设置页、克制动效。

实施检查：GPUI 真窗口的文字度量、IME、DPI、WebView 裁切/层叠、PTY 字形与实际交互。控件状态必须在真实数据和最小尺寸下保持，不能仅凭静态截图宣称完成。

原型中的示例成功、文件统计、终端输出和网页内容不转为产品常量；它们仅确定呈现方式。
