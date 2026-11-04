#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod theme;
mod workbench;

use gpui_kit::component::TitleBar;
use gpui_kit::*;
use workbench::{OpenProject, Quit, Workbench};

fn main() {
    application().with_assets(assets::Assets).run(|cx| {
        init(cx);
        theme::install(cx);
        cx.bind_keys([
            KeyBinding::new("ctrl-o", OpenProject, Some("Workbench")),
            KeyBinding::new("ctrl-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_window_closed(|cx, _| {
            // Bootstrap lifecycle: tray ownership will replace this in the MVP.
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                theme::initial_window_size(),
                cx,
            ))),
            window_min_size: Some(theme::minimum_window_size()),
            titlebar: Some(TitlebarOptions {
                title: Some("Agent Workbench".into()),
                ..TitleBar::title_bar_options()
            }),
            app_id: Some("dev.agent-workbench.desktop".into()),
            ..TitleBar::window_options()
        };
        open_window(options, cx, |window, cx| {
            cx.new(|cx| Workbench::new(window, cx))
        })
        .expect("could not open Agent Workbench window");
        cx.activate(true);
    });
}
