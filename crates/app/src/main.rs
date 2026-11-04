#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod application;
mod composer;
mod conversation;
mod fixture;
mod host;
mod navigation;
mod settings;
mod settings_agents;
mod sidebar;
mod theme;
mod workbench;

use gpui_kit::component::TitleBar;
use gpui_kit::*;
use workbench::{
    FocusComposer, FocusNavigation, NewSession, OpenProject, OpenSettings, Quit, Workbench,
};

fn main() {
    application().with_assets(assets::Assets).run(|cx| {
        init(cx);
        theme::install(cx);
        let fixture = fixture::FixturePreset::from_env_or_args();
        let data_root = data_root();
        let application =
            cx.new(|cx| application::ApplicationState::new(data_root, fixture.is_none(), cx));
        cx.set_global(application::ApplicationOwner(application));
        cx.bind_keys([
            KeyBinding::new("ctrl-o", OpenProject, Some("Workbench")),
            KeyBinding::new("ctrl-n", NewSession, Some("Workbench")),
            KeyBinding::new("ctrl-q", Quit, None),
            KeyBinding::new("ctrl-,", OpenSettings, Some("Workbench")),
            KeyBinding::new("ctrl-l", FocusComposer, Some("Workbench")),
            KeyBinding::new("ctrl-shift-e", FocusNavigation, Some("Workbench")),
        ]);
        cx.on_action(|_: &Quit, cx| {
            let application = cx.global::<application::ApplicationOwner>().0.clone();
            application.update(cx, |app, cx| app.request_quit(cx));
        });
        cx.on_window_closed(|cx, _| {
            // Bootstrap lifecycle: tray ownership will replace this in the MVP.
            if cx.windows().is_empty() {
                let application = cx.global::<application::ApplicationOwner>().0.clone();
                application.update(cx, |app, cx| app.request_quit(cx));
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

fn data_root() -> Option<std::path::PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--data-root" {
            return Some(args.next().map(Into::into).unwrap_or_default());
        }
        if let Some(value) = arg
            .to_str()
            .and_then(|arg| arg.strip_prefix("--data-root="))
        {
            return Some(value.into());
        }
    }
    None
}
