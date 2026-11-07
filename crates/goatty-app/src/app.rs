//! GPUI bootstrap: settings, key bindings, the main window.

use std::sync::Arc;

use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};

use crate::actions::OpenSettings;
use crate::session::Session;
use crate::settings::AppSettings;
use crate::settings_window::{self, HostLink};
use crate::workspace::Workspace;

pub fn run(session: Arc<Session>, link: HostLink, open_settings: Option<String>) {
    gpui_platform::application().run(move |cx: &mut App| {
        AppSettings::init(cx);
        settings_window::set_host_link(cx, Some(link));
        cx.on_action(|_: &OpenSettings, cx| settings_window::open(cx));

        let bounds = Bounds::centered(None, size(px(960.), px(600.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some("dev.goatty.goatty".into()),
                ..Default::default()
            },
            |window, cx| {
                window.set_window_title("goatty");
                cx.new(|cx| Workspace::new(session, window, cx))
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
