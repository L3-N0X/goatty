//! GPUI bootstrap: settings, key bindings, the main window.

use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};

use crate::actions::{OpenSettings, ReloadShells};
use crate::command_palette;
use crate::connect::ConnectPlan;
use crate::session::Spawner;
use crate::settings::AppSettings;
use crate::settings_window;
use crate::toast::{self, Toast};
use crate::workspace::Workspace;
use crate::workspace::saved::{SavedWindow, Startup};

/// What a terminal window starts with.
pub enum Initial {
    /// A session to show right away (`--local`).
    Ready { startup: Startup, spawner: Spawner },
    /// A goattyd to connect to first: the window opens at once, its tab
    /// shows the connect screen meanwhile (`crate::connect`, #37).
    Connect(ConnectPlan),
}

pub fn run(initial: Initial, open_settings: Option<String>) {
    gpui_platform::application().with_assets(crate::ui::icons::Assets).run(move |cx: &mut App| {
        AppSettings::init(cx);
        cx.on_action(|_: &OpenSettings, cx| settings_window::open(cx));
        cx.on_action(|_: &ReloadShells, cx| reload_shells(cx));
        command_palette::register_builtins(cx);
        // `~/.ssh/config`'s hosts, ready for the palette.
        crate::hosts::refresh(cx);

        crate::welcome::check_first_run(cx);
        open_terminal_window(cx, initial, open_settings);
        crate::welcome::open_if_first_run(cx);
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

/// Open a terminal window starting with `initial`. `open_settings`
/// (`--settings[=PAGE]`) opens the settings window too, once the window's
/// host is known.
pub fn open_terminal_window(cx: &mut App, initial: Initial, open_settings: Option<String>) {
    let (initial, open_settings) = match initial {
        Initial::Connect(mut plan) => {
            plan.open_settings = plan.open_settings.or(open_settings);
            (Initial::Connect(plan), None)
        }
        ready => (ready, open_settings),
    };
    // A restored layout resizes the window when it arrives (`Workspace`).
    let (w, h) = SavedWindow::DEFAULT;
    let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            app_id: Some("dev.goatty.goatty".into()),
            window_background: crate::workspace::background_appearance(&cx.global::<AppSettings>().client.terminal),
            ..Default::default()
        },
        |window, cx| {
            window.set_window_title("goatty");
            cx.new(|cx| Workspace::new(initial, window, cx))
        },
    );
    if let Err(e) = opened {
        log::error!("couldn't open the terminal window: {e:#}");
        // From the command line there's nothing else; from the palette the
        // other windows stay.
        if cx.windows().is_empty() {
            cx.quit();
        }
        return;
    }
    if let Some(page) = open_settings {
        settings_window::open_at(cx, settings_window::Page::from_name(&page));
    }
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
