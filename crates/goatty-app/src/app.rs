//! GPUI bootstrap: settings global, key bindings, the main window.

use std::sync::Arc;

use goatty_config::Settings;
use gpui::{App, AppContext, Bounds, Global, WindowBounds, WindowOptions, px, size};

use crate::session::Session;
use crate::workspace::Workspace;

/// Settings shared by every view. Read with `cx.global::<AppSettings>()`.
/// Stage 4 replaces it when the user edits settings; views that care
/// observe the global.
pub struct AppSettings(pub Arc<Settings>);

impl Global for AppSettings {}

pub fn run(mut settings: Settings, session: Arc<Session>) {
    gpui_platform::application().run(move |cx: &mut App| {
        // Resolve the font once, against what's actually installed.
        let family = settings.font.resolve_family(&cx.text_system().all_font_names());
        log::info!("font: {family} {}px", settings.font.size);
        settings.font.family = Some(family);
        cx.set_global(AppSettings(Arc::new(settings)));
        crate::actions::bind_default_keys(cx);

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
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.activate(true);
    });
}
