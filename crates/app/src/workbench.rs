use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, TitleBar,
    button::{Button, ButtonVariants},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::theme;

actions!(workbench, [OpenProject, Quit]);

pub struct Workbench {
    focus: FocusHandle,
    directory: Option<PathBuf>,
    picker_open: bool,
    error: Option<SharedString>,
    picker_task: Option<Task<()>>,
}

impl Workbench {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            focus,
            directory: None,
            picker_open: false,
            error: None,
            picker_task: None,
        }
    }

    fn open_project(&mut self, _: &OpenProject, _: &mut Window, cx: &mut Context<Self>) {
        if self.picker_open {
            return;
        }
        self.picker_open = true;
        self.error = None;
        let response = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("选择项目文件夹".into()),
        });
        self.picker_task = Some(cx.spawn(async move |this, cx| {
            let result = response.await;
            // The view may have closed while the native picker was open.
            let _ = this.update(cx, |this, cx| {
                this.picker_open = false;
                match result {
                    Ok(Ok(Some(paths))) => this.directory = paths.into_iter().next(),
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => {
                        this.error = Some(format!("无法打开文件夹：{error}").into());
                    }
                    Err(_) => this.error = Some("文件夹选择器已断开，请重试。".into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn project_name(&self) -> SharedString {
        self.directory
            .as_ref()
            .map(|path| {
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned()
                    .into()
            })
            .unwrap_or_else(|| "未选择项目".into())
    }
}

impl Render for Workbench {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let heading = self
            .directory
            .as_ref()
            .map(|_| self.project_name())
            .unwrap_or_else(|| "打开一个项目".into());
        let description: SharedString = if self.directory.is_some() {
            "工作目录已选择。Agent 连接将在后续版本开放。".into()
        } else {
            "选择本地文件夹作为工作目录。".into()
        };
        div()
            .key_context("Workbench")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::open_project))
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                TitleBar::new()
                    // Override the component's default gradient with our solid surface.
                    .bg(cx.theme().background)
                    .border_color(cx.theme().background)
                    .child(div().text_sm().child("Agent Workbench")),
            )
            .child(
                div()
                    .flex()
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w(theme::sidebar_width())
                            .flex_shrink_0()
                            .p_4()
                            .gap_4()
                            .bg(cx.theme().sidebar)
                            .border_r_1()
                            .border_color(cx.theme().sidebar_border)
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("项目"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(Icon::new(IconName::FolderOpen))
                                    .child(div().min_w_0().truncate().child(self.project_name())),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .p_6()
                            .items_center()
                            .justify_center()
                            .gap_4()
                            .child(
                                Icon::new(IconName::FolderOpen)
                                    .size_8()
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(heading),
                            )
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(description),
                            )
                            .when_some(self.directory.as_ref(), |view, path| {
                                view.child(
                                    div()
                                        .max_w_full()
                                        .truncate()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(path.display().to_string()),
                                )
                            })
                            .child(
                                Button::new("open-project")
                                    .outline()
                                    .icon(IconName::FolderOpen)
                                    .label(if self.directory.is_some() {
                                        "更换文件夹…"
                                    } else {
                                        "选择文件夹…"
                                    })
                                    .disabled(self.picker_open)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_project(&OpenProject, window, cx)
                                    })),
                            )
                            .when_some(self.error.clone(), |view, error| {
                                view.child(div().text_color(cx.theme().danger).child(error))
                            }),
                    ),
            )
    }
}
