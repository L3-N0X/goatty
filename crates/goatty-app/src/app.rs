//! GPUI bootstrap: settings, key bindings, the main window.

use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};

use crate::actions::{OpenSettings, ReloadShells};
use crate::command_palette;
use crate::session::Spawner;
use crate::settings::AppSettings;
use crate::settings_window::{self, HostLink};
use crate::toast::{self, Toast};
use crate::workspace::Workspace;
use crate::workspace::saved::Startup;

pub fn run(startup: Startup, spawner: Spawner, link: HostLink, open_settings: Option<String>) {
    gpui_platform::application().with_assets(crate::ui::icons::Assets).run(move |cx: &mut App| {
        AppSettings::init(cx);
        settings_window::set_host_link(cx, Some(link));
        cx.on_action(|_: &OpenSettings, cx| settings_window::open(cx));
        cx.on_action(|_: &ReloadShells, cx| reload_shells(cx));
        command_palette::register_builtins(cx);

        let bounds = Bounds::centered(None, size(px(960.), px(600.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some("dev.goatty.goatty".into()),
                ..Default::default()
            },
            |window, cx| {
                window.set_window_title("goatty");
                cx.new(|cx| Workspace::new(startup, spawner, window, cx))
            },
        )
        .expect("open window");
        if let Some(page) = open_settings {
            settings_window::open_at(cx, settings_window::Page::from_name(&page));
        }
        // Quit with the last window (the settings window may outlive the
        // terminal window briefly, or close first).
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}

/// Ask the host's running managed shells to reload (the command and the
/// shortcut; the settings window has its own button). The result shows as
/// a toast.
fn reload_shells(cx: &mut App) {
    let Some(link) = cx.try_global::<settings_window::HostLinkGlobal>().and_then(|g| g.0.clone()) else {
        toast::show(cx, Toast::warning("Can't reload shells").body("This window isn't connected to a goatty host."));
        return;
    };
    let id = toast::show(cx, Toast::loading("Reloading shells…").key("shell.reload"));
    cx.spawn(async move |cx| {
        let result = cx.background_executor().spawn(async move { link.reload_shells() }).await;
        cx.update(|cx| toast::update(cx, id, reload_result(result)));
    })
    .detach();
}

fn reload_result(result: anyhow::Result<u32>) -> Toast {
    let t = match result {
        Ok(0) => Toast::info("No goatty shells to reload").body("Only managed shells (goatty's zsh) can reload; none are running."),
        Ok(n) => Toast::success(if n == 1 { "Asked 1 shell to reload".to_string() } else { format!("Asked {n} shells to reload") })
            .body("Idle prompts restart now, busy shells after their current command."),
        Err(e) => Toast::error("Couldn't reload shells")
            .body(format!("{e:#}"))
            .action("Retry", |_, cx| reload_shells(cx))
            .action("Shell settings", |_, cx| settings_window::open_at(cx, Some(settings_window::Page::Shell))),
    };
    t.key("shell.reload")
}
